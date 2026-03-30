use std::collections::HashMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The JSON file stored in the cloud alongside the game zips.
/// Equivalent to GameListFile in EmuSync.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameListFile {
    pub games: Vec<GameMetaData>,
}

/// Metadata for a single game stored in the cloud.
/// Equivalent to GameMetaData in EmuSync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameMetaData {
    /// Unique ID for the game (we use the game name as ID since Ludusavi is name-based)
    pub id: String,

    /// Game name
    pub name: String,

    /// Map of device_id -> local path on that device
    /// Equivalent to SyncSourceIdLocations in EmuSync
    pub path_by_device: HashMap<String, String>,

    /// ID of the device that last synced
    pub last_synced_from: Option<String>,

    /// When the last sync happened
    pub last_sync_time_utc: Option<DateTime<Utc>>,

    /// Latest write time of the save files at the time of last sync
    /// Used to detect if local or cloud is newer
    pub latest_write_time_utc: Option<DateTime<Utc>>,

    /// Size in bytes of the last backup
    pub storage_bytes: u64,
}

impl GameMetaData {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            path_by_device: HashMap::new(),
            last_synced_from: None,
            last_sync_time_utc: None,
            latest_write_time_utc: None,
            storage_bytes: 0,
        }
    }
}

impl GameListFile {
    pub fn get_game(&self, id: &str) -> Option<&GameMetaData> {
        self.games.iter().find(|g| g.id == id)
    }

    pub fn upsert_game(&mut self, game: GameMetaData) {
        if let Some(existing) = self.games.iter_mut().find(|g| g.id == game.id) {
            *existing = game;
        } else {
            self.games.push(game);
        }
    }
}
