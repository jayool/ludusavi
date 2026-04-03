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
            download_game, extract_root_from_scan, read_game_list_from_cloud, upload_game, write_game_list_to_cloud,
        },
    },
};

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

    if !config.apps.rclone.is_valid() {
        log::warn!("[sync daemon] Rclone is not configured, cannot start");
        return Ok(());
    }

    if config.cloud.remote.is_none() {
        log::warn!("[sync daemon] No cloud remote configured, cannot start");
        return Ok(());
    }

    let app_dir = app_dir();
    let device = DeviceIdentity::load_or_create(&app_dir);

    log::info!("[sync daemon] Running as device: {} ({})", device.name, device.id);

    // Paso 2: auto-registrar rutas para juegos sin ruta en este dispositivo
    log::info!("[sync daemon] Auto-registering paths for unregistered games...");
    if let Err(e) = auto_register_paths(&config, &device) {
        log::error!("[sync daemon] Error during path auto-registration: {e}");
    }

    if stop_flag.load(Ordering::Relaxed) {
        return Ok(());
    }

    if stop_flag.load(Ordering::Relaxed) {
        return Ok(());
    }

    // Paso 3: leer el game list para saber qué rutas vigilar
    let game_list = read_game_list_from_cloud(&config).unwrap_or_default();

    let watched_paths: HashMap<String, String> = game_list
        .games
        .iter()
        .filter_map(|g| {
            g.path_by_device
                .get(&device.id)
                .map(|path| (g.id.clone(), path.clone()))
        })
        .collect();

    if watched_paths.is_empty() {
        log::info!("[sync daemon] No games registered for this device, will retry in 30s...");
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

    while !stop_flag.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_secs(1));
        poll_counter += 1;

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

        let mut game_list = read_game_list_from_cloud(&config).unwrap_or_default();
        let mut any_changes = false;

        for game_id in &ready_games {
            if let Some(game) = game_list.get_game_mut(game_id) {
                let local_path = game.path_by_device.get(&device.id).cloned();
                let scan = DirectoryScanResult::scan(local_path.as_deref());
                let status = determine_sync_type(game, &scan);
                if status != SyncStatus::RequiresUpload {
                    log::info!("[sync daemon] Skipping upload for {} — already in sync", game.name);
                    debounce_state.lock().unwrap().remove(game_id);
                    continue;
                }
                log::info!("[sync daemon] Uploading: {}", game.name);
                match upload_game(&config, &app_dir, &device, game) {
                    Ok(_) => {
                        log::info!("[sync daemon] Upload complete: {}", game.name);
                        any_changes = true;
                    }
                    Err(e) => {
                        log::error!("[sync daemon] Upload failed for {}: {e}", game.name);
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
                // Actualizar mod time para que el polling no redetecte nuestro propio upload
                last_known_mod_time = crate::sync::operations::get_game_list_mod_time(&config);
                save_last_mod_time(&app_dir, &last_known_mod_time);
            }
        }
    }

    log::info!("[sync daemon] Stop flag set, shutting down watcher");
    Ok(())
}
fn load_last_mod_time(app_dir: &StrictPath) -> Option<String> {
    let path = app_dir.joined("daemon-state.json");
    let content = path.read()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    json.get("last_known_mod_time")?.as_str().map(|s| s.to_string())
}

fn save_last_mod_time(app_dir: &StrictPath, mod_time: &Option<String>) {
    let path = app_dir.joined("daemon-state.json");
    let json = serde_json::json!({ "last_known_mod_time": mod_time });
    if let Ok(content) = serde_json::to_string(&json) {
        let _ = std::fs::write(path.as_std_path_buf().unwrap(), content);
    }
}

/// Para cada juego en el game-list sin ruta registrada para este dispositivo,
/// usa el manifiesto de Ludusavi para encontrar los saves automáticamente.
fn auto_register_paths(config: &Config, device: &DeviceIdentity) -> Result<(), String> {
    let mut game_list = match read_game_list_from_cloud(config) {
        Some(gl) => gl,
        None => return Ok(()),
    };

    let unregistered: Vec<String> = game_list
        .games
        .iter()
        .filter(|g| !g.path_by_device.contains_key(&device.id))
        .map(|g| g.id.clone())
        .collect();

    if unregistered.is_empty() {
        log::debug!("[sync daemon] All games already have a path for this device");
        return Ok(());
    }

    log::info!(
        "[sync daemon] Auto-registering {} game(s) for this device",
        unregistered.len()
    );

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

    let mut any_changes = false;

    for game_id in &unregistered {
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
                    game.path_by_device.insert(device.id.clone(), root_path);
                    any_changes = true;
                }
            }
            None => {
                // No hay saves locales — intentar resolver la ruta esperada via manifiesto
                match crate::sync::operations::resolve_expected_save_path(config, game_entry) {
                    Some(expected_path) => {
                        log::info!(
                            "[sync daemon] Auto-registered expected path for {}: {}",
                            game_id,
                            expected_path
                        );
                        if let Some(game) = game_list.get_game_mut(game_id) {
                            game.path_by_device.insert(device.id.clone(), expected_path);
                            any_changes = true;
                        }
                    }
                    None => {
                        log::debug!("[sync daemon] Cannot resolve expected path for {}, skipping", game_id);
                    }
                }
            }
        }
    }

    if any_changes {
        write_game_list_to_cloud(config, &game_list).map_err(|e| format!("Failed to write game list: {e}"))?;
        log::info!("[sync daemon] Updated game list with auto-registered paths");
    }

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

    for game_id in game_ids {
        if let Some(game) = game_list.get_game_mut(&game_id) {
            let local_path = game.path_by_device.get(&device.id).cloned();
            let scan = DirectoryScanResult::scan(local_path.as_deref());
            log::info!(
                "[sync daemon] [{}] scan_latest={:?} game_latest={:?} directory_exists={}",
                game.name,
                scan.latest_write_time_utc,
                game.latest_write_time_utc,
                scan.directory_exists
            );
            let status = determine_sync_type(game, &scan);
            log::info!("[sync daemon] [{}] status={:?}", game.name, status);

            match status {
                SyncStatus::RequiresDownload => {
                    log::info!("[sync daemon] Downloading on startup: {}", game.name);
                    match download_game(config, app_dir, device, game) {
                        Ok(_) => {
                            log::info!("[sync daemon] Download complete: {}", game.name);
                            any_changes = true;
                        }
                        Err(e) => {
                            log::error!("[sync daemon] Download failed for {}: {e}", game.name);
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
                        }
                    }
                }
                SyncStatus::InSync => {
                    log::debug!("[sync daemon] {} is in sync", game.name);
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
    let synced_games: std::collections::HashSet<String> = game_list
        .games
        .iter()
        .filter(|g| g.path_by_device.contains_key(&device.id))
        .map(|g| g.id.clone())
        .collect();
    write_sync_status(app_dir, &synced_games);

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

    for game_id in game_ids {
        if let Some(game) = game_list.get_game_mut(&game_id) {
            let local_path = game.path_by_device.get(&device.id).cloned();
            let scan = DirectoryScanResult::scan(local_path.as_deref());
            let status = determine_sync_type(game, &scan);

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
                    }
                }
            }
        }
    }

    if any_changes {
        write_game_list_to_cloud(config, &game_list).map_err(|e| format!("Failed to write game list: {e}"))?;
    }

    // Escribir estado de sync al disco para que la GUI lo pueda leer
    let synced_games: std::collections::HashSet<String> = game_list
        .games
        .iter()
        .filter(|g| g.path_by_device.contains_key(&device.id))
        .map(|g| g.id.clone())
        .collect();
    write_sync_status(app_dir, &synced_games);

    Ok(())
}

fn write_sync_status(app_dir: &StrictPath, synced_games: &std::collections::HashSet<String>) {
    let path = app_dir.joined("daemon-status.json");
    let map: serde_json::Map<String, serde_json::Value> = synced_games
        .iter()
        .map(|id| (id.clone(), serde_json::Value::String("synced".to_string())))
        .collect();
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
