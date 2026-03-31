use std::io::{Read, Write};
use chrono::{DateTime, Utc};
use crate::resource::manifest::Game;
use crate::{
    prelude::StrictPath,
    resource::config::Config,
    sync::{
        conflict::DirectoryScanResult,
        game_list::{game_zip_file_name, GameListFile, GameMetaData, GAME_LIST_FILE_NAME},
        device::DeviceIdentity,
    },
};

#[derive(Debug)]
pub enum SyncError {
    NoLocalPath,
    NoRcloneConfig,
    ZipError(String),
    RcloneError(String),
    IoError(String),
    NoZipInCloud,
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoLocalPath => write!(f, "No local path configured for this device"),
            Self::NoRcloneConfig => write!(f, "Rclone is not configured"),
            Self::ZipError(e) => write!(f, "Zip error: {e}"),
            Self::RcloneError(e) => write!(f, "Rclone error: {e}"),
            Self::IoError(e) => write!(f, "IO error: {e}"),
            Self::NoZipInCloud => write!(f, "No zip found in cloud for this game"),
        }
    }
}

/// Directorio temporal para zips durante la sincronización.
fn temp_zip_dir(app_dir: &StrictPath) -> StrictPath {
    app_dir.joined("sync-temp-zips")
}

/// Crea un zip de todos los ficheros en `folder_path` y lo escribe en `zip_path`.
pub fn create_zip_from_folder(
    folder_path: &str,
    zip_path: &StrictPath,
) -> Result<(), SyncError> {
    let folder = std::path::Path::new(folder_path);

    if !folder.is_dir() {
        return Err(SyncError::IoError(format!(
            "Folder does not exist: {folder_path}"
        )));
    }

    if let Err(e) = zip_path.create_parent_dir() {
        return Err(SyncError::IoError(e.to_string()));
    }

    let file = std::fs::File::create(
        zip_path.as_std_path_buf().map_err(|e| SyncError::IoError(e.to_string()))?,
    )
    .map_err(|e| SyncError::IoError(e.to_string()))?;

    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);

    for entry in walkdir::WalkDir::new(folder)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let relative = path
            .strip_prefix(folder)
            .map_err(|e| SyncError::ZipError(e.to_string()))?;
        let zip_entry_name = relative.to_string_lossy().replace('\\', "/");

        zip.start_file(&zip_entry_name, options)
            .map_err(|e| SyncError::ZipError(e.to_string()))?;

        let mut f = std::fs::File::open(path)
            .map_err(|e| SyncError::IoError(e.to_string()))?;

        let mut buffer = [0u8; 65536];
        loop {
            let n = f.read(&mut buffer).map_err(|e| SyncError::IoError(e.to_string()))?;
            if n == 0 {
                break;
            }
            zip.write_all(&buffer[..n])
                .map_err(|e| SyncError::ZipError(e.to_string()))?;
        }
    }

    zip.finish().map_err(|e| SyncError::ZipError(e.to_string()))?;
    Ok(())
}

/// Extrae un zip en `output_directory`, forzando el timestamp dado si se proporciona.
pub fn extract_zip_to_directory(
    zip_path: &StrictPath,
    output_directory: &str,
    force_last_write_time: Option<DateTime<Utc>>,
) -> Result<(), SyncError> {
    let output = std::path::Path::new(output_directory);

    if output.exists() {
        std::fs::remove_dir_all(output)
            .map_err(|e| SyncError::IoError(e.to_string()))?;
    }
    std::fs::create_dir_all(output)
        .map_err(|e| SyncError::IoError(e.to_string()))?;

    let file = zip_path.open().map_err(|e| SyncError::IoError(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| SyncError::ZipError(e.to_string()))?;

    for i in 0..archive.len() {
        let mut zip_file = archive
            .by_index(i)
            .map_err(|e| SyncError::ZipError(e.to_string()))?;

        if zip_file.name().ends_with('/') {
            continue;
        }

        let out_path = output.join(zip_file.name().replace('\\', "/"));

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| SyncError::IoError(e.to_string()))?;
        }

        let mut out_file = std::fs::File::create(&out_path)
            .map_err(|e| SyncError::IoError(e.to_string()))?;

        let mut buffer = [0u8; 65536];
        loop {
            let n = zip_file.read(&mut buffer).map_err(|e| SyncError::IoError(e.to_string()))?;
            if n == 0 {
                break;
            }
            out_file.write_all(&buffer[..n])
                .map_err(|e| SyncError::IoError(e.to_string()))?;
        }

        if let Some(ts) = force_last_write_time {
            let system_time: std::time::SystemTime = ts.into();
            let _ = filetime::set_file_mtime(
                &out_path,
                filetime::FileTime::from_system_time(system_time),
            );
        }
    }

    if let Some(ts) = force_last_write_time {
        let system_time: std::time::SystemTime = ts.into();
        let _ = filetime::set_file_mtime(
            output,
            filetime::FileTime::from_system_time(system_time),
        );
    }

    Ok(())
}

/// Lee el game-list.json del cloud usando rclone.
pub fn read_game_list_from_cloud(config: &Config) -> Option<GameListFile> {
    let rclone = make_rclone(config)?;
    let cloud_path = &config.cloud.path;
    let remote_file = format!("{}/{}", cloud_path, GAME_LIST_FILE_NAME);

    let temp_path = std::env::temp_dir().join("ludusavi-game-list-temp.json");
    let temp_strict = StrictPath::from(temp_path.clone());

    let args = vec![
        "copyto".to_string(),
        rclone.path(&remote_file),
        temp_path.to_string_lossy().to_string(),
    ];

    match crate::prelude::run_command(
        config.apps.rclone.path.raw(),
        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        &[0],
        crate::prelude::Privacy::Public,
    ) {
        Ok(_) => {
            if let Some(content) = temp_strict.read() {
                let _ = std::fs::remove_file(&temp_path);
                serde_json::from_str(&content).ok()
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

/// Sube el game-list.json al cloud usando rclone.
pub fn write_game_list_to_cloud(
    config: &Config,
    game_list: &GameListFile,
) -> Result<(), SyncError> {
    let rclone = make_rclone(config).ok_or(SyncError::NoRcloneConfig)?;
    let cloud_path = &config.cloud.path;

    let json = serde_json::to_string_pretty(game_list)
        .map_err(|e| SyncError::IoError(e.to_string()))?;

    let temp_path = std::env::temp_dir().join("ludusavi-game-list-temp.json");
    std::fs::write(&temp_path, &json)
        .map_err(|e| SyncError::IoError(e.to_string()))?;

    let remote_file = format!("{}/{}", cloud_path, GAME_LIST_FILE_NAME);
    let args = vec![
        "copyto".to_string(),
        temp_path.to_string_lossy().to_string(),
        rclone.path(&remote_file),
    ];

    crate::prelude::run_command(
        config.apps.rclone.path.raw(),
        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        &[0],
        crate::prelude::Privacy::Public,
    )
    .map_err(|e| SyncError::RcloneError(e.command()))?;

    let _ = std::fs::remove_file(&temp_path);
    Ok(())
}

/// Sube el zip de un juego al cloud.
pub fn upload_game(
    config: &Config,
    app_dir: &StrictPath,
    device: &DeviceIdentity,
    game: &mut GameMetaData,
) -> Result<(), SyncError> {
    let local_path = game
        .path_by_device
        .get(&device.id)
        .ok_or(SyncError::NoLocalPath)?
        .clone();

    let rclone = make_rclone(config).ok_or(SyncError::NoRcloneConfig)?;

    let scan = DirectoryScanResult::scan(Some(&local_path));

    let temp_dir = temp_zip_dir(app_dir);
    let zip_name = format!("{}.zip", game.id);
    let zip_path = temp_dir.joined(&zip_name);

    log::info!("[{}] Creating zip from {}", game.name, local_path);
    create_zip_from_folder(&local_path, &zip_path)?;

    let cloud_path = &config.cloud.path;
    let remote_file = format!("{}/{}", cloud_path, game_zip_file_name(&game.id));

    log::info!("[{}] Uploading zip to cloud: {}", game.name, remote_file);

    let zip_path_str = zip_path
        .as_std_path_buf()
        .map_err(|e| SyncError::IoError(e.to_string()))?
        .to_string_lossy()
        .to_string();

    let args = vec![
        "copyto".to_string(),
        zip_path_str,
        rclone.path(&remote_file),
    ];

    crate::prelude::run_command(
        config.apps.rclone.path.raw(),
        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        &[0],
        crate::prelude::Privacy::Public,
    )
    .map_err(|e| SyncError::RcloneError(e.command()))?;

    game.last_synced_from = Some(device.id.clone());
    game.last_sync_time_utc = Some(Utc::now());
    game.latest_write_time_utc = scan.latest_write_time_utc;
    game.storage_bytes = scan.storage_bytes;

    let _ = zip_path.remove();

    log::info!("[{}] Upload complete", game.name);
    Ok(())
}

/// Descarga el zip de un juego del cloud y lo extrae.
pub fn download_game(
    config: &Config,
    app_dir: &StrictPath,
    device: &DeviceIdentity,
    game: &GameMetaData,
) -> Result<(), SyncError> {
    let local_path = game
        .path_by_device
        .get(&device.id)
        .ok_or(SyncError::NoLocalPath)?
        .clone();

    let rclone = make_rclone(config).ok_or(SyncError::NoRcloneConfig)?;

    let cloud_path = &config.cloud.path;
    let remote_file = format!("{}/{}", cloud_path, game_zip_file_name(&game.id));

    let temp_dir = temp_zip_dir(app_dir);
    if let Err(e) = temp_dir.create_dirs() {
        return Err(SyncError::IoError(e.to_string()));
    }
    let zip_name = format!("{}.zip", game.id);
    let zip_path = temp_dir.joined(&zip_name);

    let zip_path_str = zip_path
        .as_std_path_buf()
        .map_err(|e| SyncError::IoError(e.to_string()))?
        .to_string_lossy()
        .to_string();

    log::info!("[{}] Downloading zip from cloud: {}", game.name, remote_file);

    let args = vec![
        "copyto".to_string(),
        rclone.path(&remote_file),
        zip_path_str,
    ];

    crate::prelude::run_command(
        config.apps.rclone.path.raw(),
        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        &[0],
        crate::prelude::Privacy::Public,
    )
    .map_err(|e| SyncError::RcloneError(e.command()))?;

    if !zip_path.is_file() {
        return Err(SyncError::NoZipInCloud);
    }

    log::info!("[{}] Extracting zip to {}", game.name, local_path);
    extract_zip_to_directory(&zip_path, &local_path, game.latest_write_time_utc)?;

    let _ = zip_path.remove();

    log::info!("[{}] Download complete", game.name);
    Ok(())
}

/// Calcula la carpeta raíz común de una lista de rutas de ficheros.
pub fn get_common_root_folder(paths: &[&str]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }

    let split_paths: Vec<Vec<String>> = paths
        .iter()
        .map(|p| {
            std::path::Path::new(p)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect()
        })
        .collect();

    let first = &split_paths[0];
    let mut common_length = first.len();

    for i in 0..first.len() {
        let segment = &first[i];
        let mismatch = split_paths.iter().any(|sp| {
            sp.len() <= i
                || !sp[i].eq_ignore_ascii_case(segment)
        });
        if mismatch {
            common_length = i;
            break;
        }
    }

    if common_length == 0 {
        return None;
    }

    let common: std::path::PathBuf = first[..common_length].iter().collect();
    Some(common.to_string_lossy().to_string())
}

/// Extrae la carpeta raíz común de los ficheros encontrados por Ludusavi.
pub fn extract_root_from_scan(found_files: &std::collections::HashMap<crate::prelude::StrictPath, crate::scan::ScannedFile>) -> Option<String> {
    if found_files.is_empty() {
        return None;
    }

    let paths: Vec<String> = found_files
        .iter()
        .filter(|(_, file)| !file.ignored)
        .filter_map(|(path, _)| path.interpret().ok())
        .collect();

    if paths.is_empty() {
        return None;
    }

    if paths.len() == 1 {
        return std::path::Path::new(&paths[0])
            .parent()
            .map(|p| p.to_string_lossy().to_string());
    }

    let dirs: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            std::path::Path::new(p)
                .parent()
                .map(|d| d.to_string_lossy().to_string())
        })
        .collect();

    let dir_refs: Vec<&str> = dirs.iter().map(|s| s.as_str()).collect();
    get_common_root_folder(&dir_refs)
}

/// Helper interno para construir el path remoto de rclone.
struct RcloneHelper {
    remote_id: String,
}

impl RcloneHelper {
    fn path(&self, path: &str) -> String {
        format!("{}:{}", self.remote_id, path.replace('\\', "/"))
    }
}

fn make_rclone(config: &Config) -> Option<RcloneHelper> {
    if !config.apps.rclone.is_valid() {
        return None;
    }
    let remote = config.cloud.remote.as_ref()?;
    Some(RcloneHelper {
        remote_id: remote.id().to_string(),
    })
}

/// Resuelve la ruta esperada de saves de un juego aunque no existan ficheros todavía.
/// Soporta rutas nativas Windows, rutas Proton en Linux y rutas XDG en Linux.
pub fn resolve_expected_save_path(_config: &Config, game: &Game) -> Option<String> {
    use crate::path::CommonPath;
    use crate::resource::manifest::placeholder as p;

    let home = CommonPath::Home.get()?;

    // --- Windows: rutas nativas ---
    #[cfg(target_os = "windows")]
    {
        for (raw_path, _) in &game.files {
            if raw_path.trim().is_empty() {
                continue;
            }

            if !raw_path.contains(p::WIN_LOCAL_APP_DATA_LOW)
                && !raw_path.contains(p::WIN_APP_DATA)
                && !raw_path.contains(p::WIN_LOCAL_APP_DATA)
                && !raw_path.contains(p::WIN_DOCUMENTS)
                && !raw_path.contains(p::HOME)
            {
                continue;
            }

            let data_local_low = CommonPath::DataLocalLow.get().unwrap_or(home);
            let data_roaming = CommonPath::Data.get().unwrap_or(home);
            let data_local = CommonPath::DataLocal.get().unwrap_or(home);
            let documents = CommonPath::Document.get().unwrap_or(home);

            let resolved = raw_path
                .replace(p::WIN_LOCAL_APP_DATA_LOW, data_local_low)
                .replace(p::WIN_APP_DATA, data_roaming)
                .replace(p::WIN_LOCAL_APP_DATA, data_local)
                .replace(p::WIN_DOCUMENTS, documents)
                .replace(p::HOME, home)
                .replace(&format!("/{}", p::STORE_USER_ID), "")
                .replace(&format!("\\{}", p::STORE_USER_ID), "")
                .replace(&format!("/{}", p::OS_USER_NAME), "")
                .replace(&format!("\\{}", p::OS_USER_NAME), "")
                .replace('*', "");

            let resolved = resolved.replace('/', "\\");

            if std::path::Path::new(&resolved).is_dir() {
                log::debug!(
                    "resolve_expected_save_path: found existing Windows dir: {}",
                    resolved
                );
                return Some(resolved);
            }

            if let Some(parent) = std::path::Path::new(&resolved).parent() {
                if parent.is_dir() {
                    log::debug!(
                        "resolve_expected_save_path: parent exists, returning Windows candidate: {}",
                        resolved
                    );
                    return Some(resolved);
                }
            }
        }
    }

    // --- Linux: rutas Proton (Steam) ---
    #[cfg(target_os = "linux")]
    {
        for root in _config.expanded_roots().iter() {
            if root.store() != crate::resource::manifest::Store::Steam {
                continue;
            }

            let root_path = root.path().render();

            for steam_id in game.all_ids().steam(None) {
                let prefix = format!(
                    "{}/steamapps/compatdata/{}/pfx/drive_c/users/steamuser",
                    root_path, steam_id
                );

                for (raw_path, _) in &game.files {
                    if raw_path.trim().is_empty() {
                        continue;
                    }

                    if !raw_path.contains(p::WIN_LOCAL_APP_DATA_LOW)
                        && !raw_path.contains(p::WIN_APP_DATA)
                        && !raw_path.contains(p::WIN_LOCAL_APP_DATA)
                        && !raw_path.contains(p::WIN_DOCUMENTS)
                        && !raw_path.contains(p::HOME)
                    {
                        continue;
                    }

                    let resolved = raw_path
                        .replace(p::WIN_LOCAL_APP_DATA_LOW, &format!("{}/AppData/LocalLow", prefix))
                        .replace(p::WIN_APP_DATA, &format!("{}/AppData/Roaming", prefix))
                        .replace(p::WIN_LOCAL_APP_DATA, &format!("{}/AppData/Local", prefix))
                        .replace(p::WIN_DOCUMENTS, &format!("{}/Documents", prefix))
                        .replace(p::HOME, &prefix)
                        .replace(&format!("/{}", p::STORE_USER_ID), "")
                        .replace(&format!("/{}", p::OS_USER_NAME), "")
                        .replace('*', "");

                    let resolved = resolved
                        .split('/')
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("/");
                    let resolved = format!("/{}", resolved);

                    if std::path::Path::new(&resolved).is_dir() {
                        log::debug!(
                            "resolve_expected_save_path: found existing Proton dir: {}",
                            resolved
                        );
                        return Some(resolved);
                    }

                    if std::path::Path::new(&prefix).is_dir() {
                        log::debug!(
                            "resolve_expected_save_path: prefix exists, returning Proton candidate: {}",
                            resolved
                        );
                        return Some(resolved);
                    }
                }
            }
        }
    }

    // --- Linux: rutas nativas XDG ---
    #[cfg(target_os = "linux")]
    {
        for (raw_path, _) in &game.files {
            if raw_path.trim().is_empty() {
                continue;
            }

            if !raw_path.contains(p::XDG_DATA) && !raw_path.contains(p::XDG_CONFIG) {
                continue;
            }

            let data_dir = CommonPath::Data.get().unwrap_or(home);
            let config_dir = CommonPath::Config.get().unwrap_or(home);

            let resolved = raw_path
                .replace(p::XDG_DATA, data_dir)
                .replace(p::XDG_CONFIG, config_dir)
                .replace(&format!("/{}", p::STORE_USER_ID), "")
                .replace(&format!("/{}", p::OS_USER_NAME), "")
                .replace('*', "");

            let resolved = resolved
                .split('/')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("/");
            let resolved = format!("/{}", resolved);

            if std::path::Path::new(&resolved).is_dir() {
                log::debug!(
                    "resolve_expected_save_path: found existing XDG dir: {}",
                    resolved
                );
                return Some(resolved);
            }
        }
    }

    None
}
