use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::RecursiveMode;
use notify::Watcher;
use notify_debouncer_full::{new_debouncer, DebounceEventResult};

use crate::{
    prelude::{app_dir, StrictPath},
    resource::{
        config::{BackupFilter, Config, ToggledPaths, ToggledRegistry},
        manifest::Manifest,
    },
    scan::{layout::BackupLayout, scan_game_for_backup, Launchers, SteamShortcuts, TitleFinder},
    sync::{
        conflict::{determine_sync_type, DirectoryScanResult, SyncStatus},
        device::DeviceIdentity,
        operations::{
            classify_error, download_game, extract_root_from_scan, read_game_list_from_cloud, upload_game,
            write_game_list_to_cloud, ErrorCategory, OperationDirection,
        },
    },
};

/// Información de un error persistida en daemon-status.json para que la GUI la lea.
#[derive(Clone, Debug)]
struct ErrorInfo {
    category: ErrorCategory,
    direction: OperationDirection,
    message: String,
}

const NOTIFY_DELAY: Duration = Duration::from_secs(10);
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(60);

struct GameDebounceState {
    first_event: Instant,
    last_event: Instant,
}

impl GameDebounceState {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            first_event: now,
            last_event: now,
        }
    }

    fn update(&mut self) {
        self.last_event = Instant::now();
    }

    fn should_fire(&self) -> bool {
        let now = Instant::now();
        now.duration_since(self.last_event) > NOTIFY_DELAY || now.duration_since(self.first_event) > NOTIFY_TIMEOUT
    }
}

pub struct DaemonConfig;

impl Default for DaemonConfig {
    fn default() -> Self {
        Self
    }
}

pub fn start_daemon(stop_flag: Arc<AtomicBool>, _daemon_config: DaemonConfig) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        log::info!("[sync daemon] Starting");

        if let Err(e) = run_daemon(stop_flag) {
            log::error!("[sync daemon] Fatal error: {e}");
        }

        log::info!("[sync daemon] Stopped");
    })
}

fn run_daemon(stop_flag: Arc<AtomicBool>) -> Result<(), String> {
    let config = Config::load().map_err(|e| format!("Failed to load config: {e:?}"))?;
    let app_dir = app_dir();

    // Check profundo de rclone: ¿existe y funciona?
    if !crate::sync::operations::rclone_available_deep(&config) {
        log::warn!("[sync daemon] Rclone is not available (not installed or not working)");
        write_rclone_missing_flag(&app_dir, true);
        // Esperar en loop a que se arregle. Si el usuario instala rclone y
        // reinicia el daemon (o cambia sync-games.json), se retomará.
        let mut wait = 0u64;
        while !stop_flag.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
            wait += 1;
            if wait >= 60 {
                log::info!("[sync daemon] Retrying rclone check...");
                return run_daemon(stop_flag);
            }
        }
        return Ok(());
    }

    // Rclone disponible — limpiar el flag por si estaba activo de una ejecución anterior
    write_rclone_missing_flag(&app_dir, false);

    if config.cloud.remote.is_none() {
        log::warn!("[sync daemon] No cloud remote configured, cannot start");
        return Ok(());
    }

    let device = DeviceIdentity::load_or_create(&app_dir);

    log::info!("[sync daemon] Running as device: {} ({})", device.name, device.id);

    if stop_flag.load(Ordering::Relaxed) {
        return Ok(());
    }

    if stop_flag.load(Ordering::Relaxed) {
        return Ok(());
    }

    // Paso 3: construir watched_paths desde sync-games.json + game-list del cloud
    let sync_config = crate::sync::sync_config::SyncGamesConfig::load();
    let managed_games: Vec<String> = sync_config.games.iter()
        .filter(|(_, cfg)| match cfg.mode {
            crate::sync::sync_config::SaveMode::None => false,
            crate::sync::sync_config::SaveMode::Local => cfg.auto_sync,
            crate::sync::sync_config::SaveMode::Cloud => cfg.auto_sync,
            crate::sync::sync_config::SaveMode::Sync => true,
        })
        .map(|(name, _)| name.clone())
        .collect();

    // Leer o crear el game-list del cloud
    let mut game_list = read_game_list_from_cloud(&config).unwrap_or_default();

    // Auto-registrar rutas para juegos sin ruta en este dispositivo
    if let Err(e) = auto_register_paths(&config, &device, &sync_config) {
        log::error!("[sync daemon] Error during path auto-registration: {e}");
    }

    // Releer el game-list después de auto-registro
    let game_list = read_game_list_from_cloud(&config).unwrap_or_else(|| {
        let local_path = crate::prelude::app_dir().joined("ludusavi-game-list.json");
        if let Some(content) = local_path.read() {
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            crate::sync::game_list::GameListFile::default()
        }
    });

    let mut watched_paths: HashMap<String, String> = game_list
        .games
        .iter()
        .filter(|g| {
            let mode = sync_config.get_mode(&g.id);
            let auto_sync = sync_config.get_auto_sync(&g.id);
            match mode {
                crate::sync::sync_config::SaveMode::None => false,
                crate::sync::sync_config::SaveMode::Local => false, // LOCAL se maneja abajo
                crate::sync::sync_config::SaveMode::Cloud => auto_sync,
                crate::sync::sync_config::SaveMode::Sync => true,
            }
        })
        .filter_map(|g| {
            g.path_by_device
                .get(&device.id)
                .map(|entry| (g.id.clone(), entry.path.clone()))
        })
        .collect();
    
    // Para LOCAL + auto sync ON, resolver la ruta directamente sin game_list
    for (game_id, cfg) in &sync_config.games {
        if cfg.mode != crate::sync::sync_config::SaveMode::Local || !cfg.auto_sync {
            continue;
        }
        if watched_paths.contains_key(game_id) {
            continue;
        }
        if let Some(path) = crate::sync::operations::resolve_game_path_from_manifest(&config, game_id) {
            log::info!("[sync daemon] Resolved LOCAL path for {}: {}", game_id, path);
            watched_paths.insert(game_id.clone(), path);
        } else {
            log::warn!("[sync daemon] Cannot resolve LOCAL path for: {}", game_id);
        }
    }

    // Si sync-games.json tiene juegos pero ninguno tiene ruta aún, arrancar igual
    // y monitorizar cuando se registren
    if watched_paths.is_empty() && managed_games.is_empty() {
        log::info!("[sync daemon] No games configured in sync-games.json, will retry in 30s...");
        let mut wait = 0u64;
        while !stop_flag.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
            wait += 1;
            if wait >= 30 {
                log::info!("[sync daemon] Retrying startup...");
                return run_daemon(stop_flag);
            }
        }
        return Ok(());
    } else if watched_paths.is_empty() {
        log::info!("[sync daemon] Games configured but no paths resolved yet, will retry in 30s...");
        let mut wait = 0u64;
        while !stop_flag.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
            wait += 1;
            if wait >= 30 {
                return run_daemon(stop_flag);
            }
        }
        return Ok(());
    }
    log::info!("[sync daemon] Watching {} game(s) for changes", watched_paths.len());

    // Paso 4: estado de debounce compartido
    let debounce_state: Arc<Mutex<HashMap<String, GameDebounceState>>> = Arc::new(Mutex::new(HashMap::new()));

    let path_to_game: Arc<HashMap<String, String>> = Arc::new(
        watched_paths
            .iter()
            .map(|(game_id, path)| (normalize_path(path), game_id.clone()))
            .collect(),
    );

    // Paso 1: comprobación inmediata de descargas al arrancar
    log::info!("[sync daemon] Checking cloud for downloads on startup...");
    if let Err(e) = check_downloads(&config, &app_dir, &device) {
        log::error!("[sync daemon] Error during startup download check: {e}");
    }

    // Paso 5: arrancar el file watcher
    let debounce_state_watcher = debounce_state.clone();
    let path_to_game_watcher = path_to_game.clone();

    let mut debouncer = new_debouncer(
        Duration::from_secs(1),
        None,
        move |result: DebounceEventResult| match result {
            Ok(events) => {
                let mut state = debounce_state_watcher.lock().unwrap();
                let mut dirty_games = HashSet::new();

                for event in events {
                    for path in &event.paths {
                        let path_str = normalize_path(&path.to_string_lossy());
                        for (watch_path, game_id) in path_to_game_watcher.iter() {
                            if path_str.starts_with(watch_path.as_str()) {
                                dirty_games.insert(game_id.clone());
                                break;
                            }
                        }
                    }
                }
                for game_id in dirty_games {
                    state
                        .entry(game_id.clone())
                        .and_modify(|s| s.update())
                        .or_insert_with(GameDebounceState::new);
                    log::debug!("[sync daemon] Change detected for game: {}", game_id);
                }
            }
            Err(e) => {
                log::error!("[sync daemon] Watcher error: {:?}", e);
            }
        },
    )
    .map_err(|e| format!("Failed to create file watcher: {e}"))?;

    for (game_id, path) in &watched_paths {
        if Path::new(path).is_dir() {
            match debouncer.watcher().watch(Path::new(path), RecursiveMode::Recursive) {
                Ok(_) => log::info!("[sync daemon] Watching: {} ({})", path, game_id),
                Err(e) => log::warn!("[sync daemon] Failed to watch {}: {}", path, e),
            }
        } else {
            log::warn!("[sync daemon] Path does not exist yet, skipping watch: {}", path);
        }
    }

    // Paso 6: worker loop principal
    log::info!("[sync daemon] File watcher active, monitoring for changes");

    let mut last_known_mod_time = crate::sync::operations::get_game_list_mod_time(&config);
    save_last_mod_time(&app_dir, &last_known_mod_time);
    let mut poll_counter: u64 = 0;
    const POLL_EVERY_N_SECONDS: u64 = 30;
    let mut last_sync_games_mtime = get_sync_games_mtime();

    while !stop_flag.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_secs(1));
        poll_counter += 1;

        // Detectar cambios en sync-games.json y reiniciar si cambió
        let current_sync_games_mtime = get_sync_games_mtime();
        if current_sync_games_mtime != last_sync_games_mtime && last_sync_games_mtime.is_some() {
            log::info!("[sync daemon] sync-games.json changed, restarting to pick up new config...");
            return run_daemon(stop_flag);
        }
        last_sync_games_mtime = current_sync_games_mtime;

        let ready_games: Vec<String> = {
            let state = debounce_state.lock().unwrap();
            state
                .iter()
                .filter(|(_, s)| s.should_fire())
                .map(|(id, _)| id.clone())
                .collect()
        };

        if poll_counter >= POLL_EVERY_N_SECONDS {
            poll_counter = 0;

            let config = match Config::load() {
                Ok(c) => c,
                Err(e) => {
                    log::error!("[sync daemon] Failed to reload config for poll: {e:?}");
                    continue;
                }
            };

            let current_mod_time = crate::sync::operations::get_game_list_mod_time(&config);

            if current_mod_time.is_some() && current_mod_time != last_known_mod_time {
                log::info!("[sync daemon] Cloud game list changed, checking for downloads...");
                last_known_mod_time = current_mod_time.clone();
                save_last_mod_time(&app_dir, &last_known_mod_time);

                if let Err(e) = check_downloads_and_rewatch(&config, &app_dir, &device, &mut debouncer, &watched_paths)
                {
                    log::error!("[sync daemon] Error during poll download check: {e}");
                }
                // Actualizar mod time tras descarga para no redetectar nuestro propio write
                last_known_mod_time = crate::sync::operations::get_game_list_mod_time(&config);
                save_last_mod_time(&app_dir, &last_known_mod_time);

                // Si hay juegos nuevos para este dispositivo, reiniciar el daemon
                // para que entren en el file watcher
                let new_game_list = read_game_list_from_cloud(&config).unwrap_or_default();
                let new_paths: Vec<String> = new_game_list
                    .games
                    .iter()
                    .filter_map(|g| g.path_by_device.get(&device.id).map(|e| e.path.clone()))
                    .collect();
                let has_new_games = new_paths.iter().any(|p| {
                    let norm = normalize_path(p);
                    !path_to_game.contains_key(&norm)
                });
                if has_new_games {
                    log::info!("[sync daemon] New games detected, restarting to update file watcher...");
                    return run_daemon(stop_flag);
                }
            } else {
                log::debug!("[sync daemon] Cloud game list unchanged, skipping download check");
            }
        }

        if ready_games.is_empty() {
            continue;
        }

        log::info!(
            "[sync daemon] Processing {} game(s) ready for upload",
            ready_games.len()
        );

        let config = match Config::load() {
            Ok(c) => c,
            Err(e) => {
                log::error!("[sync daemon] Failed to reload config: {e:?}");
                continue;
            }
        };

        let mut game_list = read_game_list_from_cloud(&config).unwrap_or_else(|| {
            let local_path = crate::prelude::app_dir().joined("ludusavi-game-list.json");
            if let Some(content) = local_path.read() {
                serde_json::from_str(&content).unwrap_or_default()
            } else {
                crate::sync::game_list::GameListFile::default()
            }
        });
        let mut any_changes = false;

        let sync_config = crate::sync::sync_config::SyncGamesConfig::load();
        let mut error_games: std::collections::HashMap<String, ErrorInfo> = std::collections::HashMap::new();

    for game_id in &ready_games {
            let mode = sync_config.get_mode(game_id);

            if matches!(mode, crate::sync::sync_config::SaveMode::None) {
                log::debug!("[sync daemon] Skipping — mode is None for {}", game_id);
                debounce_state.lock().unwrap().remove(game_id);
                continue;
            }

            if matches!(mode, crate::sync::sync_config::SaveMode::Local) {
                let auto_sync = sync_config.get_auto_sync(game_id);
                if !auto_sync {
                    log::debug!("[sync daemon] Local mode, auto-sync off — skipping for {}", game_id);
                    debounce_state.lock().unwrap().remove(game_id);
                    continue;
                }
                let local_path = game_list.get_game(game_id)
                    .and_then(|g| g.path_by_device.get(&device.id).map(|e| e.path.clone()))
                    .or_else(|| crate::sync::operations::resolve_game_path_from_manifest(&config, game_id));
                if let Some(path) = local_path {
                    let zip_path = config.backup.path.joined(&format!("{}.zip", game_id));
                    match crate::sync::operations::create_zip_from_folder(&path, &zip_path) {
                        Ok(_) => {
                            log::info!("[sync daemon] Local backup complete: {}", game_id);
                            error_games.remove(game_id);
                        }
                        Err(e) => {
                            log::error!("[sync daemon] Local backup failed for {}: {e}", game_id);
                            let (category, message, direction) = classify_error(&e, OperationDirection::Backup);
                            error_games.insert(game_id.clone(), ErrorInfo { category, direction, message });
                        }
                    }
                } else {
                    log::warn!("[sync daemon] Cannot resolve local path for: {}", game_id);
                    error_games.insert(
                        game_id.clone(),
                        ErrorInfo {
                            category: ErrorCategory::Config,
                            direction: OperationDirection::Backup,
                            message: "Cannot resolve save path. Check manifest or add path manually.".to_string(),
                        },
                    );
                }
                debounce_state.lock().unwrap().remove(game_id);
                continue;
            }

            // CLOUD y SYNC — requieren game_list
            if let Some(game) = game_list.get_game_mut(game_id) {
                let local_path: Option<String> = game.path_by_device.get(&device.id).map(|e| e.path.clone());
                let scan = DirectoryScanResult::scan(local_path.as_deref());
                let status = determine_sync_type(game, &scan, &device.id);

                match &status {
                    SyncStatus::Conflict { local_time, cloud_time, cloud_from } => {
                        log::warn!(
                            "[sync daemon] CONFLICT for {} - local={} cloud={} from={:?} - skipping, user must resolve",
                            game.name,
                            local_time,
                            cloud_time,
                            cloud_from
                        );
                        // Limpiamos el debounce — no insistimos hasta que el usuario decida.
                        debounce_state.lock().unwrap().remove(game_id);
                        // Limpiamos cualquier error previo para que la GUI muestre solo el banner de conflict.
                        error_games.remove(game_id);
                        // No tocamos game_list ni any_changes — el conflict se persiste vía
                        // calculate_game_status que detecta el estado y escribe "conflict" en daemon-status.json.
                        continue;
                    }
                    SyncStatus::RequiresUpload => {
                        log::info!("[sync daemon] Uploading: {}", game.name);
                        match upload_game(&config, &app_dir, &device, game) {
                            Ok(_) => {
                                log::info!("[sync daemon] Upload complete: {}", game.name);
                                any_changes = true;
                                error_games.remove(game_id);
                            }
                            Err(e) => {
                                log::error!("[sync daemon] Upload failed for {}: {e}", game.name);
                                let (category, message, direction) = classify_error(&e, OperationDirection::Upload);
                                error_games.insert(game_id.clone(), ErrorInfo { category, direction, message });
                            }
                        }
                    }
                    _ => {
                        log::info!("[sync daemon] Skipping upload for {} — status is {:?}", game.name, status);
                    }
                }
            } else {
                log::warn!("[sync daemon] Game not found in cloud list: {}", game_id);
            }

            debounce_state.lock().unwrap().remove(game_id);
        }

        if any_changes {
            if let Err(e) = write_game_list_to_cloud(&config, &game_list) {
                log::error!("[sync daemon] Failed to write game list: {e}");
            } else {
                write_game_list_local(&app_dir, &game_list);
                // Actualizar mod time para que el polling no redetecte nuestro propio upload
                last_known_mod_time = crate::sync::operations::get_game_list_mod_time(&config);
                save_last_mod_time(&app_dir, &last_known_mod_time);
            }
        }
        write_sync_status_with_errors(&app_dir, &game_list, &device.id, &config, &sync_config, &error_games);
    }

    log::info!("[sync daemon] Stop flag set, shutting down watcher");
    Ok(())
}

fn save_last_mod_time(app_dir: &StrictPath, mod_time: &Option<String>) {
    let path = app_dir.joined("daemon-state.json");
    let json = serde_json::json!({ "last_known_mod_time": mod_time });
    if let Ok(content) = serde_json::to_string(&json) {
        let _ = std::fs::write(path.as_std_path_buf().unwrap(), content);
    }
}

/// Escribe el flag `rclone_missing` en daemon-status.json sin tocar el resto del fichero.
/// La GUI lee este flag para mostrar un warning en ThisDevice y bloquear cambios a CLOUD/SYNC.
fn write_rclone_missing_flag(app_dir: &StrictPath, missing: bool) {
    let path = app_dir.joined("daemon-status.json");

    // Leer contenido actual (si existe) para no perder los juegos ya escritos
    let mut json = if let Some(content) = path.read() {
        serde_json::from_str::<serde_json::Value>(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Asegurar que es un objeto
    if !json.is_object() {
        json = serde_json::json!({});
    }

    json.as_object_mut().unwrap().insert(
        "rclone_missing".to_string(),
        serde_json::Value::Bool(missing),
    );

    if let Ok(content) = serde_json::to_string(&json) {
        if let Ok(path_buf) = path.as_std_path_buf() {
            let _ = std::fs::write(path_buf, content);
        }
    }
}

/// Para cada juego en el game-list sin ruta registrada para este dispositivo,
/// usa el manifiesto de Ludusavi para encontrar los saves automáticamente.
fn auto_register_paths(
    config: &Config,
    device: &DeviceIdentity,
    sync_config: &crate::sync::sync_config::SyncGamesConfig,
) -> Result<(), String> {
    let mut game_list = match read_game_list_from_cloud(config) {
        Some(gl) => gl,
        None => return Ok(()),
    };

    // Solo juegos configurados como SYNC en este dispositivo
    // que no tienen ruta registrada para este dispositivo en game-list
    let unregistered: Vec<String> = sync_config.games
        .iter()
        .filter(|(_, cfg)| cfg.mode == crate::sync::sync_config::SaveMode::Sync)
        .filter(|(name, _)| {
            !game_list.games.iter()
                .any(|g| &g.id == *name && g.path_by_device.contains_key(&device.id))
        })
        .map(|(name, _)| name.clone())
        .collect();

    // Asegurar que el nombre de este dispositivo está registrado en device_names,
    // incluso si no hay juegos nuevos por registrar.
    if game_list.device_names.get(&device.id) != Some(&device.name) {
        game_list.device_names.insert(device.id.clone(), device.name.clone());
        if let Err(e) = write_game_list_to_cloud(config, &game_list) {
            log::warn!("[sync daemon] Failed to update device_names in game list: {e}");
        } else {
            log::info!("[sync daemon] Registered device name in game list: {}", device.name);
        }
    }

    if unregistered.is_empty() {
        log::debug!("[sync daemon] All SYNC games already have a path for this device");
        return Ok(());
    }

    log::info!(
        "[sync daemon] Auto-registering {} SYNC game(s) for this device",
        unregistered.len()
    );

    // El resto de la función no cambia
    let manifest = match Manifest::load() {
        Ok(m) => m.with_extensions(config),
        Err(e) => {
            log::warn!("[sync daemon] Could not load manifest: {e:?}");
            return Ok(());
        }
    };

    let app_dir = app_dir();
    let roots = config.expanded_roots();
    let layout = BackupLayout::new(config.backup.path.clone());
    let title_finder = TitleFinder::new(config, &manifest, layout.restorable_game_set());
    let steam_shortcuts = SteamShortcuts::scan(&title_finder);
    let launchers = Launchers::scan(&roots, &manifest, &unregistered, &title_finder, None);

    let mut _any_changes = false;

    for game_id in &unregistered {
        // Si el juego no está en game-list todavía, crear entrada vacía
        if !game_list.games.iter().any(|g| &g.id == game_id) {
            game_list.games.push(crate::sync::game_list::GameMetaData::new(
                game_id.clone(),
                game_id.clone(),
            ));
        }

        let game_entry = match manifest.0.get(game_id.as_str()) {
            Some(g) => g,
            None => {
                log::warn!("[sync daemon] Game not found in manifest: {}", game_id);
                continue;
            }
        };

        let scan_info = scan_game_for_backup(
            game_entry,
            game_id,
            &roots,
            &app_dir,
            &launchers,
            &BackupFilter::default(),
            None,
            &ToggledPaths::default(),
            &ToggledRegistry::default(),
            None,
            &config.redirects,
            config.restore.reverse_redirects,
            &steam_shortcuts,
            false,
        );

        match extract_root_from_scan(&scan_info.found_files) {
            Some(root_path) => {
                log::info!("[sync daemon] Auto-registered path for {}: {}", game_id, root_path);
                if let Some(game) = game_list.get_game_mut(game_id) {
                    game.set_path(device.id.clone(), root_path);
                    _any_changes = true;
                }
            }
            None => {
                match crate::sync::operations::resolve_expected_save_path(config, game_entry) {
                    Some(expected_path) => {
                        log::info!(
                            "[sync daemon] Auto-registered expected path for {}: {}",
                            game_id,
                            expected_path
                        );
                        if let Some(game) = game_list.get_game_mut(game_id) {
                            game.set_path(device.id.clone(), expected_path);
                            _any_changes = true;
                        }
                    }
                    None => {
                        log::debug!("[sync daemon] Cannot resolve expected path for {}, skipping", game_id);
                    }
                }
            }
        }
    }

    game_list.device_names.insert(device.id.clone(), device.name.clone());
    write_game_list_to_cloud(config, &game_list).map_err(|e| format!("Failed to write game list: {e}"))?;
    log::info!("[sync daemon] Updated game list with device name and auto-registered paths");

    Ok(())
}

/// Comprueba si hay juegos en el cloud más nuevos que la versión local y los descarga.
fn check_downloads(config: &Config, app_dir: &StrictPath, device: &DeviceIdentity) -> Result<(), String> {
    let mut game_list = match read_game_list_from_cloud(config) {
        Some(gl) => gl,
        None => {
            log::info!("[sync daemon] No game list found in cloud, nothing to download");
            return Ok(());
        }
    };

    let game_ids: Vec<String> = game_list
        .games
        .iter()
        .filter(|g| g.path_by_device.contains_key(&device.id))
        .map(|g| g.id.clone())
        .collect();

    let mut any_changes = false;
    let sync_config = crate::sync::sync_config::SyncGamesConfig::load();
    let mut error_games: std::collections::HashMap<String, ErrorInfo> = std::collections::HashMap::new();

    for game_id in game_ids {
        if let Some(game) = game_list.get_game_mut(&game_id) {
            let mode = sync_config.get_mode(&game.name);
            let auto_sync = sync_config.get_auto_sync(&game.name);
            if matches!(mode, crate::sync::sync_config::SaveMode::None | crate::sync::sync_config::SaveMode::Local) {
                log::debug!("[sync daemon] Skipping download for {} — mode is {:?}", game.name, mode);
                continue;
            }
            if matches!(mode, crate::sync::sync_config::SaveMode::Cloud) && !auto_sync {
                log::debug!("[sync daemon] Skipping startup sync for {} — CLOUD auto sync off", game.name);
                continue;
            }
            let local_path: Option<String> = game.path_by_device.get(&device.id).map(|e| e.path.clone());
            let scan = DirectoryScanResult::scan(local_path.as_deref());
            log::info!(
                "[sync daemon] [{}] scan_latest={:?} game_latest={:?} directory_exists={}",
                game.name,
                scan.latest_write_time_utc,
                game.latest_write_time_utc,
                scan.directory_exists
            );
            let status = determine_sync_type(game, &scan, &device.id);
            log::info!("[sync daemon] [{}] status={:?}", game.name, status);

            match &status {
                SyncStatus::RequiresDownload => {
                    log::info!("[sync daemon] Downloading on startup: {}", game.name);
                    match download_game(config, app_dir, device, game) {
                        Ok(_) => {
                            log::info!("[sync daemon] Download complete: {}", game.name);
                            any_changes = true;
                        }
                        Err(e) => {
                            log::error!("[sync daemon] Download failed for {}: {e}", game.name);
                            let (category, message, direction) = classify_error(&e, OperationDirection::Download);
                            error_games.insert(game.name.clone(), ErrorInfo { category, direction, message });
                        }
                    }
                }
                SyncStatus::RequiresUpload => {
                    log::info!("[sync daemon] Uploading on startup: {}", game.name);
                    match upload_game(config, app_dir, device, game) {
                        Ok(_) => {
                            log::info!("[sync daemon] Upload complete: {}", game.name);
                            any_changes = true;
                        }
                        Err(e) => {
                            log::error!("[sync daemon] Upload failed for {}: {e}", game.name);
                            let (category, message, direction) = classify_error(&e, OperationDirection::Upload);
                            error_games.insert(game.name.clone(), ErrorInfo { category, direction, message });
                        }
                    }
                }
                SyncStatus::InSync => {
                    log::debug!("[sync daemon] {} is in sync", game.name);
                }
                SyncStatus::Conflict { local_time, cloud_time, cloud_from } => {
                    log::warn!(
                        "[sync daemon] CONFLICT for {} - local={} cloud={} from={:?} - skipping, user must resolve",
                        game.name,
                        local_time,
                        cloud_time,
                        cloud_from
                    );
                    // No hacemos nada automático. La GUI verá status=conflict
                    // en daemon-status.json y mostrará el banner de resolución.
                }
                SyncStatus::Unknown | SyncStatus::UnsetDirectory => {
                    log::debug!("[sync daemon] {} has no actionable status: {:?}", game.name, status);
                }
            }
        }
    }

    if any_changes {
        write_game_list_to_cloud(config, &game_list).map_err(|e| format!("Failed to write game list: {e}"))?;
    }

    // Escribir estado de sync al disco para que la GUI lo pueda leer
    write_game_list_local(app_dir, &game_list);
    write_sync_status_with_errors(&app_dir, &game_list, &device.id, &config, &sync_config, &error_games);

    Ok(())
}

/// Igual que check_downloads pero después de cada descarga vuelve a registrar
/// el directorio en el watcher, ya que extract_zip_to_directory borra y recrea
/// el directorio lo que hace que inotify/Windows pierda el track.
fn check_downloads_and_rewatch(
    config: &Config,
    app_dir: &StrictPath,
    device: &DeviceIdentity,
    debouncer: &mut notify_debouncer_full::Debouncer<notify::RecommendedWatcher, notify_debouncer_full::FileIdMap>,
    watched_paths: &HashMap<String, String>,
) -> Result<(), String> {
    let mut game_list = match read_game_list_from_cloud(config) {
        Some(gl) => gl,
        None => {
            log::info!("[sync daemon] No game list found in cloud, nothing to download");
            return Ok(());
        }
    };

    let game_ids: Vec<String> = game_list
        .games
        .iter()
        .filter(|g| g.path_by_device.contains_key(&device.id))
        .map(|g| g.id.clone())
        .collect();

let mut any_changes = false;
    let sync_config = crate::sync::sync_config::SyncGamesConfig::load();
    let mut error_games: std::collections::HashMap<String, ErrorInfo> = std::collections::HashMap::new();

    for game_id in game_ids {
        if let Some(game) = game_list.get_game_mut(&game_id) {
            let mode = sync_config.get_mode(&game.name);
            let auto_sync = sync_config.get_auto_sync(&game.name);
            if matches!(mode, crate::sync::sync_config::SaveMode::None | crate::sync::sync_config::SaveMode::Local) {
                log::debug!("[sync daemon] Skipping download for {} — mode is {:?}", game.name, mode);
                continue;
            }
            if matches!(mode, crate::sync::sync_config::SaveMode::Cloud) && !auto_sync {
                log::debug!("[sync daemon] Skipping startup sync for {} — CLOUD auto sync off", game.name);
                continue;
            }
            let local_path: Option<String> = game.path_by_device.get(&device.id).map(|e| e.path.clone());
            let scan = DirectoryScanResult::scan(local_path.as_deref());
            let status = determine_sync_type(game, &scan, &device.id);

            // Si hay conflict, log warning y skip (no operar).
            if let SyncStatus::Conflict { local_time, cloud_time, cloud_from } = &status {
                log::warn!(
                    "[sync daemon] CONFLICT for {} (poll) - local={} cloud={} from={:?} - skipping",
                    game.name,
                    local_time,
                    cloud_time,
                    cloud_from
                );
                continue;
            }

            if status == SyncStatus::RequiresDownload {
                log::info!("[sync daemon] Downloading (poll): {}", game.name);
                match download_game(config, app_dir, device, game) {
                    Ok(_) => {
                        log::info!("[sync daemon] Download complete: {}", game.name);
                        any_changes = true;

                        // Re-registrar el directorio en el watcher
                        if let Some(path) = watched_paths.get(&game_id) {
                            if Path::new(path).is_dir() {
                                match debouncer.watcher().watch(Path::new(path), RecursiveMode::Recursive) {
                                    Ok(_) => log::info!("[sync daemon] Re-watching: {}", path),
                                    Err(e) => log::warn!("[sync daemon] Failed to re-watch {}: {}", path, e),
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("[sync daemon] Download failed for {}: {e}", game.name);
                        let (category, message, direction) = classify_error(&e, OperationDirection::Download);
                        error_games.insert(game.name.clone(), ErrorInfo { category, direction, message });
                    }
                }
            }
        }
    }

    if any_changes {
        write_game_list_to_cloud(config, &game_list).map_err(|e| format!("Failed to write game list: {e}"))?;
    }

    // Escribir estado de sync al disco para que la GUI lo pueda leer
    write_game_list_local(app_dir, &game_list);
    write_sync_status_with_errors(&app_dir, &game_list, &device.id, &config, &sync_config, &error_games);

    Ok(())
}

fn write_game_list_local(app_dir: &StrictPath, game_list: &crate::sync::game_list::GameListFile) {
    let path = app_dir.joined("ludusavi-game-list.json");

    // Mergear con la copia local existente para no perder juegos custom locales
    // (juegos añadidos via "Add game" que aún no están en el cloud).
    let merged = if let Ok(path_buf) = path.as_std_path_buf() {
        if let Ok(existing_content) = std::fs::read_to_string(&path_buf) {
            if let Ok(existing) = serde_json::from_str::<crate::sync::game_list::GameListFile>(&existing_content) {
                let cloud_ids: std::collections::HashSet<String> = game_list.games.iter().map(|g| g.id.clone()).collect();

                // Conservar juegos locales que no están en el cloud
                let mut merged = game_list.clone();
                for local_game in &existing.games {
                    if !cloud_ids.contains(&local_game.id) {
                        merged.games.push(local_game.clone());
                    }
                }

                // Conservar device_names del local que no están en el cloud
                for (dev_id, dev_name) in &existing.device_names {
                    merged.device_names.entry(dev_id.clone()).or_insert_with(|| dev_name.clone());
                }

                merged
            } else {
                game_list.clone()
            }
        } else {
            game_list.clone()
        }
    } else {
        game_list.clone()
    };

    if let Ok(content) = serde_json::to_string_pretty(&merged) {
        if let Ok(path_buf) = path.as_std_path_buf() {
            let _ = std::fs::write(path_buf, content);
        }
    }
}

fn calculate_game_status(
    game: &crate::sync::game_list::GameMetaData,
    device_id: &str,
    config: &Config,
    sync_config: &crate::sync::sync_config::SyncGamesConfig,
) -> String {
    let mode = sync_config.get_mode(&game.name);

    if matches!(mode, crate::sync::sync_config::SaveMode::None) {
        return "not_managed".to_string();
    }

    let local_path = game.path_by_device.get(device_id);

    match mode {
        crate::sync::sync_config::SaveMode::Local => {
            // Comparar mtime del ZIP local con mtime de los saves
            let zip_path = config.backup.path.joined(&format!("{}.zip", game.id));
            let zip_std = match zip_path.as_std_path_buf() {
                Ok(p) => p,
                Err(_) => return "pending_backup".to_string(),
            };

            let zip_mtime = std::fs::metadata(&zip_std)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| -> chrono::DateTime<chrono::Utc> { t.into() });

            let Some(path_entry) = local_path else {
                return "pending_backup".to_string();
            };

            let scan = crate::sync::conflict::DirectoryScanResult::scan(Some(&path_entry.path));

            match (zip_mtime, scan.latest_write_time_utc) {
                (None, _) => "pending_backup".to_string(),
                (Some(_), None) => "pending_restore".to_string(),
                (Some(zip_t), Some(save_t)) => {
                    let zip_secs = zip_t.timestamp();
                    let save_secs = save_t.timestamp();
                    if save_secs > zip_secs {
                        "pending_backup".to_string()
                    } else if zip_secs > save_secs {
                        "pending_restore".to_string()
                    } else {
                        "synced".to_string()
                    }
                }
            }
        }
        _ => {
            // CLOUD y SYNC: usar determine_sync_type
            let scan = crate::sync::conflict::DirectoryScanResult::scan(
                local_path.map(|e| e.path.as_str())
            );
            let status = crate::sync::conflict::determine_sync_type(game, &scan, device_id);
            match status {
                crate::sync::conflict::SyncStatus::InSync => "synced".to_string(),
                crate::sync::conflict::SyncStatus::RequiresUpload => "pending_backup".to_string(),
                crate::sync::conflict::SyncStatus::RequiresDownload => "pending_restore".to_string(),
                crate::sync::conflict::SyncStatus::Unknown => "pending_backup".to_string(),
                crate::sync::conflict::SyncStatus::UnsetDirectory => "pending_backup".to_string(),
                crate::sync::conflict::SyncStatus::Conflict { .. } => "conflict".to_string(),
            }
        }
    }
}

fn write_sync_status_with_errors(
    app_dir: &StrictPath,
    game_list: &crate::sync::game_list::GameListFile,
    device_id: &str,
    config: &Config,
    sync_config: &crate::sync::sync_config::SyncGamesConfig,
    error_games: &std::collections::HashMap<String, ErrorInfo>,
) {
    let path = app_dir.joined("daemon-status.json");
    let mut map = serde_json::Map::new();

    // Incluir todos los juegos del game-list con path en este device
    for game in &game_list.games {
        if !game.path_by_device.contains_key(device_id) {
            continue;
        }
        let (status, error_category, error_direction, error_message) = if let Some(info) = error_games.get(&game.id) {
            (
                "error".to_string(),
                Some(info.category.as_str().to_string()),
                Some(info.direction.as_str().to_string()),
                Some(info.message.clone()),
            )
        } else {
            (calculate_game_status(game, device_id, config, sync_config), None, None, None)
        };
        let last_sync = game.last_sync_time_utc
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();
        let last_local = game.latest_write_time_utc
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();

        map.insert(game.id.clone(), serde_json::json!({
            "status": status,
            "last_sync_time": last_sync,
            "last_local_write": last_local,
            "error_category": error_category,
            "error_direction": error_direction,
            "error_message": error_message,
        }));
    }

    // Añadir juegos en error_games que no están en game_list (p.ej. LOCAL sin entrada en cloud)
    for (game_id, info) in error_games {
        if map.contains_key(game_id) {
            continue;
        }
        map.insert(game_id.clone(), serde_json::json!({
            "status": "error",
            "last_sync_time": "",
            "last_local_write": "",
            "error_category": info.category.as_str(),
            "error_direction": info.direction.as_str(),
            "error_message": info.message.clone(),
        }));
    }

    let json = serde_json::json!({ "games": map });
    if let Ok(content) = serde_json::to_string(&json) {
        if let Ok(path_buf) = path.as_std_path_buf() {
            let _ = std::fs::write(path_buf, content);
        }
    }
}

fn write_sync_status(
    app_dir: &StrictPath,
    game_list: &crate::sync::game_list::GameListFile,
    device_id: &str,
    config: &Config,
    sync_config: &crate::sync::sync_config::SyncGamesConfig,
) {
    let path = app_dir.joined("daemon-status.json");
    let mut map = serde_json::Map::new();

    for game in &game_list.games {
        if !game.path_by_device.contains_key(device_id) {
            continue;
        }
        let status = calculate_game_status(game, device_id, config, sync_config);
        let last_sync = game.last_sync_time_utc
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();
        let last_local = game.latest_write_time_utc
            .map(|t| t.to_rfc3339())
            .unwrap_or_default();

        map.insert(game.id.clone(), serde_json::json!({
            "status": status,
            "last_sync_time": last_sync,
            "last_local_write": last_local,
        }));
    }

    let json = serde_json::json!({ "games": map });
    if let Ok(content) = serde_json::to_string(&json) {
        if let Ok(path_buf) = path.as_std_path_buf() {
            let _ = std::fs::write(path_buf, content);
        }
    }
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn get_sync_games_mtime() -> Option<std::time::SystemTime> {
    let app_dir = crate::prelude::app_dir();
    let path = app_dir.joined("sync-games.json");
    path.as_std_path_buf().ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
}
