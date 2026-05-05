use crate::resource::manifest::Game;
use crate::{
    prelude::{CommandError, StrictPath},
    resource::config::Config,
    sync::{
        conflict::DirectoryScanResult,
        device::DeviceIdentity,
        game_list::{game_zip_file_name, GameListFile, GameMetaData, GAME_LIST_FILE_NAME},
    },
};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, Mutex};

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

/// Extrae un mensaje útil de un CommandError. Preferimos el stderr (contiene el error real
/// de rclone con patrones como "no such host", "invalid_grant", etc.) sobre el comando ejecutado
/// (que puede confundir al clasificador porque contiene flags como `--checksum`).
fn command_error_message(e: &CommandError) -> String {
    match e {
        CommandError::Exited {
            stderr: Some(s), ..
        } if !s.trim().is_empty() => s.clone(),
        CommandError::Exited {
            stdout: Some(s), ..
        } if !s.trim().is_empty() => s.clone(),
        _ => e.command(),
    }
}

/// Directorio temporal para zips durante la sincronización.
fn temp_zip_dir(app_dir: &StrictPath) -> StrictPath {
    app_dir.joined("sync-temp-zips")
}

/// Crea un zip de todos los ficheros en `folder_path` y lo escribe en `zip_path`.
pub fn create_zip_from_folder(folder_path: &str, zip_path: &StrictPath) -> Result<(), SyncError> {
    let folder = std::path::Path::new(folder_path);
    if !folder.is_dir() {
        return Err(SyncError::IoError(format!(
            "Folder does not exist or is not a directory: {folder_path}"
        )));
    }
    if let Err(e) = zip_path.create_parent_dir() {
        return Err(SyncError::IoError(format!(
            "Failed to create parent dir for zip ({}): {e}",
            zip_path.render()
        )));
    }
    let zip_std = zip_path
        .as_std_path_buf()
        .map_err(|e| SyncError::IoError(format!("Cannot resolve zip path: {e}")))?;

    let file = std::fs::File::create(&zip_std).map_err(|e| {
        SyncError::IoError(format!(
            "Failed to create zip file at {:?}: {e}",
            zip_std
        ))
    })?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);

    let mut files_processed = 0u32;
    let mut bytes_processed = 0u64;
    let mut latest_mtime: Option<std::time::SystemTime> = None;

    for entry in walkdir::WalkDir::new(folder)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        let relative = path
            .strip_prefix(folder)
            .map_err(|e| SyncError::ZipError(format!("strip_prefix failed for {:?}: {e}", path)))?;
        let zip_entry_name = relative.to_string_lossy().replace('\\', "/");

        zip.start_file(&zip_entry_name, options)
            .map_err(|e| SyncError::ZipError(format!(
                "Failed to start zip entry {} (file {:?}): {e}",
                zip_entry_name, path
            )))?;

        let mut f = std::fs::File::open(path).map_err(|e| {
            SyncError::IoError(format!(
                "Failed to open source file {:?} for zipping: {e}",
                path
            ))
        })?;

        let mut buffer = [0u8; 65536];
        loop {
            let n = f.read(&mut buffer).map_err(|e| {
                SyncError::IoError(format!(
                    "Failed to read from source file {:?}: {e}",
                    path
                ))
            })?;
            if n == 0 {
                break;
            }
            zip.write_all(&buffer[..n]).map_err(|e| {
                SyncError::ZipError(format!(
                    "Failed to write to zip (entry {}): {e}",
                    zip_entry_name
                ))
            })?;
            bytes_processed += n as u64;
        }

        files_processed += 1;
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                latest_mtime = Some(latest_mtime.map_or(mtime, |existing| existing.max(mtime)));
            }
        }
    }

    zip.finish().map_err(|e| SyncError::ZipError(format!("zip.finish() failed: {e}")))?;

    // El mtime del ZIP se fija al del save mas reciente para que la comparacion
    // de status en LOCAL (mtime ZIP vs mtime saves) muestre "synced" tras un
    // backup, en lugar de "pending_restore" por el ZIP recien creado.
    if let Some(mtime) = latest_mtime {
        let _ = filetime::set_file_mtime(&zip_std, filetime::FileTime::from_system_time(mtime));
    }

    log::debug!(
        "[zip] Created {:?} ({} files, {} bytes)",
        zip_std,
        files_processed,
        bytes_processed
    );

    Ok(())
}

/// Extrae un zip en `output_directory`, forzando el timestamp dado si se proporciona.
///
/// Usa un swap atómico para evitar pérdida de datos si la extracción falla a mitad:
/// 1. Extrae el contenido a un directorio temporal hermano (<output>.ludusavi-tmp).
/// 2. Si la extracción se completa, swap atómico: rename <output> → <output>.ludusavi-old,
///    rename <output>.ludusavi-tmp → <output>, borrar <output>.ludusavi-old.
/// 3. Si la extracción falla a mitad, borrar el temporal. El directorio original no se toca.
pub fn extract_zip_to_directory(
    zip_path: &StrictPath,
    output_directory: &str,
    force_last_write_time: Option<DateTime<Utc>>,
    // Estos dos son opcionales para no romper llamadores que no necesitan safety backup:
    app_dir: Option<&StrictPath>,
    game_id: Option<&str>,
) -> Result<(), SyncError> {
    let output = std::path::Path::new(output_directory);
    // Safety backup: crea un snapshot de los saves actuales antes de destruirlos.
    // Silencioso ante errores — no debe bloquear la operación principal.
    if let (Some(app_dir), Some(game_id)) = (app_dir, game_id) {
        if let Err(e) = create_safety_backup(app_dir, game_id, output_directory) {
            log::warn!(
                "[safety-backup] Failed to create for {}: {} — continuing with operation",
                game_id,
                e
            );
        }
    }
    let tmp = {
        let mut s = output_directory.to_string();
        s.push_str(".ludusavi-tmp");
        std::path::PathBuf::from(s)
    };
    let old = {
        let mut s = output_directory.to_string();
        s.push_str(".ludusavi-old");
        std::path::PathBuf::from(s)
    };

    // Limpiar residuos de ejecuciones anteriores fallidas
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).map_err(|e| SyncError::IoError(e.to_string()))?;
    }
    if old.exists() {
        log::warn!(
            "[extract] Leftover .ludusavi-old directory detected, removing: {:?}",
            old
        );
        std::fs::remove_dir_all(&old).map_err(|e| SyncError::IoError(e.to_string()))?;
    }

    // Crear el directorio temporal donde volcamos el contenido del ZIP
    std::fs::create_dir_all(&tmp).map_err(|e| SyncError::IoError(e.to_string()))?;

    // Helper: si algo sale mal durante la extracción, limpiar el temporal
    let cleanup_tmp = |tmp: &std::path::Path| {
        if tmp.exists() {
            let _ = std::fs::remove_dir_all(tmp);
        }
    };

    let file = match zip_path.open() {
        Ok(f) => f,
        Err(e) => {
            cleanup_tmp(&tmp);
            return Err(SyncError::IoError(e.to_string()));
        }
    };

    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => {
            cleanup_tmp(&tmp);
            return Err(SyncError::ZipError(e.to_string()));
        }
    };

    // Extraer al directorio temporal
    for i in 0..archive.len() {
        let mut zip_file = match archive.by_index(i) {
            Ok(zf) => zf,
            Err(e) => {
                cleanup_tmp(&tmp);
                return Err(SyncError::ZipError(e.to_string()));
            }
        };

        if zip_file.name().ends_with('/') {
            continue;
        }

        let out_path = tmp.join(zip_file.name().replace('\\', "/"));

        if let Some(parent) = out_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                cleanup_tmp(&tmp);
                return Err(SyncError::IoError(e.to_string()));
            }
        }

        let mut out_file = match std::fs::File::create(&out_path) {
            Ok(f) => f,
            Err(e) => {
                cleanup_tmp(&tmp);
                return Err(SyncError::IoError(e.to_string()));
            }
        };

        let mut buffer = [0u8; 65536];
        loop {
            let n = match zip_file.read(&mut buffer) {
                Ok(n) => n,
                Err(e) => {
                    cleanup_tmp(&tmp);
                    return Err(SyncError::IoError(e.to_string()));
                }
            };
            if n == 0 {
                break;
            }
            if let Err(e) = out_file.write_all(&buffer[..n]) {
                cleanup_tmp(&tmp);
                return Err(SyncError::IoError(e.to_string()));
            }
        }

        if let Some(ts) = force_last_write_time {
            let system_time: std::time::SystemTime = ts.into();
            let _ = filetime::set_file_mtime(&out_path, filetime::FileTime::from_system_time(system_time));
        }
    }

    if let Some(ts) = force_last_write_time {
        let system_time: std::time::SystemTime = ts.into();
        let _ = filetime::set_file_mtime(&tmp, filetime::FileTime::from_system_time(system_time));
    }

    // Swap atómico: el directorio original (si existe) se mueve a .ludusavi-old,
    // luego el .ludusavi-tmp se mueve al destino, finalmente se borra .ludusavi-old.
    if output.exists() {
        if let Err(e) = std::fs::rename(output, &old) {
            cleanup_tmp(&tmp);
            return Err(SyncError::IoError(format!(
                "Failed to move original directory aside: {e}"
            )));
        }
    }

    if let Err(e) = std::fs::rename(&tmp, output) {
        // Intentar restaurar el directorio original si lo habíamos movido
        if old.exists() {
            let _ = std::fs::rename(&old, output);
        }
        cleanup_tmp(&tmp);
        return Err(SyncError::IoError(format!(
            "Failed to swap extracted directory into place: {e}"
        )));
    }

    // Éxito: borrar el directorio viejo. No fallamos si esto da error,
    // solo lo loggeamos — el save nuevo ya está en su sitio.
    if old.exists() {
        if let Err(e) = std::fs::remove_dir_all(&old) {
            log::warn!(
                "[extract] Extraction succeeded but could not clean up old directory {:?}: {e}",
                old
            );
        }
    }

    Ok(())
}

/// Lee el game-list.json del cloud usando rclone.
pub fn read_game_list_from_cloud(config: &Config) -> Option<GameListFile> {
    let rclone = make_rclone(config)?;
    let cloud_path = &config.cloud.path;
    let remote_file = format!("{}/{}", cloud_path, GAME_LIST_FILE_NAME);

    // NamedTempFile: nombre único por llamada — evita carrera con otras
    // operaciones rclone concurrentes (daemon + GUI, dos ticks solapados, tests).
    let temp_file = tempfile::Builder::new()
        .prefix("ludusavi-game-list-")
        .suffix(".json")
        .tempfile()
        .ok()?;
    let temp_path = temp_file.path().to_path_buf();
    let temp_strict = StrictPath::from(temp_path.clone());

    let args = [
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
        Ok(_) => temp_strict.read().and_then(|c| serde_json::from_str(&c).ok()),
        Err(_) => None,
    }
    // temp_file se borra automáticamente al salir de scope.
}

/// Sube el game-list.json al cloud usando rclone.
pub fn write_game_list_to_cloud(config: &Config, game_list: &GameListFile) -> Result<(), SyncError> {
    let rclone = make_rclone(config).ok_or(SyncError::NoRcloneConfig)?;
    let cloud_path = &config.cloud.path;

    let json = serde_json::to_string_pretty(game_list).map_err(|e| SyncError::IoError(e.to_string()))?;

    // NamedTempFile genera un nombre único, así operaciones concurrentes (daemon
    // + GUI, dos ticks solapados, tests en paralelo) no se pisan entre sí.
    let mut temp_file = tempfile::Builder::new()
        .prefix("ludusavi-game-list-")
        .suffix(".json")
        .tempfile()
        .map_err(|e| SyncError::IoError(e.to_string()))?;
    use std::io::Write as _;
    temp_file
        .write_all(json.as_bytes())
        .map_err(|e| SyncError::IoError(e.to_string()))?;
    temp_file
        .flush()
        .map_err(|e| SyncError::IoError(e.to_string()))?;
    let temp_path = temp_file.path().to_path_buf();

    let remote_file = format!("{}/{}", cloud_path, GAME_LIST_FILE_NAME);
    let args = [
        "copyto".to_string(),
        "--checksum".to_string(),
        temp_path.to_string_lossy().to_string(),
        rclone.path(&remote_file),
    ];

    crate::prelude::run_command(
        config.apps.rclone.path.raw(),
        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        &[0],
        crate::prelude::Privacy::Public,
    )
    .map_err(|e| SyncError::RcloneError(command_error_message(&e)))?;

    // temp_file se borra automáticamente al salir de scope.
    drop(temp_file);
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
        .path
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

    let args = [
        "copyto".to_string(),
        "--checksum".to_string(),
        zip_path_str,
        rclone.path(&remote_file),
    ];

    crate::prelude::run_command(
        config.apps.rclone.path.raw(),
        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        &[0],
        crate::prelude::Privacy::Public,
    )
    .map_err(|e| SyncError::RcloneError(command_error_message(&e)))?;

    game.last_synced_from = Some(device.id.clone());
    game.last_sync_time_utc = Some(Utc::now());
    game.latest_write_time_utc = scan.latest_write_time_utc;
    game.storage_bytes = scan.storage_bytes;

    // Tras un upload exitoso, este device ha "visto" el cloud en el estado actual.
    // Guardamos el latest_write_time_utc como referencia para detectar conflicts futuros.
    if let Some(mtime) = scan.latest_write_time_utc {
        game.set_last_sync_mtime(&device.id, mtime);
    }

    let _ = zip_path.remove();
    log::info!("[{}] Upload complete", game.name);
    Ok(())
}

/// Descarga el zip de un juego del cloud y lo extrae.
pub fn download_game(
    config: &Config,
    app_dir: &StrictPath,
    device: &DeviceIdentity,
    game: &mut GameMetaData,
) -> Result<(), SyncError> {
    let local_path = game
        .path_by_device
        .get(&device.id)
        .ok_or(SyncError::NoLocalPath)?
        .path
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

    let args = [
        "copyto".to_string(),
        "--checksum".to_string(),
        rclone.path(&remote_file),
        zip_path_str,
    ];

    crate::prelude::run_command(
        config.apps.rclone.path.raw(),
        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        &[0],
        crate::prelude::Privacy::Public,
    )
    .map_err(|e| SyncError::RcloneError(command_error_message(&e)))?;

    if !zip_path.is_file() {
        return Err(SyncError::NoZipInCloud);
    }

    log::info!("[{}] Extracting zip to {}", game.name, local_path);
    extract_zip_to_directory(
        &zip_path,
        &local_path,
        game.latest_write_time_utc,
        Some(app_dir),
        Some(&game.id),
    )?;

    // Tras un download exitoso, este device ha "visto" el cloud en el estado actual.
    // El cloud tiene latest_write_time_utc, que ahora también es el mtime de los saves locales.
    if let Some(mtime) = game.latest_write_time_utc {
        game.set_last_sync_mtime(&device.id, mtime);
    }

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
        let mismatch = split_paths
            .iter()
            .any(|sp| sp.len() <= i || !sp[i].eq_ignore_ascii_case(segment));
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
pub fn extract_root_from_scan(
    found_files: &std::collections::HashMap<crate::prelude::StrictPath, crate::scan::ScannedFile>,
) -> Option<String> {
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

/// Obtiene el ModTime del game-list.json en el cloud sin descargarlo.
/// Usado para detectar cambios sin hacer una descarga completa.
pub fn get_game_list_mod_time(config: &Config) -> Option<String> {
    let rclone = make_rclone(config)?;
    let cloud_path = &config.cloud.path;
    let remote_file = format!("{}/{}", cloud_path, GAME_LIST_FILE_NAME);

    let args = ["lsjson".to_string(), rclone.path(&remote_file)];

    let output = crate::prelude::run_command(
        config.apps.rclone.path.raw(),
        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        &[0],
        crate::prelude::Privacy::Public,
    )
    .ok()?;

    // Parsear el JSON y extraer ModTime
    let parsed: serde_json::Value = serde_json::from_str(&output.stdout).ok()?;
    parsed
        .as_array()?
        .first()?
        .get("ModTime")?
        .as_str()
        .map(|s| s.to_string())
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
        let data_local_low = CommonPath::DataLocalLow.get().unwrap_or(home);
        let data_roaming = CommonPath::Data.get().unwrap_or(home);
        let data_local = CommonPath::DataLocal.get().unwrap_or(home);
        let documents = CommonPath::Document.get().unwrap_or(home);

        for raw_path in game.files.keys() {
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

            let pattern = raw_path
                .replace(p::WIN_LOCAL_APP_DATA_LOW, data_local_low)
                .replace(p::WIN_APP_DATA, data_roaming)
                .replace(p::WIN_LOCAL_APP_DATA, data_local)
                .replace(p::WIN_DOCUMENTS, documents)
                .replace(p::HOME, home)
                .replace(p::STORE_USER_ID, "*")
                .replace(p::OS_USER_NAME, "*");

            if let Some(found) = resolve_dir_pattern(&pattern) {
                log::debug!("resolve_expected_save_path: resolved Windows dir: {}", found);
                return Some(found);
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

                if !std::path::Path::new(&prefix).is_dir() {
                    continue;
                }

                for raw_path in game.files.keys() {
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

                    let pattern = raw_path
                        .replace(p::WIN_LOCAL_APP_DATA_LOW, &format!("{}/AppData/LocalLow", prefix))
                        .replace(p::WIN_APP_DATA, &format!("{}/AppData/Roaming", prefix))
                        .replace(p::WIN_LOCAL_APP_DATA, &format!("{}/AppData/Local", prefix))
                        .replace(p::WIN_DOCUMENTS, &format!("{}/Documents", prefix))
                        .replace(p::HOME, &prefix)
                        .replace(p::STORE_USER_ID, "*")
                        .replace(p::OS_USER_NAME, "*");

                    if let Some(found) = resolve_dir_pattern(&pattern) {
                        log::debug!("resolve_expected_save_path: resolved Proton dir: {}", found);
                        return Some(found);
                    }
                }
            }
        }
    }

    // --- Linux: rutas nativas XDG ---
    #[cfg(target_os = "linux")]
    {
        let data_dir = CommonPath::Data.get().unwrap_or(home);
        let config_dir = CommonPath::Config.get().unwrap_or(home);

        for raw_path in game.files.keys() {
            if raw_path.trim().is_empty() {
                continue;
            }

            if !raw_path.contains(p::XDG_DATA) && !raw_path.contains(p::XDG_CONFIG) {
                continue;
            }

            let pattern = raw_path
                .replace(p::XDG_DATA, data_dir)
                .replace(p::XDG_CONFIG, config_dir)
                .replace(p::STORE_USER_ID, "*")
                .replace(p::OS_USER_NAME, "*");

            if let Some(found) = resolve_dir_pattern(&pattern) {
                log::debug!("resolve_expected_save_path: resolved XDG dir: {}", found);
                return Some(found);
            }
        }
    }

    None
}

/// Toma un patron de path con posibles wildcards (`*`) y devuelve la primera
/// carpeta del filesystem que encaja. Si el ultimo segmento parece un fichero
/// (extension o glob), se elimina antes de buscar — el resolver siempre devuelve
/// la carpeta padre, no el fichero.
fn resolve_dir_pattern(pattern: &str) -> Option<String> {
    let sep = if cfg!(target_os = "windows") { '\\' } else { '/' };
    let pattern = if cfg!(target_os = "windows") {
        pattern.replace('/', "\\")
    } else {
        pattern.replace('\\', "/")
    };

    // Quitar el ultimo segmento si parece un fichero (extension o glob) o esta
    // vacio. La parte que nos interesa es la carpeta contenedora.
    let pattern = match pattern.rfind(sep) {
        Some(idx) => {
            let last = &pattern[idx + 1..];
            if last.is_empty() || last.contains('*') || last.contains('.') {
                pattern[..idx].to_string()
            } else {
                pattern
            }
        }
        None => pattern,
    };

    if !pattern.contains('*') {
        if std::path::Path::new(&pattern).is_dir() {
            return Some(pattern);
        }
        if let Some(parent) = std::path::Path::new(&pattern).parent() {
            if parent.is_dir() {
                return Some(parent.to_string_lossy().to_string());
            }
        }
        return None;
    }

    glob_first_existing_dir(&pattern, sep)
}

/// Camina el filesystem segmento a segmento, expandiendo cada `*` con
/// `read_dir`. Devuelve la primera ruta existente tras consumir todos los
/// segmentos. Toleramos separadores duplicados (`//`) que pueden quedar tras
/// borrar placeholders huerfanos.
fn glob_first_existing_dir(pattern: &str, sep: char) -> Option<String> {
    use std::path::PathBuf;

    let segments: Vec<&str> = pattern.split(sep).collect();
    if segments.is_empty() {
        return None;
    }

    let mut candidates: Vec<PathBuf> = {
        let first = segments[0];
        if cfg!(target_os = "windows") && first.len() == 2 && first.ends_with(':') {
            // "C:" sin separador trailing no es un path valido en Windows.
            vec![PathBuf::from(format!("{first}\\"))]
        } else if first.is_empty() {
            // Path absoluto Unix que empieza con `/`.
            vec![PathBuf::from("/")]
        } else {
            vec![PathBuf::from(first)]
        }
    };

    for seg in &segments[1..] {
        if seg.is_empty() {
            continue;
        }
        let mut next: Vec<PathBuf> = vec![];
        for cand in &candidates {
            if seg.contains('*') {
                // `**` matches zero or more directory levels — incluir el
                // candidato actual como "cero niveles" antes de descender.
                if *seg == "**" {
                    next.push(cand.clone());
                }
                if let Ok(entries) = std::fs::read_dir(cand) {
                    for entry in entries.flatten() {
                        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            next.push(entry.path());
                        }
                    }
                }
            } else {
                next.push(cand.join(seg));
            }
        }
        candidates = next;
        if candidates.is_empty() {
            return None;
        }
    }

    candidates
        .into_iter()
        .find(|p| p.is_dir())
        .map(|p| p.to_string_lossy().to_string())
}
/// Resuelve la ruta de saves de un juego usando el manifiesto de Ludusavi.
/// Primero intenta encontrar saves existentes, luego la ruta esperada.
/// Equivale a lo que hace auto_register_paths en el daemon.
pub fn resolve_game_path_from_manifest(config: &Config, game_name: &str) -> Option<String> {
    use crate::resource::manifest::Manifest;
    use crate::scan::{layout::BackupLayout, scan_game_for_backup, Launchers, SteamShortcuts, TitleFinder};
    use crate::resource::config::{ToggledPaths, ToggledRegistry};
    use crate::prelude::app_dir;

    let manifest = Manifest::load().ok()?.with_extensions(config);
    let game_entry = manifest.0.get(game_name)?;

    let app_dir = app_dir();
    let roots = config.expanded_roots();
    let layout = BackupLayout::new(config.backup.path.clone());
    let title_finder = TitleFinder::new(config, &manifest, layout.restorable_game_set());
    let steam_shortcuts = SteamShortcuts::scan(&title_finder);
    let launchers = Launchers::scan(&roots, &manifest, &[game_name.to_string()], &title_finder, None);

    let scan_info = scan_game_for_backup(
        game_entry,
        game_name,
        &roots,
        &app_dir,
        &launchers,
        None,
        &ToggledPaths::default(),
        &ToggledRegistry::default(),
        &steam_shortcuts,
        false,
    );

    // Primero intentar con ficheros existentes
    if let Some(path) = extract_root_from_scan(&scan_info.found_files) {
        return Some(path);
    }

    // Si no hay ficheros, resolver la ruta esperada
    resolve_expected_save_path(config, game_entry)
}
/// Borra el ZIP de un juego del cloud.
pub fn delete_game_zip_from_cloud(config: &Config, game_name: &str) -> Result<(), SyncError> {
    let rclone = make_rclone(config).ok_or(SyncError::NoRcloneConfig)?;
    let cloud_path = &config.cloud.path;
    let remote_file = format!("{}/{}", cloud_path, game_zip_file_name(game_name));

    let args = ["deletefile".to_string(), rclone.path(&remote_file)];

    crate::prelude::run_command(
        config.apps.rclone.path.raw(),
        &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        &[0],
        crate::prelude::Privacy::Public,
    )
    .map_err(|e| SyncError::RcloneError(command_error_message(&e)))?;

    log::info!("[{}] Cloud ZIP deleted", game_name);
    Ok(())
}
/// Cache en memoria del resolver lite. Se popula la primera vez que un juego
/// se resuelve correctamente y persiste hasta el final de la sesion. Solo se
/// cachean Some, asi un None se reintenta — util cuando la carpeta de saves
/// aun no existe (el usuario no ha lanzado el juego).
static SAVE_PATH_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Version ligera de resolve_game_path_from_manifest.
/// Resuelve placeholders del manifiesto y hace un read_dir minimo por wildcard
/// para encontrar la carpeta real (storeUserId, **). El resultado se cachea
/// para que llamadas posteriores (suscripcion de 5s, view de GameDetail) no
/// vuelvan a tocar disco.
pub fn resolve_game_path_lite(
    config: &Config,
    manifest: &crate::resource::manifest::Manifest,
    game_name: &str,
) -> Option<String> {
    if let Ok(cache) = SAVE_PATH_CACHE.lock() {
        if let Some(cached) = cache.get(game_name) {
            return Some(cached.clone());
        }
    }

    let game_entry = manifest.0.get(game_name)?;
    let result = resolve_expected_save_path(config, game_entry);

    if let Some(ref path) = result {
        if let Ok(mut cache) = SAVE_PATH_CACHE.lock() {
            cache.insert(game_name.to_string(), path.clone());
        }
    }

    result
}

/// Comprueba si rclone está disponible ejecutando `rclone --version`.
/// Check profundo: detecta que el binario existe, es ejecutable y funciona.
/// Usado por el daemon al arrancar. Tarda ~100ms así que no usarlo en caliente.
pub fn rclone_available_deep(config: &Config) -> bool {
    if !config.apps.rclone.is_valid() {
        return false;
    }
    let args = ["--version"];
    matches!(
        crate::prelude::run_command(
            config.apps.rclone.path.raw(),
            &args,
            &[0],
            crate::prelude::Privacy::Public,
        ),
        Ok(_)
    )
}

/// Categoría de un error de sync. Usada para mostrar mensajes accionables al usuario.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ErrorCategory {
    /// No se puede contactar con el cloud (DNS, timeout, red caída).
    Network,
    /// Token OAuth expirado o revocado. Requiere reconfigurar el remote.
    Authentication,
    /// Cuota del cloud llena o disco local lleno.
    StorageFull,
    /// Rate limit del proveedor del cloud. El daemon reintentará.
    RateLimit,
    /// Corrupción detectada (hash mismatch, ZIP inválido, JSON corrupto).
    Corruption,
    /// Fichero o carpeta de saves no encontrados.
    Missing,
    /// Problema de configuración (rclone ausente, remote no definido, etc.).
    Config,
    /// Acceso denegado a ficheros (lockeados por otro proceso, permisos).
    Permission,
    /// Error no clasificado. El mensaje raw se muestra al usuario.
    Unknown,
}

impl ErrorCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Authentication => "authentication",
            Self::StorageFull => "storage_full",
            Self::RateLimit => "rate_limit",
            Self::Corruption => "corruption",
            Self::Missing => "missing",
            Self::Config => "config",
            Self::Permission => "permission",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "network" => Self::Network,
            "authentication" => Self::Authentication,
            "storage_full" => Self::StorageFull,
            "rate_limit" => Self::RateLimit,
            "corruption" => Self::Corruption,
            "missing" => Self::Missing,
            "config" => Self::Config,
            "permission" => Self::Permission,
            _ => Self::Unknown,
        }
    }
}

/// Dirección de la operación que falló. Acompaña a la categoría para dar contexto.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OperationDirection {
    /// Subida al cloud.
    Upload,
    /// Descarga del cloud.
    Download,
    /// Backup local (modo LOCAL).
    Backup,
    /// Restore local (modo LOCAL).
    Restore,
}

impl OperationDirection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Upload => "upload",
            Self::Download => "download",
            Self::Backup => "backup",
            Self::Restore => "restore",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "upload" => Self::Upload,
            "download" => Self::Download,
            "backup" => Self::Backup,
            "restore" => Self::Restore,
            _ => Self::Upload,
        }
    }
}

/// Clasifica un SyncError en categoría + mensaje limpio para el usuario.
///
/// Para `RcloneError` intenta extraer patrones del comando/stderr.
/// Actualmente `RcloneError` solo contiene el comando ejecutado (no el stderr limpio),
/// así que los patrones se aplican sobre esa string.
pub fn classify_error(
    error: &SyncError,
    direction: OperationDirection,
) -> (ErrorCategory, String, OperationDirection) {
    let raw = match error {
        SyncError::RcloneError(s) => s.clone(),
        SyncError::IoError(s) => s.clone(),
        SyncError::ZipError(s) => s.clone(),
        SyncError::NoLocalPath => "No local path configured for this game on this device.".to_string(),
        SyncError::NoRcloneConfig => {
            return (
                ErrorCategory::Config,
                "Rclone is not configured.".to_string(),
                direction,
            );
        }
        SyncError::NoZipInCloud => {
            return (
                ErrorCategory::Missing,
                "No backup found in the cloud for this game yet.".to_string(),
                direction,
            );
        }
    };
    let lower = raw.to_lowercase();

    // Authentication: problemas de credenciales OAuth.
    // invalid_grant = token refresh fallido; invalid_client = client ID mal;
    // redirect_uri_mismatch = configuración de OAuth mal en el proveedor.
    if lower.contains("invalid_grant")
        || lower.contains("invalid_client")
        || lower.contains("unauthorized")
        || lower.contains("token expired")
        || lower.contains("oauth2")
        || lower.contains("access_denied")
        || lower.contains("authentication")
        || lower.contains("redirect_uri_mismatch")
        || lower.contains("401")
    {
        (
            ErrorCategory::Authentication,
            "Cloud credentials expired or invalid. Please re-authorize rclone.".to_string(),
            direction,
        )
    }
    // StorageFull: espacio agotado, tanto en cloud como en disco local.
    else if lower.contains("storagequotaexceeded")
        || lower.contains("quota exceeded")
        || lower.contains("insufficient storage")
        || lower.contains("no space left")
        || lower.contains("not enough space on the disk")
        || lower.contains("disk full")
    {
        (
            ErrorCategory::StorageFull,
            "Storage is full. Free up space or upgrade your plan.".to_string(),
            direction,
        )
    }
    // RateLimit: límites de API (Google Drive tiene varios, nombramos los comunes).
    else if lower.contains("ratelimitexceeded")
        || lower.contains("rate limit")
        || lower.contains("too many requests")
        || lower.contains("dailylimitexceeded")
        || lower.contains("sharingratelimit")
        || lower.contains("429")
    {
        (
            ErrorCategory::RateLimit,
            "Too many requests to the cloud. Will retry automatically later.".to_string(),
            direction,
        )
    }
    // Network: problemas de conectividad. Se evalúa ANTES que Missing y Permission
    // porque `404` y `403` aparecen a veces en errores de resolución DNS/TLS.
    else if lower.contains("no such host")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("network is unreachable")
        || lower.contains("timed out")
        || lower.contains("i/o timeout")
        || lower.contains("tls handshake")
        || lower.contains("temporary failure in name resolution")
        || lower.contains("dial tcp")
        || lower.contains("broken pipe")
    {
        (
            ErrorCategory::Network,
            "Cannot reach the cloud. Check your internet connection.".to_string(),
            direction,
        )
    }
    // Permission: acceso denegado (local o cloud). Google Drive devuelve
    // `appNotAuthorizedToFile` y `cannotDownloadAbusiveFile`.
    else if lower.contains("permission denied")
        || lower.contains("access is denied")
        || lower.contains("access denied")
        || lower.contains("forbidden")
        || lower.contains("appnotauthorized")
        || lower.contains("cannot access the file")
        || lower.contains("403")
    {
        (
            ErrorCategory::Permission,
            "Access denied. Check file/folder permissions.".to_string(),
            direction,
        )
    }
    // Missing: fichero o path no existe.
    else if lower.contains("object not found")
        || lower.contains("file not found")
        || lower.contains("no such file")
        || lower.contains("404")
    {
        (
            ErrorCategory::Missing,
            "Expected file was not found in the cloud.".to_string(),
            direction,
        )
    }
    // Corruption: datos corruptos reales (hash real no coincide, fichero dañado).
    // Nota: no matcheamos "checksum" a secas — es demasiado ambiguo, aparece en
    // flags legítimos de rclone y en mensajes informativos.
    else if lower.contains("hash differ")
        || lower.contains("corrupt")
        || lower.contains("integrity")
    {
        (
            ErrorCategory::Corruption,
            "Data integrity check failed. The file may have been corrupted during transfer.".to_string(),
            direction,
        )
    }
    // Unknown: fallback. Muestra la primera línea del error real.
    else {
        (
            ErrorCategory::Unknown,
            format!("Unexpected error: {}", raw.lines().next().unwrap_or(&raw)),
            direction,
        )
    }
}

// ============================================================================
// Safety backups — protegen contra pérdida de saves en operaciones destructivas
// ============================================================================

/// Tamaño máximo (bytes) a partir del cual saltamos el safety backup.
/// 500 MB. Evita problemas con emuladores que manejan GBs de estado.
const SAFETY_BACKUP_MAX_BYTES: u64 = 500 * 1024 * 1024;

/// Sanitiza un game_id para que sea un nombre de directorio válido en Windows y Linux.
/// Reemplaza caracteres problemáticos por guion bajo.
fn sanitize_game_id_for_fs(game_id: &str) -> String {
    game_id
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect()
}

/// Directorio raíz de safety backups dentro de app_dir.
pub fn safety_backup_dir(app_dir: &StrictPath) -> StrictPath {
    app_dir.joined("safety-backups")
}

/// Path del snapshot de un juego concreto.
pub fn safety_backup_path_for_game(app_dir: &StrictPath, game_id: &str) -> StrictPath {
    safety_backup_dir(app_dir)
        .joined(&sanitize_game_id_for_fs(game_id))
        .joined("snapshot")
}

/// Metadata de un safety backup existente.
#[derive(Debug, Clone)]
pub struct SafetyBackupInfo {
    pub created_at: DateTime<Utc>,
    pub size_bytes: u64,
}

/// Devuelve info del safety backup de un juego, si existe.
pub fn get_safety_backup_info(app_dir: &StrictPath, game_id: &str) -> Option<SafetyBackupInfo> {
    let snapshot = safety_backup_path_for_game(app_dir, game_id);
    let snapshot_path = snapshot.as_std_path_buf().ok()?;

    if !snapshot_path.is_dir() {
        return None;
    }

    // created_at: mtime del propio directorio snapshot
    let meta = std::fs::metadata(&snapshot_path).ok()?;
    let created_at: DateTime<Utc> = meta.modified().ok()?.into();

    // size_bytes: suma de todos los ficheros del snapshot
    let size_bytes = walkdir::WalkDir::new(&snapshot_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum();

    Some(SafetyBackupInfo { created_at, size_bytes })
}

/// Calcula el tamaño total (bytes) de un directorio. Cero si no existe.
fn directory_size_bytes(path: &std::path::Path) -> u64 {
    if !path.is_dir() {
        return 0;
    }
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Comprueba si un directorio está vacío (o no existe).
fn directory_is_empty_or_missing(path: &std::path::Path) -> bool {
    if !path.is_dir() {
        return true;
    }
    match std::fs::read_dir(path) {
        Ok(mut iter) => iter.next().is_none(),
        Err(_) => true,
    }
}

/// Copia recursiva de un directorio a otro. Sobrescribe si el destino existe.
/// Implementación simple para evitar dependencia adicional.
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;

    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let rel = entry.path().strip_prefix(src).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
        })?;

        let target = dst.join(rel);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &target)?;
        }
        // Symlinks y otros tipos: los ignoramos. Los saves no suelen tenerlos.
    }

    Ok(())
}

/// Crea un safety backup del directorio de saves local antes de una operación destructiva.
///
/// Condiciones de skip (devuelven Ok(()) sin error):
/// - Flag `safety_backups_enabled` desactivado en sync-games.json.
/// - El directorio de saves no existe o está vacío (nada que proteger).
/// - El directorio pesa más de SAFETY_BACKUP_MAX_BYTES (500 MB).
///
/// Si un snapshot anterior existe, se sobrescribe (mantenemos solo 1).
pub fn create_safety_backup(
    app_dir: &StrictPath,
    game_id: &str,
    save_path: &str,
) -> Result<(), SyncError> {
    // Cargar sync-games.json para ver si el flag está activo.
    // Nota: usamos SyncGamesConfig::load() porque es el único sitio donde vive el flag global.
    let sync_config = crate::sync::sync_config::SyncGamesConfig::load();
    if !sync_config.safety_backups_enabled() {
        log::debug!("[safety-backup] Disabled by config, skipping for {}", game_id);
        return Ok(());
    }

    let src = std::path::Path::new(save_path);

    // Directorio vacío o inexistente: nada que proteger
    if directory_is_empty_or_missing(src) {
        log::debug!(
            "[safety-backup] Source empty/missing, skipping for {}: {}",
            game_id,
            save_path
        );
        return Ok(());
    }

    // Tamaño excesivo: saltar con warning
    let size = directory_size_bytes(src);
    if size > SAFETY_BACKUP_MAX_BYTES {
        log::warn!(
            "[safety-backup] Skipping {} ({}MB > {}MB limit)",
            game_id,
            size / (1024 * 1024),
            SAFETY_BACKUP_MAX_BYTES / (1024 * 1024)
        );
        return Ok(());
    }

    let snapshot = safety_backup_path_for_game(app_dir, game_id);
    let snapshot_path = snapshot
        .as_std_path_buf()
        .map_err(|e| SyncError::IoError(e.to_string()))?;

    // Borrar snapshot anterior si existe (mantenemos solo 1)
    if snapshot_path.exists() {
        std::fs::remove_dir_all(&snapshot_path)
            .map_err(|e| SyncError::IoError(format!("Failed to clean previous safety backup: {e}")))?;
    }

    let started = std::time::Instant::now();
    copy_dir_recursive(src, &snapshot_path)
        .map_err(|e| SyncError::IoError(format!("Failed to create safety backup: {e}")))?;

    log::info!(
        "[safety-backup] Created for {} ({}KB in {}ms)",
        game_id,
        size / 1024,
        started.elapsed().as_millis()
    );

    Ok(())
}

/// Restaura un safety backup al directorio de saves original.
/// Usa el mismo swap atómico que extract_zip_to_directory para evitar estados inconsistentes.
pub fn restore_safety_backup(
    app_dir: &StrictPath,
    game_id: &str,
    save_path: &str,
) -> Result<(), SyncError> {
    let snapshot = safety_backup_path_for_game(app_dir, game_id);
    let snapshot_path = snapshot
        .as_std_path_buf()
        .map_err(|e| SyncError::IoError(e.to_string()))?;

    if !snapshot_path.is_dir() {
        return Err(SyncError::IoError(format!(
            "No safety backup found for {}",
            game_id
        )));
    }

    let output = std::path::Path::new(save_path);
    let tmp = {
        let mut s = save_path.to_string();
        s.push_str(".ludusavi-tmp");
        std::path::PathBuf::from(s)
    };
    let old = {
        let mut s = save_path.to_string();
        s.push_str(".ludusavi-old");
        std::path::PathBuf::from(s)
    };

    // Limpiar residuos
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).map_err(|e| SyncError::IoError(e.to_string()))?;
    }
    if old.exists() {
        log::warn!(
            "[safety-backup] Leftover .ludusavi-old detected during restore, removing: {:?}",
            old
        );
        std::fs::remove_dir_all(&old).map_err(|e| SyncError::IoError(e.to_string()))?;
    }

    // Copiar snapshot a tmp
    copy_dir_recursive(&snapshot_path, &tmp)
        .map_err(|e| SyncError::IoError(format!("Failed to stage safety backup: {e}")))?;

    // Swap atómico
    if output.exists() {
        if let Err(e) = std::fs::rename(output, &old) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(SyncError::IoError(format!(
                "Failed to move current saves aside: {e}"
            )));
        }
    }

    if let Err(e) = std::fs::rename(&tmp, output) {
        // Intentar restaurar
        if old.exists() {
            let _ = std::fs::rename(&old, output);
        }
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(SyncError::IoError(format!(
            "Failed to swap safety backup into place: {e}"
        )));
    }

    if old.exists() {
        if let Err(e) = std::fs::remove_dir_all(&old) {
            log::warn!(
                "[safety-backup] Restore succeeded but could not clean up old directory: {e}"
            );
        }
    }

    log::info!("[safety-backup] Restored for {}", game_id);
    Ok(())
}

/// Borra el safety backup de un juego (y su directorio padre si queda vacío).
pub fn delete_safety_backup(app_dir: &StrictPath, game_id: &str) -> Result<(), SyncError> {
    let snapshot = safety_backup_path_for_game(app_dir, game_id);
    let snapshot_path = snapshot
        .as_std_path_buf()
        .map_err(|e| SyncError::IoError(e.to_string()))?;

    if snapshot_path.is_dir() {
        std::fs::remove_dir_all(&snapshot_path)
            .map_err(|e| SyncError::IoError(format!("Failed to delete safety backup: {e}")))?;
    }

    // Intentar borrar el directorio del juego si queda vacío
    if let Some(game_dir) = snapshot_path.parent() {
        if game_dir.is_dir() {
            let _ = std::fs::remove_dir(game_dir); // silencioso: si no está vacío, no pasa nada
        }
    }

    log::info!("[safety-backup] Deleted for {}", game_id);
    Ok(())
}
/// Crea un snapshot permanente del directorio de saves para "Keep both" en conflict resolution.
/// A diferencia del safety backup automático, este NO se sobrescribe — se identifica por timestamp.
/// Devuelve el path del snapshot creado.
pub fn create_keep_both_snapshot(
    app_dir: &StrictPath,
    game_id: &str,
    save_path: &str,
) -> Result<StrictPath, SyncError> {
    let folder = std::path::Path::new(save_path);
    if !folder.is_dir() {
        return Err(SyncError::IoError(format!(
            "Cannot create keep-both snapshot — folder does not exist: {save_path}"
        )));
    }

    let sanitized = sanitize_game_id_for_fs(game_id);
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let snapshot_dir = app_dir
        .joined("safety-backups")
        .joined(&sanitized)
        .joined(&format!("keep-both-{}", timestamp));

    if let Err(e) = snapshot_dir.create_dirs() {
        return Err(SyncError::IoError(format!(
            "Cannot create keep-both directory: {e}"
        )));
    }

    let snapshot_std = snapshot_dir
        .as_std_path_buf()
        .map_err(|e| SyncError::IoError(e.to_string()))?;

    copy_dir_recursive(folder, &snapshot_std)
        .map_err(|e| SyncError::IoError(e.to_string()))?;

    log::info!(
        "[keep-both] Snapshot created for {} at {:?}",
        game_id,
        snapshot_std
    );

    Ok(snapshot_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    // get_common_root_folder ------------------------------------------------------

    #[test]
    fn common_root_returns_none_for_empty_input() {
        assert_eq!(get_common_root_folder(&[]), None);
    }

    #[test]
    fn common_root_returns_full_path_for_single_path() {
        // Con un solo path la "raíz común" es el path entero (sin parent).
        let result = get_common_root_folder(&["/home/jayo/saves"]);
        assert_eq!(result.as_deref(), Some("/home/jayo/saves"));
    }

    #[test]
    fn common_root_returns_shared_prefix() {
        let result = get_common_root_folder(&[
            "/home/jayo/saves/game1/save1.dat",
            "/home/jayo/saves/game1/save2.dat",
        ]);
        assert_eq!(result.as_deref(), Some("/home/jayo/saves/game1"));
    }

    #[test]
    fn common_root_returns_root_when_only_root_overlaps() {
        // Dos rutas absolutas que solo comparten "/": la raíz común es "/".
        let result = get_common_root_folder(&["/home/jayo/a", "/var/log/b"]);
        assert_eq!(result.as_deref(), Some("/"));
    }

    #[test]
    fn common_root_returns_none_for_relative_paths_with_no_overlap() {
        // Sin raíz absoluta y sin componentes comunes → None.
        let result = get_common_root_folder(&["foo/bar", "baz/qux"]);
        assert_eq!(result, None);
    }

    #[test]
    fn common_root_is_case_insensitive() {
        // Importante en Windows: C:\Users\Foo y c:\users\Foo deben matchear.
        let result = get_common_root_folder(&[
            "/Home/Jayo/Saves/game",
            "/home/jayo/saves/game",
        ]);
        // El resultado conserva el case del primero.
        assert!(result.is_some(), "expected a common root, got None");
        let root = result.unwrap();
        assert!(
            root.eq_ignore_ascii_case("/home/jayo/saves/game"),
            "got {root:?}"
        );
    }

    #[test]
    fn common_root_handles_subset() {
        // Si una ruta es prefijo de otra, la común es la corta.
        let result = get_common_root_folder(&["/a/b", "/a/b/c/d"]);
        assert_eq!(result.as_deref(), Some("/a/b"));
    }

    // sanitize_game_id_for_fs -----------------------------------------------------

    #[test]
    fn sanitize_replaces_illegal_windows_chars() {
        // 9 caracteres prohibidos por NTFS, cada uno → '_'.
        // Input: game<name>:foo|bar?*"\baz/qux  (9 ilegales en total)
        let input = "game<name>:foo|bar?*\"\\baz/qux";
        let expected = "game_name__foo_bar____baz_qux";
        assert_eq!(sanitize_game_id_for_fs(input), expected);
    }

    #[test]
    fn sanitize_replaces_each_illegal_char_with_underscore() {
        // Test más simple, char por char.
        for illegal in ['<', '>', ':', '"', '/', '\\', '|', '?', '*'] {
            let input = format!("a{illegal}b");
            let sanitized = sanitize_game_id_for_fs(&input);
            assert_eq!(sanitized, "a_b", "failed for {illegal:?}");
        }
    }

    #[test]
    fn sanitize_replaces_control_characters() {
        let input = "name\twith\nnewlines\0";
        let sanitized = sanitize_game_id_for_fs(input);
        assert_eq!(sanitized, "name_with_newlines_");
    }

    #[test]
    fn sanitize_preserves_normal_unicode_and_spaces() {
        assert_eq!(
            sanitize_game_id_for_fs("Stardew Valley"),
            "Stardew Valley"
        );
        assert_eq!(sanitize_game_id_for_fs("龙之谷"), "龙之谷");
        assert_eq!(sanitize_game_id_for_fs("Pokémon"), "Pokémon");
    }

    // safety_backup_dir / safety_backup_path_for_game ---------------------------

    #[test]
    fn safety_backup_path_concatenates_under_app_dir() {
        let app_dir = StrictPath::new("/tmp/ludusavi-app".to_string());
        let game_path = safety_backup_path_for_game(&app_dir, "Stardew Valley");
        let rendered = game_path.render();
        assert!(
            rendered.contains("safety-backups"),
            "expected safety-backups in path, got {rendered}"
        );
        assert!(
            rendered.contains("Stardew Valley"),
            "expected game name in path, got {rendered}"
        );
        assert!(
            rendered.ends_with("snapshot"),
            "expected to end with snapshot, got {rendered}"
        );
    }

    #[test]
    fn safety_backup_path_sanitizes_game_id() {
        let app_dir = StrictPath::new("/tmp/ludusavi-app".to_string());
        // Un game_id con ':' debe sanearse a '_' en el nombre del directorio.
        let path = safety_backup_path_for_game(&app_dir, "Half-Life 2: Episode One");
        let rendered = path.render();
        assert!(
            rendered.contains("Half-Life 2_ Episode One"),
            "expected colon sanitized, got {rendered}"
        );
    }

    // ErrorCategory + OperationDirection round-trip ------------------------------

    #[test]
    fn error_category_str_round_trip() {
        for cat in [
            ErrorCategory::Network,
            ErrorCategory::Authentication,
            ErrorCategory::StorageFull,
            ErrorCategory::RateLimit,
            ErrorCategory::Corruption,
            ErrorCategory::Missing,
            ErrorCategory::Config,
            ErrorCategory::Permission,
            ErrorCategory::Unknown,
        ] {
            let s = cat.as_str();
            let parsed = ErrorCategory::from_str(s);
            assert_eq!(parsed.as_str(), s, "round-trip failed for {s}");
        }
    }

    #[test]
    fn error_category_from_str_unknown_falls_back() {
        assert_eq!(ErrorCategory::from_str("nonsense").as_str(), "unknown");
    }

    #[test]
    fn operation_direction_str_round_trip() {
        for dir in [
            OperationDirection::Upload,
            OperationDirection::Download,
            OperationDirection::Backup,
            OperationDirection::Restore,
        ] {
            let s = dir.as_str();
            let parsed = OperationDirection::from_str(s);
            assert_eq!(parsed.as_str(), s);
        }
    }

    #[test]
    fn operation_direction_from_str_unknown_falls_back_to_upload() {
        assert_eq!(
            OperationDirection::from_str("nonsense").as_str(),
            "upload"
        );
    }

    // classify_error: cada categoría con un input representativo -----------------

    fn classify(raw: &str) -> ErrorCategory {
        let err = SyncError::RcloneError(raw.to_string());
        let (cat, _msg, _dir) = classify_error(&err, OperationDirection::Upload);
        cat
    }

    #[test]
    fn classify_error_authentication_patterns() {
        assert_eq!(classify("invalid_grant: token expired"), ErrorCategory::Authentication);
        assert_eq!(classify("HTTP 401 Unauthorized"), ErrorCategory::Authentication);
        assert_eq!(classify("oauth2: redirect_uri_mismatch"), ErrorCategory::Authentication);
    }

    #[test]
    fn classify_error_storage_full_patterns() {
        assert_eq!(classify("storageQuotaExceeded"), ErrorCategory::StorageFull);
        assert_eq!(classify("No space left on device"), ErrorCategory::StorageFull);
        assert_eq!(classify("disk full"), ErrorCategory::StorageFull);
    }

    #[test]
    fn classify_error_rate_limit_patterns() {
        assert_eq!(classify("rateLimitExceeded"), ErrorCategory::RateLimit);
        assert_eq!(classify("HTTP 429 Too Many Requests"), ErrorCategory::RateLimit);
        assert_eq!(classify("dailyLimitExceeded"), ErrorCategory::RateLimit);
    }

    #[test]
    fn classify_error_network_patterns() {
        assert_eq!(classify("dial tcp: lookup foo: no such host"), ErrorCategory::Network);
        assert_eq!(classify("connection reset by peer"), ErrorCategory::Network);
        assert_eq!(classify("i/o timeout"), ErrorCategory::Network);
        assert_eq!(classify("TLS handshake failure"), ErrorCategory::Network);
    }

    #[test]
    fn classify_error_permission_patterns() {
        assert_eq!(classify("permission denied"), ErrorCategory::Permission);
        assert_eq!(classify("HTTP 403 Forbidden"), ErrorCategory::Permission);
    }

    #[test]
    fn classify_error_missing_patterns() {
        assert_eq!(classify("object not found"), ErrorCategory::Missing);
        assert_eq!(classify("HTTP 404"), ErrorCategory::Missing);
    }

    #[test]
    fn classify_error_corruption_patterns() {
        assert_eq!(classify("hash differ between source and dest"), ErrorCategory::Corruption);
        assert_eq!(classify("file is corrupt"), ErrorCategory::Corruption);
    }

    #[test]
    fn classify_error_unknown_falls_through() {
        let cat = classify("something completely random nobody saw coming");
        assert_eq!(cat, ErrorCategory::Unknown);
    }

    /// Network gana sobre Missing y Permission cuando 404/403 aparecen
    /// junto con "no such host" o "timed out".
    #[test]
    fn classify_error_network_wins_over_404_when_dns_fails() {
        let cat = classify("dial tcp: lookup api.dropbox.com: no such host (returned 404)");
        assert_eq!(cat, ErrorCategory::Network);
    }

    /// SyncError::NoRcloneConfig clasifica como Config sin importar el direction.
    #[test]
    fn classify_error_no_rclone_config_is_config() {
        let (cat, _msg, _dir) =
            classify_error(&SyncError::NoRcloneConfig, OperationDirection::Download);
        assert_eq!(cat, ErrorCategory::Config);
    }

    /// SyncError::NoZipInCloud clasifica como Missing.
    #[test]
    fn classify_error_no_zip_in_cloud_is_missing() {
        let (cat, _msg, _dir) =
            classify_error(&SyncError::NoZipInCloud, OperationDirection::Download);
        assert_eq!(cat, ErrorCategory::Missing);
    }

    /// El direction se preserva en el resultado.
    #[test]
    fn classify_error_preserves_direction() {
        let err = SyncError::RcloneError("permission denied".to_string());
        let (_, _, dir) = classify_error(&err, OperationDirection::Backup);
        assert_eq!(dir, OperationDirection::Backup);
    }

    // ============================================================================
    // Tests con filesystem real (tempdir). Sin red.
    // ============================================================================

    use chrono::TimeZone;
    use std::fs;
    use std::path::Path;

    /// Escribe `content` en `path`, creando los padres si no existen.
    fn write_file(path: &Path, content: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// Lee el contenido íntegro de un fichero o `panic!`.
    fn read_file(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap_or_else(|e| panic!("read({path:?}) failed: {e}"))
    }

    /// Strict path desde una std::path::Path.
    fn sp(p: &Path) -> StrictPath {
        StrictPath::new(p.to_string_lossy().to_string())
    }

    // create_zip_from_folder + extract_zip_to_directory: round-trip ----------

    #[test]
    fn zip_round_trip_preserves_file_contents() {
        let src = tempfile::tempdir().unwrap();
        let zip_dir = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        // Source con varios ficheros.
        write_file(&src.path().join("save1.dat"), b"hello world");
        write_file(&src.path().join("config.ini"), b"[settings]\nvalue=42\n");

        let zip_path = zip_dir.path().join("game.zip");
        create_zip_from_folder(&src.path().to_string_lossy(), &sp(&zip_path)).unwrap();
        assert!(zip_path.exists(), "zip should exist after creation");

        // Extraer en un destino distinto.
        let dst_dir = dst.path().join("extracted");
        extract_zip_to_directory(
            &sp(&zip_path),
            &dst_dir.to_string_lossy(),
            None,
            None,
            None,
        )
        .unwrap();

        // Los contenidos deben ser idénticos.
        assert_eq!(read_file(&dst_dir.join("save1.dat")), b"hello world");
        assert_eq!(
            read_file(&dst_dir.join("config.ini")),
            b"[settings]\nvalue=42\n"
        );
    }

    #[test]
    fn zip_round_trip_preserves_nested_directories() {
        let src = tempfile::tempdir().unwrap();
        let zip_dir = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        write_file(&src.path().join("top.txt"), b"top");
        write_file(&src.path().join("subdir/inner.txt"), b"inner");
        write_file(&src.path().join("subdir/deep/deepest.txt"), b"deepest");

        let zip_path = zip_dir.path().join("game.zip");
        create_zip_from_folder(&src.path().to_string_lossy(), &sp(&zip_path)).unwrap();

        let dst_dir = dst.path().join("extracted");
        extract_zip_to_directory(
            &sp(&zip_path),
            &dst_dir.to_string_lossy(),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(read_file(&dst_dir.join("top.txt")), b"top");
        assert_eq!(read_file(&dst_dir.join("subdir/inner.txt")), b"inner");
        assert_eq!(
            read_file(&dst_dir.join("subdir/deep/deepest.txt")),
            b"deepest"
        );
    }

    #[test]
    fn zip_round_trip_preserves_empty_files() {
        let src = tempfile::tempdir().unwrap();
        let zip_dir = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        write_file(&src.path().join("empty.txt"), b"");
        write_file(&src.path().join("normal.txt"), b"x");

        let zip_path = zip_dir.path().join("game.zip");
        create_zip_from_folder(&src.path().to_string_lossy(), &sp(&zip_path)).unwrap();

        let dst_dir = dst.path().join("extracted");
        extract_zip_to_directory(
            &sp(&zip_path),
            &dst_dir.to_string_lossy(),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(read_file(&dst_dir.join("empty.txt")), b"");
        assert_eq!(read_file(&dst_dir.join("normal.txt")), b"x");
    }

    #[test]
    fn zip_round_trip_preserves_unicode_filenames() {
        let src = tempfile::tempdir().unwrap();
        let zip_dir = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        write_file(&src.path().join("龙之谷.sav"), b"chinese");
        write_file(&src.path().join("Pokémon.dat"), b"accent");
        write_file(&src.path().join("hello world.txt"), b"with space");

        let zip_path = zip_dir.path().join("game.zip");
        create_zip_from_folder(&src.path().to_string_lossy(), &sp(&zip_path)).unwrap();

        let dst_dir = dst.path().join("extracted");
        extract_zip_to_directory(
            &sp(&zip_path),
            &dst_dir.to_string_lossy(),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(read_file(&dst_dir.join("龙之谷.sav")), b"chinese");
        assert_eq!(read_file(&dst_dir.join("Pokémon.dat")), b"accent");
        assert_eq!(read_file(&dst_dir.join("hello world.txt")), b"with space");
    }

    #[test]
    fn zip_round_trip_preserves_binary_content() {
        let src = tempfile::tempdir().unwrap();
        let zip_dir = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        // Datos binarios con todos los bytes 0..255.
        let binary: Vec<u8> = (0..=255u8).collect();
        write_file(&src.path().join("binary.bin"), &binary);

        let zip_path = zip_dir.path().join("game.zip");
        create_zip_from_folder(&src.path().to_string_lossy(), &sp(&zip_path)).unwrap();

        let dst_dir = dst.path().join("extracted");
        extract_zip_to_directory(
            &sp(&zip_path),
            &dst_dir.to_string_lossy(),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(read_file(&dst_dir.join("binary.bin")), binary);
    }

    #[test]
    fn zip_of_empty_folder_succeeds() {
        let src = tempfile::tempdir().unwrap();
        let zip_dir = tempfile::tempdir().unwrap();

        let zip_path = zip_dir.path().join("empty.zip");
        create_zip_from_folder(&src.path().to_string_lossy(), &sp(&zip_path)).unwrap();
        assert!(zip_path.exists(), "empty zip should still be created");
    }

    #[test]
    fn zip_fails_when_source_folder_does_not_exist() {
        let zip_dir = tempfile::tempdir().unwrap();
        let nonexistent = "/nonexistent/path/that/does/not/exist";
        let zip_path = zip_dir.path().join("game.zip");

        let result = create_zip_from_folder(nonexistent, &sp(&zip_path));
        match result {
            Err(SyncError::IoError(_)) => {}
            other => panic!("expected IoError, got {other:?}"),
        }
    }

    // mtime preservation -------------------------------------------------------

    #[test]
    fn zip_mtime_set_to_latest_source_file_mtime() {
        // Memory.md: mtime del ZIP = mtime del save más reciente, así LOCAL status
        // muestra "synced" tras un backup en lugar de "pending_restore".
        let src = tempfile::tempdir().unwrap();
        let zip_dir = tempfile::tempdir().unwrap();

        let f1 = src.path().join("old.txt");
        let f2 = src.path().join("new.txt");
        write_file(&f1, b"old");
        write_file(&f2, b"new");

        // Forzar mtimes específicos: f1 = T-100s, f2 = T (más reciente).
        let now = filetime::FileTime::from_system_time(std::time::SystemTime::now());
        let earlier = filetime::FileTime::from_unix_time(now.unix_seconds() - 100, 0);
        filetime::set_file_mtime(&f1, earlier).unwrap();
        filetime::set_file_mtime(&f2, now).unwrap();

        let zip_path = zip_dir.path().join("game.zip");
        create_zip_from_folder(&src.path().to_string_lossy(), &sp(&zip_path)).unwrap();

        // El mtime del ZIP debe ser el del más reciente (f2 = now).
        let zip_mtime = filetime::FileTime::from_last_modification_time(
            &fs::metadata(&zip_path).unwrap(),
        );
        assert_eq!(
            zip_mtime.unix_seconds(),
            now.unix_seconds(),
            "expected ZIP mtime to match latest source mtime"
        );
    }

    #[test]
    fn extract_with_force_timestamp_sets_mtime_on_extracted_files() {
        let src = tempfile::tempdir().unwrap();
        let zip_dir = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        write_file(&src.path().join("a.txt"), b"a");

        let zip_path = zip_dir.path().join("game.zip");
        create_zip_from_folder(&src.path().to_string_lossy(), &sp(&zip_path)).unwrap();

        let forced = chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let dst_dir = dst.path().join("extracted");
        extract_zip_to_directory(
            &sp(&zip_path),
            &dst_dir.to_string_lossy(),
            Some(forced),
            None,
            None,
        )
        .unwrap();

        let extracted_mtime = filetime::FileTime::from_last_modification_time(
            &fs::metadata(dst_dir.join("a.txt")).unwrap(),
        );
        assert_eq!(extracted_mtime.unix_seconds(), 1_700_000_000);
    }

    // Atomic swap: extracción sobre directorio existente ---------------------

    #[test]
    fn extract_replaces_existing_directory_atomically() {
        let src = tempfile::tempdir().unwrap();
        let zip_dir = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        // Source para el ZIP nuevo.
        write_file(&src.path().join("new1.txt"), b"new content 1");
        write_file(&src.path().join("new2.txt"), b"new content 2");

        let zip_path = zip_dir.path().join("game.zip");
        create_zip_from_folder(&src.path().to_string_lossy(), &sp(&zip_path)).unwrap();

        // El destino YA tiene contenido antiguo distinto.
        let dst_dir = dst.path().join("game-saves");
        write_file(&dst_dir.join("old1.txt"), b"old content 1");
        write_file(&dst_dir.join("old2.txt"), b"old content 2");

        extract_zip_to_directory(
            &sp(&zip_path),
            &dst_dir.to_string_lossy(),
            None,
            None,
            None,
        )
        .unwrap();

        // Los nuevos están.
        assert_eq!(read_file(&dst_dir.join("new1.txt")), b"new content 1");
        assert_eq!(read_file(&dst_dir.join("new2.txt")), b"new content 2");
        // Los viejos ya no.
        assert!(!dst_dir.join("old1.txt").exists(), "old file should be gone");
        assert!(!dst_dir.join("old2.txt").exists(), "old file should be gone");
        // Y los temporales del swap fueron limpiados.
        assert!(
            !Path::new(&format!("{}.ludusavi-tmp", dst_dir.display())).exists(),
            ".ludusavi-tmp leftover"
        );
        assert!(
            !Path::new(&format!("{}.ludusavi-old", dst_dir.display())).exists(),
            ".ludusavi-old leftover"
        );
    }

    #[test]
    fn extract_cleans_up_leftover_tmp_from_previous_failed_run() {
        let src = tempfile::tempdir().unwrap();
        let zip_dir = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();

        write_file(&src.path().join("fresh.txt"), b"fresh");
        let zip_path = zip_dir.path().join("game.zip");
        create_zip_from_folder(&src.path().to_string_lossy(), &sp(&zip_path)).unwrap();

        let dst_dir = dst.path().join("game-saves");
        // Simulamos un .ludusavi-tmp residual de una ejecución anterior fallida.
        let leftover_tmp = dst.path().join("game-saves.ludusavi-tmp");
        fs::create_dir_all(&leftover_tmp).unwrap();
        write_file(&leftover_tmp.join("garbage.txt"), b"junk");

        extract_zip_to_directory(
            &sp(&zip_path),
            &dst_dir.to_string_lossy(),
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(read_file(&dst_dir.join("fresh.txt")), b"fresh");
        assert!(
            !leftover_tmp.exists() || !leftover_tmp.join("garbage.txt").exists(),
            "leftover tmp should have been cleaned"
        );
    }

    // DirectoryScanResult::scan ----------------------------------------------

    #[test]
    fn scan_returns_unset_for_none_path() {
        let r = DirectoryScanResult::scan(None);
        assert!(!r.directory_is_set);
        assert!(!r.directory_exists);
        assert_eq!(r.storage_bytes, 0);
        assert!(r.latest_write_time_utc.is_none());
    }

    #[test]
    fn scan_returns_unset_for_empty_path() {
        let r = DirectoryScanResult::scan(Some(""));
        assert!(!r.directory_is_set);
        let r = DirectoryScanResult::scan(Some("   "));
        assert!(!r.directory_is_set);
    }

    #[test]
    fn scan_returns_set_but_not_exists_for_nonexistent() {
        let r = DirectoryScanResult::scan(Some("/nonexistent/foo/bar/baz"));
        assert!(r.directory_is_set);
        assert!(!r.directory_exists);
    }

    #[test]
    fn scan_returns_correct_size_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("a.txt"), b"hello");      // 5 bytes
        write_file(&dir.path().join("subdir/b.txt"), b"world"); // 5 bytes
        write_file(&dir.path().join("c.dat"), &[0u8; 100]);    // 100 bytes

        let r = DirectoryScanResult::scan(Some(&dir.path().to_string_lossy()));
        assert!(r.directory_is_set);
        assert!(r.directory_exists);
        assert_eq!(r.storage_bytes, 110, "expected 5+5+100");
        assert!(r.latest_write_time_utc.is_some(), "should detect mtime");
    }

    #[test]
    fn scan_uses_latest_mtime_across_files() {
        let dir = tempfile::tempdir().unwrap();
        let f_old = dir.path().join("old.txt");
        let f_new = dir.path().join("new.txt");
        write_file(&f_old, b"old");
        write_file(&f_new, b"new");

        // Forzar timestamps explícitos: old en T-100, new en T.
        let now_ts = filetime::FileTime::from_system_time(std::time::SystemTime::now());
        let earlier = filetime::FileTime::from_unix_time(now_ts.unix_seconds() - 100, 0);
        filetime::set_file_mtime(&f_old, earlier).unwrap();
        filetime::set_file_mtime(&f_new, now_ts).unwrap();

        let r = DirectoryScanResult::scan(Some(&dir.path().to_string_lossy()));
        let detected = r.latest_write_time_utc.unwrap();
        assert_eq!(
            detected.timestamp(),
            now_ts.unix_seconds(),
            "expected scan to pick the newest mtime"
        );
    }

    // Safety backup: create / restore / delete / get_info -------------------

    #[test]
    fn safety_backup_get_info_returns_none_when_missing() {
        let app = tempfile::tempdir().unwrap();
        let info = get_safety_backup_info(&sp(app.path()), "MissingGame");
        assert!(info.is_none());
    }

    #[test]
    fn safety_backup_get_info_returns_size_when_present() {
        let app = tempfile::tempdir().unwrap();
        // Crear un snapshot manualmente con 3 bytes.
        let snapshot = safety_backup_path_for_game(&sp(app.path()), "MyGame");
        let snap_path = snapshot.as_std_path_buf().unwrap();
        fs::create_dir_all(&snap_path).unwrap();
        write_file(&snap_path.join("a.txt"), b"abc");

        let info = get_safety_backup_info(&sp(app.path()), "MyGame").unwrap();
        assert_eq!(info.size_bytes, 3);
    }

    #[test]
    fn delete_safety_backup_is_idempotent() {
        let app = tempfile::tempdir().unwrap();
        // Borrar cuando no existe → no error.
        delete_safety_backup(&sp(app.path()), "Missing").unwrap();

        // Crear y borrar dos veces → segunda vez también OK.
        let snapshot = safety_backup_path_for_game(&sp(app.path()), "MyGame");
        let snap_path = snapshot.as_std_path_buf().unwrap();
        fs::create_dir_all(&snap_path).unwrap();
        write_file(&snap_path.join("a.txt"), b"x");

        delete_safety_backup(&sp(app.path()), "MyGame").unwrap();
        assert!(!snap_path.exists(), "snapshot should be gone");
        delete_safety_backup(&sp(app.path()), "MyGame").unwrap();
    }

    // ============================================================================
    // Tests con rclone real (backend local). Sin red.
    //
    // Usamos el backend "on-the-fly" de rclone (`:local:` prefix), que no requiere
    // ninguna config previa. operations.rs construye paths con format!("{remote_id}:{path}"),
    // así que basta con poner remote_id = ":local" y cloud.path = tempdir absoluto para
    // que rclone escriba/lea de ese tempdir.
    // ============================================================================

    use crate::cloud::Remote;
    use crate::sync::conflict::SyncStatus;

    /// Salta el test si no hay rclone disponible (CI sin la dependencia, etc.).
    /// No usamos `#[ignore]` porque queremos que los tests corran cuando rclone está
    /// instalado y queden silenciados si no.
    fn skip_if_no_rclone() -> bool {
        if which::which("rclone").is_err() {
            eprintln!("[skip] rclone not found in PATH");
            return true;
        }
        false
    }

    /// Entorno de test con un "cloud" simulado en disco local.
    struct RcloneTestEnv {
        cloud: tempfile::TempDir,
        config: Config,
    }

    impl RcloneTestEnv {
        fn new() -> Self {
            let cloud = tempfile::tempdir().unwrap();
            let mut config = Config::default();
            // Backend on-the-fly: ":local:" no requiere registro previo.
            config.cloud.remote = Some(Remote::Custom {
                id: ":local".to_string(),
            });
            config.cloud.path = cloud.path().to_string_lossy().to_string();
            // El default de apps.rclone.path apunta a `which rclone` si está, así
            // que normalmente no hace falta tocarlo. Lo seteamos explícitamente
            // para que is_valid() funcione aunque el default no haya resuelto.
            if !config.apps.rclone.is_valid() {
                let rclone_path = which::which("rclone")
                    .expect("rclone in PATH")
                    .to_string_lossy()
                    .to_string();
                config.apps.rclone.path = StrictPath::new(rclone_path);
            }
            Self { cloud, config }
        }

        fn cloud_path(&self) -> &Path {
            self.cloud.path()
        }
    }

    /// Helper: crea un GameMetaData con path local registrado para `device`.
    fn metadata_with_local_path(id: &str, device_id: &str, local: &str) -> GameMetaData {
        let mut g = GameMetaData::new(id.to_string(), id.to_string());
        g.set_path(device_id.to_string(), local.to_string());
        g
    }

    fn fixed_device(id: &str, name: &str) -> DeviceIdentity {
        DeviceIdentity {
            id: id.to_string(),
            name: name.to_string(),
        }
    }

    // upload_game ---------------------------------------------------------------

    #[test]
    fn upload_game_writes_zip_to_cloud_path() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        let app_dir = tempfile::tempdir().unwrap();
        let saves = tempfile::tempdir().unwrap();

        write_file(&saves.path().join("save.dat"), b"save data");

        let device = fixed_device("device-A", "Test-PC");
        let mut game =
            metadata_with_local_path("StardewValley", &device.id, &saves.path().to_string_lossy());

        upload_game(&env.config, &sp(app_dir.path()), &device, &mut game).unwrap();

        // El ZIP esperado debe existir en el cloud_path.
        let cloud_zip = env
            .cloud_path()
            .join(game_zip_file_name(&game.id));
        assert!(
            cloud_zip.exists(),
            "expected ZIP at {cloud_zip:?} after upload"
        );

        // Y el meta del juego se actualizó tras el upload.
        assert_eq!(game.last_synced_from.as_deref(), Some("device-A"));
        assert!(game.last_sync_time_utc.is_some());
        assert!(game.latest_write_time_utc.is_some());
        assert!(game.storage_bytes > 0);
    }

    #[test]
    fn upload_game_fails_when_no_local_path_for_device() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        let app_dir = tempfile::tempdir().unwrap();
        let device = fixed_device("device-A", "Test-PC");

        // Game con paths registrados en OTRO device, no en device-A.
        let mut game = GameMetaData::new("g1".to_string(), "g1".to_string());
        game.set_path("device-other".to_string(), "/some/path".to_string());

        match upload_game(&env.config, &sp(app_dir.path()), &device, &mut game) {
            Err(SyncError::NoLocalPath) => {}
            other => panic!("expected NoLocalPath, got {other:?}"),
        }
    }

    // download_game -------------------------------------------------------------

    #[test]
    fn download_game_extracts_zip_from_cloud() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        let app_dir = tempfile::tempdir().unwrap();
        let saves_a = tempfile::tempdir().unwrap();
        let saves_b = tempfile::tempdir().unwrap();

        // device-A sube algo al cloud.
        write_file(&saves_a.path().join("save.dat"), b"original from A");
        let device_a = fixed_device("device-A", "PC");
        let mut game =
            metadata_with_local_path("g1", &device_a.id, &saves_a.path().to_string_lossy());
        upload_game(&env.config, &sp(app_dir.path()), &device_a, &mut game).unwrap();

        // device-B baja desde el mismo cloud → debe ver "original from A" en saves_b.
        let device_b = fixed_device("device-B", "Deck");
        // Registramos la ruta local de B en el game.
        game.set_path(device_b.id.clone(), saves_b.path().to_string_lossy().to_string());

        download_game(&env.config, &sp(app_dir.path()), &device_b, &mut game).unwrap();

        let downloaded = saves_b.path().join("save.dat");
        assert!(downloaded.exists(), "expected file at {downloaded:?}");
        assert_eq!(read_file(&downloaded), b"original from A");
    }

    // read_game_list_from_cloud / write_game_list_to_cloud ---------------------

    #[test]
    fn read_game_list_returns_none_when_cloud_empty() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        let result = read_game_list_from_cloud(&env.config);
        assert!(result.is_none(), "empty cloud should return None");
    }

    #[test]
    fn write_then_read_game_list_round_trip() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();

        let mut list = GameListFile::default();
        list.device_names.insert("uuid-pc".into(), "Jayo-PC".into());
        let mut g = GameMetaData::new("g1".to_string(), "Game 1".to_string());
        g.set_path("uuid-pc".to_string(), "/saves/g1".to_string());
        g.storage_bytes = 4242;
        list.upsert_game(g);

        write_game_list_to_cloud(&env.config, &list).unwrap();

        // El fichero debe existir físicamente en el "cloud" tempdir.
        let cloud_file = env.cloud_path().join(GAME_LIST_FILE_NAME);
        assert!(
            cloud_file.exists(),
            "expected game-list at {cloud_file:?}"
        );

        // Y leerlo via read_game_list_from_cloud devuelve los mismos datos.
        let parsed = read_game_list_from_cloud(&env.config).unwrap();
        assert_eq!(parsed.games.len(), 1);
        assert_eq!(parsed.get_game("g1").unwrap().storage_bytes, 4242);
        assert_eq!(parsed.get_device_name("uuid-pc"), "Jayo-PC");
    }

    // get_game_list_mod_time ----------------------------------------------------

    #[test]
    fn get_game_list_mod_time_returns_none_when_missing() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        assert!(get_game_list_mod_time(&env.config).is_none());
    }

    #[test]
    fn get_game_list_mod_time_returns_some_after_write() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        let list = GameListFile::default();
        write_game_list_to_cloud(&env.config, &list).unwrap();
        let mod_time = get_game_list_mod_time(&env.config);
        assert!(
            mod_time.is_some(),
            "expected ModTime after writing, got None"
        );
    }

    // delete_game_zip_from_cloud ------------------------------------------------

    #[test]
    fn delete_game_zip_from_cloud_removes_existing_zip() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        let app_dir = tempfile::tempdir().unwrap();
        let saves = tempfile::tempdir().unwrap();
        write_file(&saves.path().join("a.txt"), b"x");

        let device = fixed_device("device-A", "PC");
        let mut game =
            metadata_with_local_path("g1", &device.id, &saves.path().to_string_lossy());
        upload_game(&env.config, &sp(app_dir.path()), &device, &mut game).unwrap();

        let cloud_zip = env.cloud_path().join(game_zip_file_name(&game.id));
        assert!(cloud_zip.exists(), "ZIP should exist before delete");

        delete_game_zip_from_cloud(&env.config, &game.id).unwrap();
        assert!(!cloud_zip.exists(), "ZIP should be gone after delete");
    }

    #[test]
    fn delete_game_zip_from_cloud_when_missing_does_not_panic() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        // No subimos nada. Borrar un ZIP inexistente no debe explotar.
        let result = delete_game_zip_from_cloud(&env.config, "nonexistent-game");
        // Es OK que sea Ok o Err (rclone deletefile sobre fichero inexistente puede
        // ser cualquiera de las dos según versión); lo importante es que no panic.
        let _ = result;
    }

    // ============================================================================
    // Tests E2E: escenarios del checklist con dos devices simulados.
    //
    // Cada SimulatedDevice tiene su propio app_dir y saves_dir, pero comparten
    // el mismo cloud (RcloneTestEnv). Las operaciones se ejecutan llamando a las
    // funciones públicas del módulo en el orden que el daemon lo haría.
    // ============================================================================

    /// Un dispositivo simulado: identidad propia, app_dir propio, saves_dir propio.
    struct SimulatedDevice {
        identity: DeviceIdentity,
        app_dir: tempfile::TempDir,
        saves_dir: tempfile::TempDir,
    }

    impl SimulatedDevice {
        fn new(id: &str, name: &str) -> Self {
            Self {
                identity: fixed_device(id, name),
                app_dir: tempfile::tempdir().unwrap(),
                saves_dir: tempfile::tempdir().unwrap(),
            }
        }

        fn app_dir(&self) -> StrictPath {
            sp(self.app_dir.path())
        }

        fn saves_path(&self) -> String {
            self.saves_dir.path().to_string_lossy().to_string()
        }

        /// Escribe un fichero de save dentro de saves_dir.
        fn write_save(&self, relative: &str, content: &[u8]) {
            write_file(&self.saves_dir.path().join(relative), content);
        }

        /// Lee un fichero relativo a saves_dir, o panic si no existe.
        fn read_save(&self, relative: &str) -> Vec<u8> {
            read_file(&self.saves_dir.path().join(relative))
        }

        fn save_exists(&self, relative: &str) -> bool {
            self.saves_dir.path().join(relative).is_file()
        }

        /// Construye un GameMetaData con el path local de ESTE device registrado.
        fn register_game(&self, game_id: &str) -> GameMetaData {
            metadata_with_local_path(game_id, &self.identity.id, &self.saves_path())
        }
    }

    /// Computa el SyncStatus que un device vería para un game dado.
    /// Equivalente al cálculo del worker loop del daemon.
    fn sync_status_for(device: &SimulatedDevice, game: &GameMetaData) -> SyncStatus {
        let scan = DirectoryScanResult::scan(Some(&device.saves_path()));
        crate::sync::conflict::determine_sync_type(game, &scan, &device.identity.id)
    }

    // E2E #1: PC → Deck cold start ----------------------------------------------

    #[test]
    fn e2e_pc_to_deck_cold_start() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();

        let pc = SimulatedDevice::new("uuid-pc", "Jayo-PC");
        let deck = SimulatedDevice::new("uuid-deck", "Steam-Deck");

        // PC tiene saves, sube por primera vez.
        pc.write_save("save.dat", b"PC save");
        let mut game = pc.register_game("Stardew");
        upload_game(&env.config, &pc.app_dir(), &pc.identity, &mut game).unwrap();

        // PC escribe el game-list al cloud.
        let mut list = GameListFile::default();
        list.upsert_game(game.clone());
        write_game_list_to_cloud(&env.config, &list).unwrap();

        // Deck arranca: lee game-list del cloud.
        let cloud_list = read_game_list_from_cloud(&env.config).unwrap();
        let mut deck_game = cloud_list.get_game("Stardew").unwrap().clone();
        // Deck registra su path local.
        deck_game.set_path(deck.identity.id.clone(), deck.saves_path());

        // Estado: Deck no tiene saves todavía. Status debe ser RequiresDownload.
        // (Ojo: el directorio del Deck SÍ existe — es el tempdir — pero está vacío
        // así que su mtime es N/A y latest_write < cloud → RequiresDownload por la
        // rama "no last_sync_mtime → fallback a comparación directa".)
        let status = sync_status_for(&deck, &deck_game);
        assert!(
            matches!(status, SyncStatus::RequiresDownload),
            "expected RequiresDownload for cold deck, got {status:?}"
        );

        // Deck baja.
        download_game(&env.config, &deck.app_dir(), &deck.identity, &mut deck_game).unwrap();

        // Deck ahora tiene los saves del PC.
        assert_eq!(deck.read_save("save.dat"), b"PC save");

        // Tras la bajada, el status del Deck debe ser InSync (no RequiresUpload falso).
        let status_after = sync_status_for(&deck, &deck_game);
        assert_eq!(
            status_after,
            SyncStatus::InSync,
            "expected InSync after download"
        );
    }

    // E2E #2: No download redundante al reiniciar ------------------------------

    #[test]
    fn e2e_no_redundant_download_after_restart() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        let pc = SimulatedDevice::new("uuid-pc", "PC");

        pc.write_save("save.dat", b"hello");
        let mut game = pc.register_game("g1");
        upload_game(&env.config, &pc.app_dir(), &pc.identity, &mut game).unwrap();

        // Tras subir, el propio PC NO debe necesitar nada (InSync).
        let status = sync_status_for(&pc, &game);
        assert_eq!(status, SyncStatus::InSync);
    }

    // E2E #3: Conflicto detectado entre dos devices ----------------------------

    #[test]
    fn e2e_conflict_detected_when_both_modified_independently() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();

        let pc = SimulatedDevice::new("uuid-pc", "PC");
        let deck = SimulatedDevice::new("uuid-deck", "Deck");

        // PC sube primero.
        pc.write_save("save.dat", b"v1");
        let mut game = pc.register_game("g1");
        upload_game(&env.config, &pc.app_dir(), &pc.identity, &mut game).unwrap();
        let mut list = GameListFile::default();
        list.upsert_game(game.clone());
        write_game_list_to_cloud(&env.config, &list).unwrap();

        // Deck baja, ya está sincronizado.
        let cloud_list = read_game_list_from_cloud(&env.config).unwrap();
        let mut deck_game = cloud_list.get_game("g1").unwrap().clone();
        deck_game.set_path(deck.identity.id.clone(), deck.saves_path());
        download_game(&env.config, &deck.app_dir(), &deck.identity, &mut deck_game).unwrap();

        // Ahora ambos modifican localmente sin sincronizar.
        // Necesitamos forzar mtimes nuevos POR ENCIMA del last_sync_mtime persistido.
        let future =
            filetime::FileTime::from_unix_time(chrono::Utc::now().timestamp() + 3600, 0);

        // PC modifica.
        std::thread::sleep(std::time::Duration::from_millis(10));
        pc.write_save("save.dat", b"PC modified");
        filetime::set_file_mtime(&pc.saves_dir.path().join("save.dat"), future).unwrap();

        // Cloud "se entera" del cambio del Deck (que también modificó). Para simular
        // esto sin volver a subir: editamos directamente latest_write_time_utc de
        // game (que vive en el cloud según el game-list) para que sea > last_sync.
        let cloud_future = chrono::Utc.timestamp_opt(future.unix_seconds(), 0).unwrap();
        deck_game.latest_write_time_utc = Some(cloud_future);
        // Y last_synced_from = otro device (Deck), no PC, para que no se aplique
        // la excepción "cloud was uploaded from this device".
        deck_game.last_synced_from = Some(deck.identity.id.clone());

        // El PC ve que: (a) sus saves locales son más nuevos que su last_sync_mtime
        // (porque acaba de modificar) y (b) el cloud también es más nuevo y vino
        // de OTRO device → CONFLICT.
        let mut pc_view = deck_game.clone();
        // Restauramos el path de PC (el game viene de cloud_list, así que solo tiene
        // el path del Deck en path_by_device tras download).
        pc_view.set_path(pc.identity.id.clone(), pc.saves_path());
        // Y conservamos el last_sync_mtime que PC tenía tras subir.
        pc_view.set_last_sync_mtime(&pc.identity.id, game.latest_write_time_utc.unwrap());

        let status = sync_status_for(&pc, &pc_view);
        assert!(
            matches!(status, SyncStatus::Conflict { .. }),
            "expected Conflict, got {status:?}"
        );
    }

    // E2E #4: Re-upload del mismo device NO produce conflict -------------------

    #[test]
    fn e2e_same_device_reupload_does_not_conflict() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        let pc = SimulatedDevice::new("uuid-pc", "PC");

        pc.write_save("save.dat", b"v1");
        let mut game = pc.register_game("g1");
        upload_game(&env.config, &pc.app_dir(), &pc.identity, &mut game).unwrap();

        // PC modifica y "el cloud avanza". Para que el cloud parezca "más nuevo
        // que last_sync_mtime de PC" necesitamos que game.latest_write_time_utc avance.
        // Eso ocurre realmente al subir de nuevo, así que llamamos upload_game otra vez.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        pc.write_save("save.dat", b"v2");
        let future =
            filetime::FileTime::from_unix_time(chrono::Utc::now().timestamp() + 60, 0);
        filetime::set_file_mtime(&pc.saves_dir.path().join("save.dat"), future).unwrap();

        // Tras un primer sync_status, el PC ve "local cambió, cloud no" → RequiresUpload.
        let status = sync_status_for(&pc, &game);
        assert!(
            matches!(status, SyncStatus::RequiresUpload),
            "expected RequiresUpload, got {status:?}"
        );

        // PC sube de nuevo. Como el cloud lo subió ESTE mismo device, no hay conflict
        // aunque ambos timestamps avancen.
        upload_game(&env.config, &pc.app_dir(), &pc.identity, &mut game).unwrap();

        // Tras el segundo upload, status InSync.
        let status_after = sync_status_for(&pc, &game);
        assert_eq!(status_after, SyncStatus::InSync);
    }

    // E2E #5: Limpieza de ZIP del cloud al cambiar de modo --------------------

    #[test]
    fn e2e_delete_cloud_zip_when_switching_to_local() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        let pc = SimulatedDevice::new("uuid-pc", "PC");

        // Modo CLOUD (o SYNC): subimos.
        pc.write_save("save.dat", b"data");
        let mut game = pc.register_game("g1");
        upload_game(&env.config, &pc.app_dir(), &pc.identity, &mut game).unwrap();
        let cloud_zip = env.cloud_path().join(game_zip_file_name("g1"));
        assert!(cloud_zip.exists(), "ZIP should exist in cloud after upload");

        // Cambio de modo CLOUD/SYNC → LOCAL/NONE: limpia el ZIP del cloud.
        delete_game_zip_from_cloud(&env.config, "g1").unwrap();
        assert!(!cloud_zip.exists(), "ZIP should be gone after mode switch");
    }

    // E2E #6: Round-trip de un mtime forzado durante download ------------------

    /// Memoria del fork: download_game pasa game.latest_write_time_utc como
    /// force_last_write_time a extract_zip_to_directory, así que el mtime de los
    /// archivos extraídos coincide con el mtime que el cloud reportó. Esto es
    /// crítico para que el siguiente determine_sync_type devuelva InSync en vez
    /// de un falso RequiresUpload por nanos distintos.
    #[test]
    fn e2e_download_preserves_mtime_for_in_sync_status() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();

        let pc = SimulatedDevice::new("uuid-pc", "PC");
        let deck = SimulatedDevice::new("uuid-deck", "Deck");

        pc.write_save("save.dat", b"x");
        let mut game = pc.register_game("g1");
        upload_game(&env.config, &pc.app_dir(), &pc.identity, &mut game).unwrap();
        let cloud_mtime = game.latest_write_time_utc.unwrap().timestamp();

        // Deck baja.
        let mut deck_game = game.clone();
        deck_game.set_path(deck.identity.id.clone(), deck.saves_path());
        download_game(&env.config, &deck.app_dir(), &deck.identity, &mut deck_game).unwrap();

        // El mtime del fichero extraído debe coincidir (a segundos) con el del cloud.
        let extracted_mtime = filetime::FileTime::from_last_modification_time(
            &fs::metadata(deck.saves_dir.path().join("save.dat")).unwrap(),
        );
        assert_eq!(
            extracted_mtime.unix_seconds(),
            cloud_mtime,
            "extracted mtime should match cloud's latest_write_time_utc"
        );

        // Y el SyncStatus subsiguiente debe ser InSync.
        let status = sync_status_for(&deck, &deck_game);
        assert_eq!(status, SyncStatus::InSync);
    }

    // E2E #7: Save modes — None ignora; Local backup queda en disco --------

    #[test]
    fn e2e_local_backup_creates_zip_locally_without_touching_cloud() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();
        let pc = SimulatedDevice::new("uuid-pc", "PC");

        pc.write_save("save.dat", b"local only");

        // Modo LOCAL: el ZIP se escribe en backup.path, NO en el cloud.
        // El daemon haría: create_zip_from_folder(saves, backup_path/game-id.zip).
        // Verificamos que esa función concreta NO toca el cloud.
        let backup_dir = tempfile::tempdir().unwrap();
        let zip_path = sp(&backup_dir.path().join("game-g1.zip"));
        create_zip_from_folder(&pc.saves_path(), &zip_path).unwrap();

        // El ZIP local existe.
        assert!(backup_dir.path().join("game-g1.zip").exists());
        // El cloud sigue vacío.
        assert!(
            std::fs::read_dir(env.cloud_path()).unwrap().next().is_none(),
            "cloud should be empty in LOCAL mode"
        );
    }

    // E2E #8: Cloud vacío + game-list ausente → comportamiento robusto -------

    #[test]
    fn e2e_first_run_no_game_list_in_cloud() {
        if skip_if_no_rclone() {
            return;
        }
        let env = RcloneTestEnv::new();

        // Antes de cualquier sync: cloud vacío, no hay game-list.
        assert!(read_game_list_from_cloud(&env.config).is_none());
        assert!(get_game_list_mod_time(&env.config).is_none());

        // El daemon escribe un game-list vacío como bootstrap.
        let empty = GameListFile::default();
        write_game_list_to_cloud(&env.config, &empty).unwrap();

        // Ahora sí existe.
        let parsed = read_game_list_from_cloud(&env.config).unwrap();
        assert!(parsed.games.is_empty());
        assert!(get_game_list_mod_time(&env.config).is_some());
    }

    // ============================================================================
    // Tests adicionales: extract_root_from_scan, safety backups extra, keep-both
    // ============================================================================

    use crate::scan::ScannedFile;

    fn scanned(ignored: bool) -> ScannedFile {
        ScannedFile {
            size: 1,
            hash: "h".to_string(),
            original_path: None,
            ignored,
            change: Default::default(),
            container: None,
        }
    }

    // extract_root_from_scan ---------------------------------------------------

    #[test]
    fn extract_root_returns_none_for_empty() {
        let found: std::collections::HashMap<StrictPath, ScannedFile> =
            std::collections::HashMap::new();
        assert_eq!(extract_root_from_scan(&found), None);
    }

    #[test]
    fn extract_root_returns_parent_for_single_file() {
        let mut found = std::collections::HashMap::new();
        found.insert(
            StrictPath::new("/home/jayo/saves/game1/save.dat".to_string()),
            scanned(false),
        );
        let root = extract_root_from_scan(&found).unwrap();
        assert!(
            root.eq_ignore_ascii_case("/home/jayo/saves/game1"),
            "got {root:?}"
        );
    }

    #[test]
    fn extract_root_skips_ignored_files() {
        let mut found = std::collections::HashMap::new();
        found.insert(
            StrictPath::new("/home/jayo/IGNORED/foo.dat".to_string()),
            scanned(true),
        );
        found.insert(
            StrictPath::new("/home/jayo/saves/game1/save.dat".to_string()),
            scanned(false),
        );
        let root = extract_root_from_scan(&found).unwrap();
        // Solo el no-ignorado cuenta.
        assert!(root.contains("game1"), "got {root:?}");
    }

    #[test]
    fn extract_root_finds_common_parent_of_multiple_files() {
        let mut found = std::collections::HashMap::new();
        found.insert(
            StrictPath::new("/home/jayo/saves/game1/save1.dat".to_string()),
            scanned(false),
        );
        found.insert(
            StrictPath::new("/home/jayo/saves/game1/save2.dat".to_string()),
            scanned(false),
        );
        let root = extract_root_from_scan(&found).unwrap();
        assert!(root.contains("game1"), "got {root:?}");
    }

    #[test]
    fn extract_root_returns_none_when_all_ignored() {
        let mut found = std::collections::HashMap::new();
        found.insert(
            StrictPath::new("/foo/a.dat".to_string()),
            scanned(true),
        );
        found.insert(
            StrictPath::new("/bar/b.dat".to_string()),
            scanned(true),
        );
        assert_eq!(extract_root_from_scan(&found), None);
    }

    // create_safety_backup edge cases -----------------------------------------

    #[test]
    fn safety_backup_skips_when_source_missing() {
        let app = tempfile::tempdir().unwrap();
        // save_path no existe.
        let result = create_safety_backup(&sp(app.path()), "g1", "/nonexistent/path");
        assert!(result.is_ok(), "should silently skip, got {result:?}");
        let info = get_safety_backup_info(&sp(app.path()), "g1");
        assert!(info.is_none(), "no snapshot should have been created");
    }

    #[test]
    fn safety_backup_skips_when_source_empty() {
        let app = tempfile::tempdir().unwrap();
        let saves = tempfile::tempdir().unwrap();
        // saves dir exists but is empty.
        let result = create_safety_backup(
            &sp(app.path()),
            "g1",
            &saves.path().to_string_lossy(),
        );
        assert!(result.is_ok());
        assert!(get_safety_backup_info(&sp(app.path()), "g1").is_none());
    }

    #[test]
    fn safety_backup_creates_snapshot_when_source_has_files() {
        let app = tempfile::tempdir().unwrap();
        let saves = tempfile::tempdir().unwrap();
        write_file(&saves.path().join("save.dat"), b"hello");

        create_safety_backup(
            &sp(app.path()),
            "MyGame",
            &saves.path().to_string_lossy(),
        )
        .unwrap();

        let snapshot = safety_backup_path_for_game(&sp(app.path()), "MyGame");
        let snap_path = snapshot.as_std_path_buf().unwrap();
        assert!(snap_path.is_dir(), "snapshot dir should exist");
        assert_eq!(read_file(&snap_path.join("save.dat")), b"hello");
    }

    #[test]
    fn safety_backup_overwrites_previous_snapshot() {
        let app = tempfile::tempdir().unwrap();
        let saves = tempfile::tempdir().unwrap();

        // Crear primer snapshot.
        write_file(&saves.path().join("v1.dat"), b"v1");
        create_safety_backup(&sp(app.path()), "g1", &saves.path().to_string_lossy()).unwrap();

        // Modificar saves: borrar v1, crear v2.
        std::fs::remove_file(saves.path().join("v1.dat")).unwrap();
        write_file(&saves.path().join("v2.dat"), b"v2");

        // Crear segundo snapshot — debe sobreescribir el primero.
        create_safety_backup(&sp(app.path()), "g1", &saves.path().to_string_lossy()).unwrap();

        let snapshot = safety_backup_path_for_game(&sp(app.path()), "g1");
        let snap_path = snapshot.as_std_path_buf().unwrap();
        assert!(!snap_path.join("v1.dat").exists(), "v1 should be gone");
        assert!(snap_path.join("v2.dat").exists(), "v2 should be present");
    }

    // restore_safety_backup ---------------------------------------------------

    #[test]
    fn restore_safety_backup_fails_when_no_snapshot() {
        let app = tempfile::tempdir().unwrap();
        let saves = tempfile::tempdir().unwrap();
        let result = restore_safety_backup(
            &sp(app.path()),
            "missing-game",
            &saves.path().to_string_lossy(),
        );
        match result {
            Err(SyncError::IoError(_)) => {}
            other => panic!("expected IoError, got {other:?}"),
        }
    }

    #[test]
    fn restore_safety_backup_swaps_in_snapshot_contents() {
        let app = tempfile::tempdir().unwrap();
        let saves = tempfile::tempdir().unwrap();

        // 1. Saves originales: v_original.
        write_file(&saves.path().join("v_original.dat"), b"original");

        // 2. Crear safety backup.
        create_safety_backup(&sp(app.path()), "g1", &saves.path().to_string_lossy()).unwrap();

        // 3. Algo "destructivo": cambiar el contenido de saves.
        std::fs::remove_file(saves.path().join("v_original.dat")).unwrap();
        write_file(&saves.path().join("v_corrupted.dat"), b"oops");

        // 4. Restaurar safety backup.
        restore_safety_backup(&sp(app.path()), "g1", &saves.path().to_string_lossy()).unwrap();

        // 5. Verificar que v_original volvió y v_corrupted desapareció.
        assert!(saves.path().join("v_original.dat").exists());
        assert_eq!(read_file(&saves.path().join("v_original.dat")), b"original");
        assert!(!saves.path().join("v_corrupted.dat").exists());
    }

    // create_keep_both_snapshot -----------------------------------------------

    #[test]
    fn keep_both_snapshot_fails_when_source_missing() {
        let app = tempfile::tempdir().unwrap();
        let result = create_keep_both_snapshot(&sp(app.path()), "g1", "/nonexistent");
        match result {
            Err(SyncError::IoError(_)) => {}
            other => panic!("expected IoError, got {other:?}"),
        }
    }

    #[test]
    fn keep_both_snapshot_creates_timestamped_dir_with_contents() {
        let app = tempfile::tempdir().unwrap();
        let saves = tempfile::tempdir().unwrap();
        write_file(&saves.path().join("save.dat"), b"content");

        let snap_path = create_keep_both_snapshot(
            &sp(app.path()),
            "MyGame",
            &saves.path().to_string_lossy(),
        )
        .unwrap();

        let snap_std = snap_path.as_std_path_buf().unwrap();
        assert!(snap_std.is_dir(), "snapshot dir should exist");
        assert_eq!(read_file(&snap_std.join("save.dat")), b"content");

        // El nombre incluye "keep-both-" + timestamp.
        let name = snap_std.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("keep-both-"), "name was {name:?}");
    }

    #[test]
    fn keep_both_snapshots_can_coexist() {
        // Llamadas consecutivas deben crear directorios distintos (timestamps).
        let app = tempfile::tempdir().unwrap();
        let saves = tempfile::tempdir().unwrap();
        write_file(&saves.path().join("a.dat"), b"a");

        let s1 = create_keep_both_snapshot(&sp(app.path()), "g1", &saves.path().to_string_lossy())
            .unwrap();
        // Esperamos 1100ms para asegurar timestamp distinto (granularidad de segundos).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_file(&saves.path().join("b.dat"), b"b");
        let s2 = create_keep_both_snapshot(&sp(app.path()), "g1", &saves.path().to_string_lossy())
            .unwrap();

        let p1 = s1.as_std_path_buf().unwrap();
        let p2 = s2.as_std_path_buf().unwrap();
        assert_ne!(p1, p2, "two snapshots should have different paths");
        assert!(p1.exists() && p2.exists());
        assert!(p2.join("b.dat").exists(), "second snapshot has the new file");
    }
}
