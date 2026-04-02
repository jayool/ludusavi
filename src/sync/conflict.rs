use chrono::{DateTime, Utc};
use crate::sync::game_list::GameMetaData;

/// Estado de sincronización de un juego.
/// Equivalente a GameSyncStatus en EmuSync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    /// No se puede determinar el estado.
    Unknown,
    /// Los ficheros locales son más nuevos que el cloud → hay que subir.
    RequiresUpload,
    /// El cloud es más nuevo que los ficheros locales → hay que bajar.
    RequiresDownload,
    /// Local y cloud están sincronizados.
    InSync,
    /// No hay ruta local configurada para este dispositivo.
    UnsetDirectory,
}

/// Resultado del escaneo del directorio local.
/// Equivalente a DirectoryScanResult en EmuSync.
#[derive(Debug, Clone)]
pub struct DirectoryScanResult {
    pub directory_is_set: bool,
    pub directory_exists: bool,
    pub latest_write_time_utc: Option<DateTime<Utc>>,
    pub storage_bytes: u64,
}

impl DirectoryScanResult {
    /// Escanea un directorio y devuelve información sobre él.
    /// Equivalente a LocalDataAccessor.ScanDirectory en EmuSync.
    pub fn scan(path: Option<&str>) -> Self {
        let Some(path) = path else {
            return Self {
                directory_is_set: false,
                directory_exists: false,
                latest_write_time_utc: None,
                storage_bytes: 0,
            };
        };

        if path.trim().is_empty() {
            return Self {
                directory_is_set: false,
                directory_exists: false,
                latest_write_time_utc: None,
                storage_bytes: 0,
            };
        }

        let std_path = std::path::Path::new(path);
        let directory_exists = std_path.is_dir();

        if !directory_exists {
            return Self {
                directory_is_set: true,
                directory_exists: false,
                latest_write_time_utc: None,
                storage_bytes: 0,
            };
        }

        let mut latest_write_time: Option<DateTime<Utc>> = None;
        let mut storage_bytes: u64 = 0;

        for entry in walkdir::WalkDir::new(std_path)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                storage_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);

                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        let modified: DateTime<Utc> = modified.into();
                        match latest_write_time {
                            None => latest_write_time = Some(modified),
                            Some(current) if modified > current => latest_write_time = Some(modified),
                            _ => {}
                        }
                    }
                }
            }
        }
        Self {
            directory_is_set: true,
            directory_exists: true,
            latest_write_time_utc: latest_write_time,
            storage_bytes,
        }
    }
}

/// Determina qué tipo de sync necesita un juego.
/// Traducción exacta de DetermineSyncType en GameSyncManager.cs de EmuSync.
pub fn determine_sync_type(
    game: &GameMetaData,
    scan_result: &DirectoryScanResult,
) -> SyncStatus {
    // Nunca se ha sincronizado antes
    if game.last_sync_time_utc.is_none() {
        if scan_result.directory_exists {
            log::debug!("[{}] No cloud sync exists - game should be uploaded", game.name);
            return SyncStatus::RequiresUpload;
        }
        log::debug!("[{}] No local files or directories found to upload", game.name);
        return SyncStatus::Unknown;
    }

    // No hay directorio local configurado
    if !scan_result.directory_is_set {
        log::debug!("[{}] No local directory is set - unknown sync status", game.name);
        return SyncStatus::UnsetDirectory;
    }

    // El directorio cloud existe pero el local no → hay que bajar
    if !scan_result.directory_exists && game.last_sync_time_utc.is_some() {
        log::debug!("[{}] No local directory found - game should be downloaded", game.name);
        return SyncStatus::RequiresDownload;
    }

    let scan_latest = scan_result.latest_write_time_utc.unwrap_or(DateTime::<Utc>::MIN_UTC);
    let game_latest = game.latest_write_time_utc.unwrap_or(DateTime::<Utc>::MIN_UTC);

    // Truncar a segundos para evitar falsos positivos por diferencias de precisión
    // entre filesystems (Windows trunca nanosegundos al leer mtime)
    let scan_secs = scan_latest.timestamp();
    let game_secs = game_latest.timestamp();
    
    if scan_secs > game_secs {
        log::debug!("[{}] Local version is newer - game should be uploaded", game.name);
        SyncStatus::RequiresUpload
    } else if scan_secs < game_secs {
        log::debug!("[{}] Cloud version is newer - game should be downloaded", game.name);
        SyncStatus::RequiresDownload
    } else {
        log::debug!("[{}] Local version is in sync with cloud version", game.name);
        SyncStatus::InSync
    }
}
