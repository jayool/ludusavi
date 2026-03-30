use crate::{
    prelude::app_dir,
    resource::config::Config,
    scan::ScanInfo,
    sync::{
        conflict::DirectoryScanResult,
        device::DeviceIdentity,
        game_list::GameMetaData,
        operations::{extract_root_from_scan, read_game_list_from_cloud, write_game_list_to_cloud},
    },
};

/// Registra o actualiza un juego en el game-list.json del cloud
/// después de un backup exitoso de Ludusavi.
/// Este es el puente entre la detección de Ludusavi y el sistema de EmuSync.
pub fn register_game_after_backup(config: &Config, scan_info: &ScanInfo) {
    if !config.apps.rclone.is_valid() {
        return;
    }
    if config.cloud.remote.is_none() {
        return;
    }

    let root_path = match extract_root_from_scan(&scan_info.found_files) {
        Some(path) => path,
        None => {
            log::debug!(
                "[{}] No root path found, skipping game-list registration",
                scan_info.game_name
            );
            return;
        }
    };

    // Escaneamos el directorio local para obtener los metadatos reales
    let scan_result = DirectoryScanResult::scan(Some(&root_path));

    log::info!(
        "[{}] Registering game with root path: {} (latest_write: {:?}, bytes: {})",
        scan_info.game_name,
        root_path,
        scan_result.latest_write_time_utc,
        scan_result.storage_bytes,
    );

    let app_dir = app_dir();
    let device = DeviceIdentity::load_or_create(&app_dir);

    let mut game_list = read_game_list_from_cloud(config).unwrap_or_default();
    let game_id = scan_info.game_name.clone();

    match game_list.get_game_mut(&game_id) {
        Some(existing) => {
            // Siempre actualizamos la ruta y los metadatos en cada backup
            existing.path_by_device.insert(device.id.clone(), root_path);
            existing.latest_write_time_utc = scan_result.latest_write_time_utc;
            existing.storage_bytes = scan_result.storage_bytes;

            log::info!(
                "[{}] Updated game metadata for device {}",
                scan_info.game_name,
                device.name,
            );
        }
        None => {
            log::info!(
                "[{}] Adding new game to game-list for device {}",
                scan_info.game_name,
                device.name,
            );

            let mut game = GameMetaData::new(game_id.clone(), scan_info.game_name.clone());
            game.path_by_device.insert(device.id.clone(), root_path);
            game.latest_write_time_utc = scan_result.latest_write_time_utc;
            game.storage_bytes = scan_result.storage_bytes;
            game_list.upsert_game(game);
        }
    }

    if let Err(e) = write_game_list_to_cloud(config, &game_list) {
        log::error!("[{}] Failed to write game-list: {}", scan_info.game_name, e);
    }
}
