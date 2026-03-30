use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use crate::{
    prelude::{app_dir, StrictPath},
    resource::{
        config::Config,
        ResourceFile,
    },
    sync::{
        conflict::{determine_sync_type, DirectoryScanResult, SyncStatus},
        device::DeviceIdentity,
        game_list::{GameListFile, GameMetaData},
        operations::{download_game, read_game_list_from_cloud, upload_game, write_game_list_to_cloud},
    },
};

/// Configuración del daemon.
pub struct DaemonConfig {
    /// Cada cuántos segundos se comprueba si hay cambios.
    /// Equivalente a LoopDelayTimeSpan en EmuSync.
    pub check_interval_secs: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            check_interval_secs: 120, // 2 minutos, igual que EmuSync por defecto
        }
    }
}

/// Ejecuta el daemon de sincronización en un hilo separado.
/// Equivalente a GameSyncWorker.ExecuteAsync en EmuSync.
///
/// Devuelve un flag que se puede usar para detener el daemon:
/// ponlo a `true` para que el hilo termine limpiamente.
pub fn start_daemon(
    stop_flag: Arc<AtomicBool>,
    daemon_config: DaemonConfig,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        log::info!("[sync daemon] Starting");

        // Pequeño delay inicial igual que EmuSync (30 segundos)
        // para dar tiempo al sistema a arrancar
        std::thread::sleep(Duration::from_secs(30));

        loop {
            if stop_flag.load(Ordering::Relaxed) {
                log::info!("[sync daemon] Stop requested, shutting down");
                break;
            }

            log::debug!("[sync daemon] Running sync check");

            if let Err(e) = run_sync_cycle() {
                log::error!("[sync daemon] Error during sync cycle: {e}");
            }

            // Espera el intervalo configurado, comprobando el stop flag
            // cada segundo para poder parar rápido si se solicita
            for _ in 0..daemon_config.check_interval_secs {
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }

        log::info!("[sync daemon] Stopped");
    })
}

/// Un ciclo completo de sincronización: lee la config, lee el game list,
/// y para cada juego decide si hay que subir o bajar.
/// Equivalente a TryDetectGameChangesAsync en EmuSync.
fn run_sync_cycle() -> Result<(), String> {
    // Carga la config de Ludusavi
    let config = Config::load().map_err(|e| format!("Failed to load config: {e:?}"))?;

    // Comprueba que rclone está configurado
    if !config.apps.rclone.is_valid() {
        log::warn!("[sync daemon] Rclone is not configured, skipping sync");
        return Ok(());
    }

    if config.cloud.remote.is_none() {
        log::warn!("[sync daemon] No cloud remote configured, skipping sync");
        return Ok(());
    }

    let app_dir = app_dir();
    let device = DeviceIdentity::load_or_create(&app_dir);

    log::debug!("[sync daemon] Running as device: {} ({})", device.name, device.id);

    // Lee el game list del cloud
    let mut game_list = read_game_list_from_cloud(&config).unwrap_or_default();

    let mut any_changes = false;

    // Para cada juego en el game list, decide qué hacer
    // Solo procesamos juegos que tienen una ruta configurada para este dispositivo
    let game_ids: Vec<String> = game_list
        .games
        .iter()
        .filter(|g| g.path_by_device.contains_key(&device.id))
        .map(|g| g.id.clone())
        .collect();

    for game_id in game_ids {
        if let Some(game) = game_list.get_game_mut(&game_id) {
            match process_game(&config, &app_dir, &device, game) {
                Ok(changed) => {
                    if changed {
                        any_changes = true;
                    }
                }
                Err(e) => {
                    log::error!("[sync daemon] Error processing game {}: {e}", game.name);
                }
            }
        }
    }

    // Si hubo cambios, actualiza el game list en el cloud
    if any_changes {
        log::info!("[sync daemon] Uploading updated game list to cloud");
        write_game_list_to_cloud(&config, &game_list)
            .map_err(|e| format!("Failed to write game list: {e}"))?;
    }

    Ok(())
}

/// Procesa un juego: determina si hay que subir o bajar y lo hace.
/// Devuelve true si se hizo algún cambio (para saber si hay que
/// actualizar el game list en el cloud).
/// Equivalente a DetectChangesForGameAsync en EmuSync.
fn process_game(
    config: &Config,
    app_dir: &StrictPath,
    device: &DeviceIdentity,
    game: &mut GameMetaData,
) -> Result<bool, String> {
    let local_path = game.path_by_device.get(&device.id).cloned();
    let scan = DirectoryScanResult::scan(local_path.as_deref());
    let status = determine_sync_type(game, &scan);

    log::debug!(
        "[sync daemon] Game '{}' status: {:?}",
        game.name,
        status
    );

    match status {
        SyncStatus::RequiresUpload => {
            log::info!("[sync daemon] Uploading game: {}", game.name);
            upload_game(config, app_dir, device, game)
                .map_err(|e| format!("Upload failed for {}: {e}", game.name))?;
            Ok(true)
        }
        SyncStatus::RequiresDownload => {
            log::info!("[sync daemon] Downloading game: {}", game.name);
            download_game(config, app_dir, device, game)
                .map_err(|e| format!("Download failed for {}: {e}", game.name))?;
            Ok(true)
        }
        SyncStatus::InSync => {
            log::debug!("[sync daemon] Game '{}' is in sync, nothing to do", game.name);
            Ok(false)
        }
        SyncStatus::Unknown | SyncStatus::UnsetDirectory => {
            log::debug!(
                "[sync daemon] Game '{}' has no actionable sync status: {:?}",
                game.name,
                status
            );
            Ok(false)
        }
    }
}
