use crate::sync::game_list::GameMetaData;
use chrono::{DateTime, Utc};

/// Estado de sincronización de un juego.
/// Equivalente a GameSyncStatus en EmuSync, extendido con Conflict.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Local y cloud han cambiado AMBOS desde el último sync conocido.
    /// El usuario debe decidir qué versión conservar.
    Conflict {
        local_time: DateTime<Utc>,
        cloud_time: DateTime<Utc>,
        /// device_id de quien subió la versión actual del cloud
        cloud_from: Option<String>,
    },
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
///
/// Lógica:
/// 1. Si nunca se ha sincronizado en el cloud (`game.last_sync_time_utc == None`):
///    - Si hay directorio local → upload
///    - Si no hay directorio → unknown
/// 2. Si no hay path local configurado para este device → UnsetDirectory
/// 3. Si el directorio local no existe pero el cloud sí → download
/// 4. Caso normal: comparar local, cloud y last_sync_mtime de este device para decidir.
///    Si this_device no tiene last_sync_mtime, asumimos primera vez (sin conflict).
pub fn determine_sync_type(
    game: &GameMetaData,
    scan_result: &DirectoryScanResult,
    this_device_id: &str,
) -> SyncStatus {
    // Nunca se ha sincronizado antes (cloud vacío para este juego)
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
    let cloud_latest = game.latest_write_time_utc.unwrap_or(DateTime::<Utc>::MIN_UTC);

    // Truncar a segundos para evitar falsos positivos por diferencias de precisión
    // entre filesystems (Windows trunca nanosegundos al leer mtime).
    let scan_secs = scan_latest.timestamp();
    let cloud_secs = cloud_latest.timestamp();

    // Si el cloud subió desde este propio device, no puede haber conflict.
    // Comparamos timestamps directamente.
    let last_synced_from = game.last_synced_from.as_deref();
    let cloud_uploaded_from_this_device = last_synced_from == Some(this_device_id);

    // Obtener last_sync_mtime de este device — referencia para detectar conflict.
    let last_sync_mtime = game.get_last_sync_mtime(this_device_id);

    // Si no tenemos last_sync_mtime para este device, no podemos detectar conflict
    // de forma fiable. Asumimos primera vez y comparamos timestamps puros.
    if last_sync_mtime.is_none() {
        log::debug!(
            "[{}] No last_sync_mtime for this device - falling back to direct timestamp comparison",
            game.name
        );
        if scan_secs > cloud_secs {
            return SyncStatus::RequiresUpload;
        } else if scan_secs < cloud_secs {
            return SyncStatus::RequiresDownload;
        } else {
            return SyncStatus::InSync;
        }
    }

    // Caminamos la tabla de verdad con last_sync_mtime como referencia.
    let last_sync_secs = last_sync_mtime.unwrap().timestamp();

    let local_changed = scan_secs > last_sync_secs;
    let cloud_changed = cloud_secs > last_sync_secs;

    log::debug!(
        "[{}] local_changed={} cloud_changed={} (scan={} cloud={} last_sync={})",
        game.name,
        local_changed,
        cloud_changed,
        scan_secs,
        cloud_secs,
        last_sync_secs
    );

    match (local_changed, cloud_changed) {
        (false, false) => {
            log::debug!("[{}] Local and cloud unchanged since last sync - in sync", game.name);
            SyncStatus::InSync
        }
        (true, false) => {
            log::debug!("[{}] Only local changed since last sync - upload", game.name);
            SyncStatus::RequiresUpload
        }
        (false, true) => {
            log::debug!("[{}] Only cloud changed since last sync - download", game.name);
            SyncStatus::RequiresDownload
        }
        (true, true) => {
            // Caso especial: si el cloud lo subió este mismo device, no hay conflict real.
            // Sólo significa que tras subir hicimos más cambios → upload normal.
            if cloud_uploaded_from_this_device {
                log::debug!(
                    "[{}] Both changed but cloud was uploaded from this device - upload",
                    game.name
                );
                return SyncStatus::RequiresUpload;
            }
            log::warn!(
                "[{}] CONFLICT: both local and cloud changed since last sync (local={}, cloud={}, from={:?})",
                game.name,
                scan_latest,
                cloud_latest,
                last_synced_from
            );
            SyncStatus::Conflict {
                local_time: scan_latest,
                cloud_time: cloud_latest,
                cloud_from: last_synced_from.map(|s| s.to_string()),
            }
        }
    }
}
