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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    // game_zip_file_name ----------------------------------------------------------

    #[test]
    fn game_zip_file_name_uses_game_id() {
        assert_eq!(game_zip_file_name("Stardew Valley"), "game-Stardew Valley.zip");
        assert_eq!(game_zip_file_name(""), "game-.zip");
    }

    // GameMetaData::set_path / get_path ------------------------------------------

    #[test]
    fn set_path_creates_entry_with_no_mtime() {
        let mut g = GameMetaData::new("game1", "Game 1");
        g.set_path("device-A", "/home/user/saves/game1");
        assert_eq!(g.get_path("device-A"), Some("/home/user/saves/game1"));
        assert_eq!(g.get_last_sync_mtime("device-A"), None);
    }

    #[test]
    fn set_path_updates_existing_path_preserving_mtime() {
        let mut g = GameMetaData::new("game1", "Game 1");
        g.set_path("device-A", "/old/path");
        g.set_last_sync_mtime("device-A", ts(123));
        // Cambiar el path tras un sync no debe perder el mtime registrado.
        g.set_path("device-A", "/new/path");
        assert_eq!(g.get_path("device-A"), Some("/new/path"));
        assert_eq!(g.get_last_sync_mtime("device-A"), Some(ts(123)));
    }

    #[test]
    fn set_last_sync_mtime_no_op_when_device_not_registered() {
        let mut g = GameMetaData::new("game1", "Game 1");
        // Sin set_path previo, set_last_sync_mtime debe ser no-op (no panic).
        g.set_last_sync_mtime("device-unknown", ts(123));
        assert_eq!(g.get_last_sync_mtime("device-unknown"), None);
    }

    #[test]
    fn get_path_returns_none_for_unknown_device() {
        let g = GameMetaData::new("game1", "Game 1");
        assert_eq!(g.get_path("nope"), None);
    }

    // GameListFile::upsert_game --------------------------------------------------

    #[test]
    fn upsert_game_inserts_new() {
        let mut list = GameListFile::default();
        list.upsert_game(GameMetaData::new("game1", "Game 1"));
        assert_eq!(list.games.len(), 1);
        assert!(list.get_game("game1").is_some());
    }

    #[test]
    fn upsert_game_updates_existing_in_place() {
        let mut list = GameListFile::default();
        list.upsert_game(GameMetaData::new("game1", "Game 1"));
        list.upsert_game(GameMetaData::new("game2", "Game 2"));
        // Re-upsert con datos modificados de game1.
        let mut updated = GameMetaData::new("game1", "Game 1");
        updated.storage_bytes = 9999;
        list.upsert_game(updated);
        assert_eq!(list.games.len(), 2);
        assert_eq!(list.get_game("game1").unwrap().storage_bytes, 9999);
    }

    // GameListFile::get_device_name ----------------------------------------------

    #[test]
    fn get_device_name_returns_friendly_name() {
        let mut list = GameListFile::default();
        list.device_names
            .insert("uuid-aaa".into(), "Jayo-PC".into());
        assert_eq!(list.get_device_name("uuid-aaa"), "Jayo-PC");
    }

    #[test]
    fn get_device_name_falls_back_to_uuid_when_unknown() {
        let list = GameListFile::default();
        assert_eq!(list.get_device_name("uuid-unknown"), "uuid-unknown");
    }

    // Round-trip JSON ------------------------------------------------------------

    #[test]
    fn round_trip_full_game_list() {
        let mut list = GameListFile::default();
        list.device_names
            .insert("uuid-pc".into(), "Jayo-PC".into());
        let mut g = GameMetaData::new("game1", "Game 1");
        g.set_path("uuid-pc", "/home/jayo/saves/game1");
        g.set_last_sync_mtime("uuid-pc", ts(1_700_000_000));
        g.last_synced_from = Some("uuid-pc".into());
        g.last_sync_time_utc = Some(ts(1_700_000_010));
        g.latest_write_time_utc = Some(ts(1_700_000_005));
        g.storage_bytes = 12345;
        list.upsert_game(g);

        let json = serde_json::to_string(&list).unwrap();
        let parsed: GameListFile = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.games.len(), 1);
        let parsed_g = parsed.get_game("game1").unwrap();
        assert_eq!(parsed_g.name, "Game 1");
        assert_eq!(parsed_g.get_path("uuid-pc"), Some("/home/jayo/saves/game1"));
        assert_eq!(parsed_g.get_last_sync_mtime("uuid-pc"), Some(ts(1_700_000_000)));
        assert_eq!(parsed_g.last_synced_from.as_deref(), Some("uuid-pc"));
        assert_eq!(parsed_g.last_sync_time_utc, Some(ts(1_700_000_010)));
        assert_eq!(parsed_g.storage_bytes, 12345);
        assert_eq!(parsed.get_device_name("uuid-pc"), "Jayo-PC");
    }

    // Backwards-compat: JSON viejo sin device_names ------------------------------

    #[test]
    fn parses_legacy_json_without_device_names_field() {
        let legacy = r#"{
            "games": [
                {
                    "id": "game1",
                    "name": "Game 1",
                    "path_by_device": {},
                    "last_synced_from": null,
                    "last_sync_time_utc": null,
                    "latest_write_time_utc": null,
                    "storage_bytes": 0
                }
            ]
        }"#;
        let parsed: GameListFile = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.games.len(), 1);
        assert!(parsed.device_names.is_empty());
    }

    // Backwards-compat: DevicePathEntry sin last_sync_mtime ----------------------

    #[test]
    fn parses_legacy_device_path_entry_without_last_sync_mtime() {
        let legacy = r#"{
            "id": "game1",
            "name": "Game 1",
            "path_by_device": {
                "uuid-old": { "path": "/saves/game1" }
            },
            "last_synced_from": null,
            "last_sync_time_utc": null,
            "latest_write_time_utc": null,
            "storage_bytes": 0
        }"#;
        let parsed: GameMetaData = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.get_path("uuid-old"), Some("/saves/game1"));
        assert_eq!(parsed.get_last_sync_mtime("uuid-old"), None);
    }

    // GameListFile::get_game_mut -------------------------------------------------

    #[test]
    fn get_game_mut_allows_in_place_modification() {
        let mut list = GameListFile::default();
        list.upsert_game(GameMetaData::new("game1", "Game 1"));
        list.get_game_mut("game1").unwrap().storage_bytes = 42;
        assert_eq!(list.get_game("game1").unwrap().storage_bytes, 42);
    }
}
