use crate::{
    prelude::{app_dir, StrictPath},
    resource::config::Config,
    scan::{ScanInfo, ScannedFile},
    sync::{
        device::DeviceIdentity,
        game_list::{GameListFile, GameMetaData},
        operations::{extract_root_from_scan, read_game_list_from_cloud, write_game_list_to_cloud},
    },
};

/// Registra o actualiza un juego en el game-list.json del cloud
/// después de un backup exitoso de Ludusavi.
/// Este es el puente entre la detección de Ludusavi y el sistema de EmuSync.
/// Equivalente al flujo de SyncSourceManager.SetLocalStorageProviderAsync en EmuSync.
pub fn register_game_after_backup(config: &Config, scan_info: &ScanInfo) {
    // Si no hay cloud configurado, no hacemos nada
    if !config.apps.rclone.is_valid() {
        return;
    }
    if config.cloud.remote.is_none() {
        return;
    }

    // Calculamos la carpeta raíz común de los ficheros encontrados
    // Traducción de GetMostCommonFolder en EmuSync
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

    log::info!(
        "[{}] Registering game with root path: {}",
        scan_info.game_name,
        root_path
    );

    let app_dir = app_dir();
    let device = DeviceIdentity::load_or_create(&app_dir);

    // Lee el game-list actual del cloud, o crea uno nuevo si no existe
    let mut game_list = read_game_list_from_cloud(config).unwrap_or_default();

    // Busca si el juego ya existe en el game-list
    let game_id = scan_info.game_name.clone();

    match game_list.get_game_mut(&game_id) {
        Some(existing) => {
            // El juego ya existe, solo actualizamos la ruta de este dispositivo
            // si ha cambiado
            let current_path = existing.path_by_device.get(&device.id).cloned();
            if current_path.as_deref() != Some(&root_path) {
                log::info!(
                    "[{}] Updating path for device {}: {} -> {}",
                    scan_info.game_name,
                    device.name,
                    current_path.unwrap_or_default(),
                    root_path
                );
                existing.path_by_device.insert(device.id.clone(), root_path);

                if let Err(e) = write_game_list_to_cloud(config, &game_list) {
                    log::error!("[{}] Failed to update game-list: {}", scan_info.game_name, e);
                }
            }
        }
        None => {
            // El juego no existe, lo añadimos
            log::info!(
                "[{}] Adding new game to game-list for device {}",
                scan_info.game_name,
                device.name
            );

            let mut game = GameMetaData::new(game_id.clone(), scan_info.game_name.clone());
            game.path_by_device.insert(device.id.clone(), root_path);
            game_list.upsert_game(game);

            if let Err(e) = write_game_list_to_cloud(config, &game_list) {
                log::error!("[{}] Failed to write game-list: {}", scan_info.game_name, e);
            }
        }
    }
}
