use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SaveMode {
    #[default]
    Local,
    Cloud,
    Sync,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GameSyncConfig {
    pub mode: SaveMode,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncGamesConfig {
    pub games: HashMap<String, GameSyncConfig>,
}

impl SyncGamesConfig {
    const FILE_NAME: &'static str = "sync-games.json";

    pub fn path() -> std::path::PathBuf {
        let app_dir = crate::prelude::app_dir();
        let rendered = app_dir.render();
        std::path::PathBuf::from(rendered).join(Self::FILE_NAME)
    }

    pub fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, content);
        }
    }

    pub fn get_mode(&self, game: &str) -> &SaveMode {
        self.games
            .get(game)
            .map(|c| &c.mode)
            .unwrap_or(&SaveMode::Local)
    }

    pub fn set_mode(&mut self, game: &str, mode: SaveMode) {
        self.games
            .entry(game.to_string())
            .or_default()
            .mode = mode;
    }
}
