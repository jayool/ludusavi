use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Nombre del fichero JSON que se sube al cloud junto con los zips.
/// Equivalente a StorageConstants.FileName_GameList en EmuSync.
pub const GAME_LIST_FILE_NAME: &str = "ludusavi-game-list.json";

/// Nombre del fichero zip por juego en el cloud.
/// Equivalente a StorageConstants.FileName_GameZip en EmuSync.
/// El parámetro es el ID del juego (nombre del juego en Ludusavi).
pub fn game_zip_file_name(game_id: &str) -> String {
    format!("game-{}.zip", game_id)
}

/// Entrada de `path_by_device`: ruta local del juego en un dispositivo concreto,
/// junto con el último mtime que este dispositivo vio del cloud tras un sync exitoso.
///
/// `last_sync_mtime` se usa para detectar conflictos: si el local cambió DESDE
/// `last_sync_mtime` Y el cloud también cambió desde entonces, ambos devices
/// han modificado independientemente y el usuario debe decidir qué conservar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevicePathEntry {
    pub path: String,
    #[serde(default)]
    pub last_sync_mtime: Option<DateTime<Utc>>,
}

impl DevicePathEntry {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            last_sync_mtime: None,
        }
    }
}

/// El JSON que se sube al cloud con los metadatos de todos los juegos.
/// Equivalente a GameListFile en EmuSync.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameListFile {
    pub games: Vec<GameMetaData>,
    #[serde(default)]
    pub device_names: HashMap<String, String>,
}

impl GameListFile {
    pub fn get_game(&self, id: &str) -> Option<&GameMetaData> {
        self.games.iter().find(|g| g.id == id)
    }

    pub fn get_device_name<'a>(&'a self, device_id: &'a str) -> &'a str {
        self.device_names
            .get(device_id)
            .map(|s| s.as_str())
            .unwrap_or(device_id)
    }

    pub fn get_game_mut(&mut self, id: &str) -> Option<&mut GameMetaData> {
        self.games.iter_mut().find(|g| g.id == id)
    }

    pub fn upsert_game(&mut self, game: GameMetaData) {
        if let Some(existing) = self.games.iter_mut().find(|g| g.id == game.id) {
            *existing = game;
        } else {
            self.games.push(game);
        }
    }
}

/// Metadatos de un juego en el cloud.
/// Equivalente a GameMetaData en EmuSync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameMetaData {
    /// ID único del juego. En Ludusavi usamos el nombre del juego como ID,
    /// ya que es el identificador principal.
    pub id: String,
    /// Nombre del juego.
    pub name: String,
    /// Mapa de device_id -> entrada con path y estado de sync.
    /// Equivalente a SyncSourceIdLocations en EmuSync, extendido con last_sync_mtime
    /// para detectar conflictos.
    pub path_by_device: HashMap<String, DevicePathEntry>,
    /// ID del dispositivo que hizo el último sync.
    /// Equivalente a LastSyncedFrom en EmuSync.
    pub last_synced_from: Option<String>,
    /// Cuándo se hizo el último sync (UTC).
    /// Equivalente a LastSyncTimeUtc en EmuSync.
    pub last_sync_time_utc: Option<DateTime<Utc>>,
    /// Latest write time de los ficheros de save en el momento del último sync.
    /// Equivalente a LatestWriteTimeUtc en EmuSync.
    /// Se usa para detectar si la versión local o la del cloud es más nueva.
    pub latest_write_time_utc: Option<DateTime<Utc>>,
    /// Tamaño en bytes del último backup.
    /// Equivalente a StorageBytes en EmuSync.
    pub storage_bytes: u64,
}

impl GameMetaData {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        let id = id.into();
        let name = name.into();
        Self {
            id,
            name,
            path_by_device: HashMap::new(),
            last_synced_from: None,
            last_sync_time_utc: None,
            latest_write_time_utc: None,
            storage_bytes: 0,
        }
    }

    /// Helper: obtener solo el path (para callers que no necesitan el mtime).
    pub fn get_path(&self, device_id: &str) -> Option<&str> {
        self.path_by_device.get(device_id).map(|e| e.path.as_str())
    }

    /// Helper: obtener el last_sync_mtime de un device.
    pub fn get_last_sync_mtime(&self, device_id: &str) -> Option<DateTime<Utc>> {
        self.path_by_device
            .get(device_id)
            .and_then(|e| e.last_sync_mtime)
    }

    /// Helper: registrar o actualizar el path de un device (preserva last_sync_mtime si ya había).
    pub fn set_path(&mut self, device_id: impl Into<String>, path: impl Into<String>) {
        let device_id = device_id.into();
        let path = path.into();
        match self.path_by_device.get_mut(&device_id) {
            Some(entry) => {
                entry.path = path;
            }
            None => {
                self.path_by_device.insert(device_id, DevicePathEntry::new(path));
            }
        }
    }

    /// Helper: actualizar el last_sync_mtime de un device tras un sync exitoso.
    pub fn set_last_sync_mtime(&mut self, device_id: &str, mtime: DateTime<Utc>) {
        if let Some(entry) = self.path_by_device.get_mut(device_id) {
            entry.last_sync_mtime = Some(mtime);
        }
    }
}
