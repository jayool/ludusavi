use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SaveMode {
    #[default]
    None,
    Local,
    Cloud,
    Sync,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameSyncConfig {
    pub mode: SaveMode,
    #[serde(default = "default_true")]
    pub auto_sync: bool,
}

impl Default for GameSyncConfig {
    fn default() -> Self {
        Self {
            mode: SaveMode::None,
            auto_sync: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncGamesConfig {
    pub games: HashMap<String, GameSyncConfig>,
    #[serde(default = "default_true")]
    pub safety_backups_enabled: bool,
    #[serde(default = "default_true")]
    pub system_notifications_enabled: bool,
}

impl Default for SyncGamesConfig {
    fn default() -> Self {
        Self {
            games: HashMap::new(),
            safety_backups_enabled: true,
            system_notifications_enabled: true,
        }
    }
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
            .unwrap_or(&SaveMode::None)
    }

    pub fn set_mode(&mut self, game: &str, mode: SaveMode) {
        self.games
            .entry(game.to_string())
            .or_default()
            .mode = mode;
    }
    
    pub fn get_auto_sync(&self, game: &str) -> bool {
        self.games.get(game).map(|c| c.auto_sync).unwrap_or(true)
    }

    pub fn set_auto_sync(&mut self, game: &str, auto_sync: bool) {
        self.games.entry(game.to_string()).or_default().auto_sync = auto_sync;
    }
        pub fn safety_backups_enabled(&self) -> bool {
        self.safety_backups_enabled
    }

    pub fn system_notifications_enabled(&self) -> bool {
        self.system_notifications_enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Defaults --------------------------------------------------------------------

    #[test]
    fn default_save_mode_is_none() {
        assert_eq!(SaveMode::default(), SaveMode::None);
    }

    /// Tras el fix de impl Default manual, default programático y serde-default
    /// coinciden: ambos dan auto_sync=true. Esto evita que `set_mode("nuevo", X)`
    /// cree entradas con auto_sync=false cuando el usuario solo cambia el modo.
    #[test]
    fn default_auto_sync_is_true() {
        let cfg = GameSyncConfig::default();
        assert!(cfg.auto_sync, "Default::default() must agree with serde default");
    }

    /// Regresión del bug: cuando un usuario cambia el modo de un juego que no
    /// existía antes en sync-games.json, el toggle "Auto Sync" debe quedar ON
    /// (true), no OFF — antes se rompía aquí porque or_default() daba false.
    #[test]
    fn set_mode_for_new_game_leaves_auto_sync_on() {
        let mut cfg = SyncGamesConfig::default();
        // Ningún juego configurado todavía.
        assert!(cfg.get_auto_sync("FreshGame"), "default should be true");
        cfg.set_mode("FreshGame", SaveMode::Cloud);
        assert!(
            cfg.get_auto_sync("FreshGame"),
            "Auto Sync should remain ON after setting mode for the first time"
        );
    }

    #[test]
    fn default_safety_backups_and_notifications_are_enabled() {
        let cfg = SyncGamesConfig::default();
        assert!(cfg.safety_backups_enabled);
        assert!(cfg.system_notifications_enabled);
    }

    // get_mode / set_mode --------------------------------------------------------

    #[test]
    fn get_mode_returns_none_for_unknown_game() {
        let cfg = SyncGamesConfig::default();
        assert_eq!(cfg.get_mode("nonexistent"), &SaveMode::None);
    }

    #[test]
    fn set_mode_creates_entry_if_missing() {
        let mut cfg = SyncGamesConfig::default();
        cfg.set_mode("game1", SaveMode::Sync);
        assert_eq!(cfg.get_mode("game1"), &SaveMode::Sync);
    }

    #[test]
    fn set_mode_preserves_auto_sync_when_updating() {
        let mut cfg = SyncGamesConfig::default();
        cfg.set_auto_sync("game1", false);
        cfg.set_mode("game1", SaveMode::Cloud);
        assert_eq!(cfg.get_mode("game1"), &SaveMode::Cloud);
        assert!(!cfg.get_auto_sync("game1"));
    }

    // get_auto_sync / set_auto_sync ----------------------------------------------

    #[test]
    fn get_auto_sync_returns_true_for_unknown_game() {
        let cfg = SyncGamesConfig::default();
        assert!(cfg.get_auto_sync("nonexistent"));
    }

    #[test]
    fn set_auto_sync_creates_entry_with_default_mode() {
        let mut cfg = SyncGamesConfig::default();
        // Set explícito a false: pisa el default (true).
        cfg.set_auto_sync("game1", false);
        assert!(!cfg.get_auto_sync("game1"));
        // El modo recién creado por set_auto_sync debe ser el default (None).
        assert_eq!(cfg.get_mode("game1"), &SaveMode::None);
    }

    #[test]
    fn set_auto_sync_preserves_mode_when_updating() {
        let mut cfg = SyncGamesConfig::default();
        cfg.set_mode("game1", SaveMode::Sync);
        cfg.set_auto_sync("game1", false);
        assert_eq!(cfg.get_mode("game1"), &SaveMode::Sync);
        assert!(!cfg.get_auto_sync("game1"));
    }

    // Round-trip JSON ------------------------------------------------------------

    #[test]
    fn round_trip_json_preserves_all_fields() {
        let mut cfg = SyncGamesConfig {
            safety_backups_enabled: false,
            system_notifications_enabled: false,
            ..Default::default()
        };
        cfg.set_mode("Stardew Valley", SaveMode::Sync);
        cfg.set_auto_sync("Stardew Valley", true);
        cfg.set_mode("Hades", SaveMode::Local);
        cfg.set_auto_sync("Hades", false);

        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: SyncGamesConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.get_mode("Stardew Valley"), &SaveMode::Sync);
        assert!(parsed.get_auto_sync("Stardew Valley"));
        assert_eq!(parsed.get_mode("Hades"), &SaveMode::Local);
        assert!(!parsed.get_auto_sync("Hades"));
        assert!(!parsed.safety_backups_enabled);
        assert!(!parsed.system_notifications_enabled);
    }

    #[test]
    fn save_mode_serializes_camel_case() {
        // El usuario o el daemon pueden inspeccionar el JSON manualmente; mantener
        // camelCase es contrato persistido.
        let mut cfg = SyncGamesConfig::default();
        cfg.set_mode("game1", SaveMode::None);
        cfg.set_mode("game2", SaveMode::Local);
        cfg.set_mode("game3", SaveMode::Cloud);
        cfg.set_mode("game4", SaveMode::Sync);
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"none\""), "expected camelCase 'none' in {json}");
        assert!(json.contains("\"local\""), "expected camelCase 'local' in {json}");
        assert!(json.contains("\"cloud\""), "expected camelCase 'cloud' in {json}");
        assert!(json.contains("\"sync\""), "expected camelCase 'sync' in {json}");
    }

    // Backwards-compat -----------------------------------------------------------

    #[test]
    fn parses_legacy_json_without_safety_or_notification_flags() {
        // JSON antiguo (antes de añadir safety_backups_enabled / system_notifications_enabled)
        // debe parsear y aplicar los defaults (true / true).
        let legacy = r#"{
            "games": {
                "game1": { "mode": "sync", "auto_sync": true }
            }
        }"#;
        let parsed: SyncGamesConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.get_mode("game1"), &SaveMode::Sync);
        assert!(parsed.safety_backups_enabled);
        assert!(parsed.system_notifications_enabled);
    }

    #[test]
    fn parses_legacy_game_config_without_auto_sync() {
        // GameSyncConfig viejo sin auto_sync debe asumir el default (true).
        let legacy = r#"{
            "games": {
                "game1": { "mode": "local" }
            },
            "safety_backups_enabled": true,
            "system_notifications_enabled": true
        }"#;
        let parsed: SyncGamesConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.get_mode("game1"), &SaveMode::Local);
        assert!(parsed.get_auto_sync("game1"));
    }
}
