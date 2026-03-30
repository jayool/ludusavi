use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult};

use crate::{
    prelude::{app_dir, StrictPath},
    resource::config::Config,
    sync::{
        conflict::{determine_sync_type, DirectoryScanResult, SyncStatus},
        device::DeviceIdentity,
        operations::{download_game, read_game_list_from_cloud, upload_game, write_game_list_to_cloud},
    },
};

/// Igual que Syncthing:
/// - notifyDelay = 10s (tiempo de silencio antes de actuar)
/// - notifyTimeout = 60s (tiempo máximo antes de actuar aunque sigan llegando cambios)
const NOTIFY_DELAY: Duration = Duration::from_secs(10);
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(60);

/// Estado de debounce para un juego concreto.
/// Replica el comportamiento del aggregator de Syncthing.
struct GameDebounceState {
    /// Momento del primer evento (para notifyTimeout)
    first_event: Instant,
    /// Momento del último evento (para notifyDelay)
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

    /// Devuelve true si el juego debe procesarse ahora.
    /// Replica isOld() del aggregator de Syncthing:
    /// - Han pasado >10s desde el último evento (silencio)
    /// - O han pasado >60s desde el primer evento (timeout máximo)
    fn should_fire(&self) -> bool {
        let now = Instant::now();
        now.duration_since(self.last_event) > NOTIFY_DELAY
            || now.duration_since(self.first_event) > NOTIFY_TIMEOUT
    }
}

/// Configuración del daemon.
pub struct DaemonConfig;

impl Default for DaemonConfig {
    fn default() -> Self {
        Self
    }
}

/// Ejecuta el daemon de sincronización en un hilo separado.
pub fn start_daemon(
    stop_flag: Arc<AtomicBool>,
    _daemon_config: DaemonConfig,
) -> std::thread::JoinHandle<()> {
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

    // --- Paso 1: comprobación inmediata de descargas al arrancar ---
    log::info!("[sync daemon] Checking cloud for downloads on startup...");
    if let Err(e) = check_downloads(&config, &app_dir, &device) {
        log::error!("[sync daemon] Error during startup download check: {e}");
    }

    if stop_flag.load(Ordering::Relaxed) {
        return Ok(());
    }

    // --- Paso 2: leer el game list para saber qué rutas vigilar ---
    let game_list = read_game_list_from_cloud(&config).unwrap_or_default();

    // Mapa game_id -> ruta local (solo para este dispositivo)
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
        log::info!("[sync daemon] No games registered for this device, watching for new registrations...");
        // Sin juegos registrados, simplemente esperamos a que stop_flag se active.
        // En el futuro se podría añadir un mecanismo para recargar el game list.
        while !stop_flag.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(5));
        }
        return Ok(());
    }

    log::info!("[sync daemon] Watching {} game(s) for changes", watched_paths.len());

    // --- Paso 3: estado de debounce compartido entre el watcher y el worker ---
    // Clave: game_id, Valor: GameDebounceState
    let debounce_state: Arc<Mutex<HashMap<String, GameDebounceState>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Mapa inverso: ruta -> game_id (para el callback del watcher)
    // Normalizamos las rutas para comparación
    let path_to_game: Arc<HashMap<String, String>> = Arc::new(
        watched_paths
            .iter()
            .map(|(game_id, path)| {
                let normalized = normalize_path(path);
                (normalized, game_id.clone())
            })
            .collect(),
    );

    // --- Paso 4: arrancar el file watcher con debounce ---
    let debounce_state_watcher = debounce_state.clone();
    let path_to_game_watcher = path_to_game.clone();

    // El debouncer de notify-debouncer-full ya hace parte del debounce a nivel
    // de evento individual, pero nosotros implementamos el debounce por juego
    // manualmente (igual que el aggregator de Syncthing) para tener el
    // notifyDelay y notifyTimeout correctos.
    let mut debouncer = new_debouncer(
        // Usamos 1s como ventana del debouncer interno, el debounce real
        // lo hacemos nosotros con NOTIFY_DELAY/NOTIFY_TIMEOUT
        Duration::from_secs(1),
        None,
        move |result: DebounceEventResult| {
            match result {
                Ok(events) => {
                    let mut state = debounce_state_watcher.lock().unwrap();
                    let mut dirty_games = HashSet::new();

                    for event in events {
                        for path in &event.paths {
                            let path_str = normalize_path(&path.to_string_lossy());
                            // Buscar a qué juego pertenece este path
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
            }
        },
    )
    .map_err(|e| format!("Failed to create file watcher: {e}"))?;

    // Registrar cada ruta en el watcher
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

    // --- Paso 5: worker loop principal ---
    // Comprueba cada segundo si hay juegos listos para subir (según el debounce)
    log::info!("[sync daemon] File watcher active, monitoring for changes");

    while !stop_flag.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_secs(1));

        // Recoger juegos listos para subir
        let ready_games: Vec<String> = {
            let state = debounce_state.lock().unwrap();
            state
                .iter()
                .filter(|(_, s)| s.should_fire())
                .map(|(id, _)| id.clone())
                .collect()
        };

        if ready_games.is_empty() {
            continue;
        }

        log::info!("[sync daemon] Processing {} game(s) ready for upload", ready_games.len());

        // Cargar config y game list frescos para cada ciclo de upload
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

            // Eliminar del estado de debounce independientemente del resultado
            debounce_state.lock().unwrap().remove(game_id);
        }

        if any_changes {
            if let Err(e) = write_game_list_to_cloud(&config, &game_list) {
                log::error!("[sync daemon] Failed to write game list: {e}");
            }
        }
    }

    log::info!("[sync daemon] Stop flag set, shutting down watcher");
    Ok(())
}

/// Comprueba si hay juegos en el cloud más nuevos que la versión local y los descarga.
/// Se llama una vez al arrancar el daemon.
fn check_downloads(
    config: &Config,
    app_dir: &StrictPath,
    device: &DeviceIdentity,
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
                    log::debug!("[sync daemon] {} requires upload, watcher will handle it", game.name);
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
        write_game_list_to_cloud(config, &game_list)
            .map_err(|e| format!("Failed to write game list: {e}"))?;
    }

    Ok(())
}

/// Normaliza una ruta para comparación consistente entre plataformas.
fn normalize_path(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}
