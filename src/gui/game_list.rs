use std::collections::HashSet;

use iced::{alignment::Horizontal as HorizontalAlignment, padding, widget::tooltip, Alignment, Length};

use crate::{
    gui::{
        badge::Badge,
        button,
        common::{
            BackupPhase, GameAction, GameSelection, Message, Operation, RestorePhase, UndoSubject,
        },
        file_tree::FileTree,
        icon::Icon,
        search::FilterComponent,
        shortcuts::TextHistories,
        style,
        widget::{
            checkbox, pick_list, text, text_editor, Button, Column, Container, IcedButtonExt, IcedParentExt, Row,
            Tooltip,
        },
    },
    lang::TRANSLATOR,
    resource::{
        cache::Cache,
        config::{Config, Sort},
        manifest::Manifest,
    },
    scan::{
        layout::GameLayout, BackupInfo, DuplicateDetector, ScanInfo, ScanKind,
    },
};

#[derive(Default)]
pub struct GameListEntry {
    pub scan_info: ScanInfo,
    pub backup_info: Option<BackupInfo>,
    pub tree: Option<FileTree>,
    pub game_layout: Option<GameLayout>,
    /// The `scan_info` gets mutated in response to things like toggling saves off,
    /// so we need a persistent flag to say if the game has been scanned yet.
    pub scanned: bool,
}

impl GameListEntry {
    pub fn refresh_tree(&mut self, duplicate_detector: &DuplicateDetector, config: &Config, scan_kind: ScanKind) {
        match self.tree.as_mut() {
            Some(tree) => tree.reset_nodes(
                self.scan_info.clone(),
                self.backup_info.as_ref(),
                duplicate_detector,
                config,
                scan_kind,
            ),
            None => {
                self.tree = Some(FileTree::new(
                    self.scan_info.clone(),
                    self.backup_info.as_ref(),
                    duplicate_detector,
                    config,
                    scan_kind,
                ))
            }
        }
    }

    pub fn clear_tree(&mut self) {
        if let Some(tree) = self.tree.as_mut() {
            tree.clear_nodes();
        }
    }
}

#[derive(Default)]
pub struct GameList {
    pub entries: Vec<GameListEntry>,
    pub search: FilterComponent,
    expanded_games: HashSet<String>,
    pub filter_duplicates_of: Option<String>,
}

impl GameList {
    fn filter_game(
        &self,
        entry: &GameListEntry,
        scan_kind: ScanKind,
        config: &Config,
        manifest: &Manifest,
        duplicate_detector: &DuplicateDetector,
        duplicatees: Option<&HashSet<String>>,
    ) -> bool {
        let show = config.should_show_game(
            &entry.scan_info.game_name,
            scan_kind,
            entry.scan_info.overall_change().is_changed(),
            entry.scanned,
        );

        let qualifies = self.search.qualifies(
            &entry.scan_info,
            manifest,
            config.is_game_enabled_for_operation(&entry.scan_info.game_name, scan_kind),
            config.is_game_customized(&entry.scan_info.game_name),
            duplicate_detector.is_game_duplicated(&entry.scan_info.game_name),
            config.scan.show_deselected_games,
        );

        let duplicate = duplicatees
            .as_ref()
            .map(|xs| xs.contains(&entry.scan_info.game_name))
            .unwrap_or(true);

        show && qualifies && duplicate
    }

    pub fn visible_games(
        &self,
        scan_kind: ScanKind,
        config: &Config,
        manifest: &Manifest,
        duplicate_detector: &DuplicateDetector,
    ) -> HashSet<String> {
        let duplicatees = self.filter_duplicates_of.as_ref().and_then(|game| {
            let mut duplicatees = duplicate_detector.duplicate_games(game);
            if duplicatees.is_empty() {
                None
            } else {
                duplicatees.insert(game.clone());
                Some(duplicatees)
            }
        });

        self.entries
            .iter()
            .filter(|entry| {
                self.filter_game(
                    entry,
                    scan_kind,
                    config,
                    manifest,
                    duplicate_detector,
                    duplicatees.as_ref(),
                )
            })
            .map(|x| x.scan_info.game_name.clone())
            .collect()
    }

    pub fn is_filtered(&self) -> bool {
        self.search.show || self.filter_duplicates_of.is_some()
    }

    pub fn sort(&mut self, sort: &Sort, config: &Config) {
        self.entries.sort_by(|x, y| {
            crate::scan::compare_games(
                sort.key,
                config,
                config.display_name(&x.scan_info.game_name),
                &x.scan_info,
                x.backup_info.as_ref(),
                config.display_name(&y.scan_info.game_name),
                &y.scan_info,
                y.backup_info.as_ref(),
            )
        });
        if sort.reversed {
            self.entries.reverse();
        }
    }

    pub fn expand_game(
        &mut self,
        game: &str,
        duplicate_detector: &DuplicateDetector,
        config: &Config,
        scan_kind: ScanKind,
    ) {
        if self.expanded_games.contains(game) {
            return;
        }

        self.expanded_games.insert(game.to_string());
        for entry in self.entries.iter_mut() {
            if entry.scan_info.game_name == game {
                entry.refresh_tree(duplicate_detector, config, scan_kind);
                break;
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.expanded_games.clear();
    }

    pub fn with_recent_games(scan_kind: ScanKind, config: &Config, cache: &Cache) -> Self {
        let games = match scan_kind {
            ScanKind::Backup => &cache.backup.recent_games,
            ScanKind::Restore => &cache.restore.recent_games,
        };
        let sort = match scan_kind {
            ScanKind::Backup => &config.backup.sort,
            ScanKind::Restore => &config.restore.sort,
        };

        let mut log = Self::default();
        for game in games {
            log.update_game(
                ScanInfo {
                    game_name: game.clone(),
                    ..Default::default()
                },
                Default::default(),
                sort,
                &DuplicateDetector::default(),
                &Default::default(),
                None,
                config,
                scan_kind,
            );
        }
        log
    }

    pub fn find_game(&self, game: &str) -> Option<usize> {
        let mut index = None;

        for (i, entry) in self.entries.iter().enumerate() {
            if entry.scan_info.game_name == game {
                index = Some(i);
                break;
            }
        }

        index
    }

    pub fn update_game(
        &mut self,
        scan_info: ScanInfo,
        backup_info: Option<BackupInfo>,
        sort: &Sort,
        duplicate_detector: &DuplicateDetector,
        duplicates: &HashSet<String>,
        game_layout: Option<GameLayout>,
        config: &Config,
        scan_kind: ScanKind,
    ) {
        let game_name = scan_info.game_name.clone();
        let index = self.find_game(&game_name);
        let scanned = scan_info.found_anything();

        match index {
            Some(i) => {
                if scan_info.can_report_game() {
                    self.entries[i].scan_info = scan_info;
                    self.entries[i].backup_info = backup_info;
                    self.entries[i].game_layout = game_layout;
                    self.entries[i].scanned = scanned || self.entries[i].scanned;
                    if self.expanded_games.contains(&game_name) {
                        self.entries[i].refresh_tree(duplicate_detector, config, scan_kind);
                    }
                } else {
                    self.entries.remove(i);
                }
            }
            None => {
                let mut entry = GameListEntry {
                    scan_info,
                    backup_info,
                    game_layout,
                    scanned,
                    ..Default::default()
                };
                if self.expanded_games.contains(&game_name) {
                    entry.refresh_tree(duplicate_detector, config, scan_kind);
                }
                self.entries.push(entry);
                self.sort(sort, config);
            }
        }

        if !duplicates.is_empty() {
            for entry in self.entries.iter_mut() {
                if duplicates.contains(&entry.scan_info.game_name)
                    && self.expanded_games.contains(&entry.scan_info.game_name)
                {
                    entry.refresh_tree(duplicate_detector, config, scan_kind);
                }
            }
        }
    }

    pub fn refresh_game_tree(
        &mut self,
        game: &str,
        config: &Config,
        duplicate_detector: &mut DuplicateDetector,
        scan_kind: ScanKind,
    ) {
        if let Some(index) = self.find_game(game) {
            match scan_kind {
                ScanKind::Backup => {
                    self.entries[index]
                        .scan_info
                        .update_ignored(&config.backup.toggled_paths, &config.backup.toggled_registry);
                }
                ScanKind::Restore => {
                    self.entries[index]
                        .scan_info
                        .update_ignored(&config.restore.toggled_paths, &config.restore.toggled_registry);
                }
            }

            let stale = duplicate_detector.add_game(
                &self.entries[index].scan_info,
                config.is_game_enabled_for_operation(game, scan_kind),
            );

            self.entries[index].refresh_tree(duplicate_detector, config, scan_kind);

            for entry in &mut self.entries {
                if stale.contains(&entry.scan_info.game_name) {
                    entry.refresh_tree(duplicate_detector, config, scan_kind);
                }
            }
        }
    }

    pub fn remove_game(
        &mut self,
        game: &str,
        duplicate_detector: &DuplicateDetector,
        duplicates: &HashSet<String>,
        config: &Config,
        scan_kind: ScanKind,
    ) {
        self.entries.retain(|entry| entry.scan_info.game_name != game);
        for entry in self.entries.iter_mut() {
            if duplicates.contains(&entry.scan_info.game_name) {
                entry.refresh_tree(duplicate_detector, config, scan_kind);
            }
        }
    }

    pub fn unscan_games(&mut self, games: &GameSelection) {
        for entry in self.entries.iter_mut() {
            if games.contains(&entry.scan_info.game_name) {
                entry.scanned = false;
                entry.scan_info.found_files.clear();
                entry.scan_info.found_registry_keys.clear();
                if !games.is_single() {
                    entry.clear_tree();
                    self.expanded_games.remove(&entry.scan_info.game_name);
                }
            }
        }
    }

    pub fn contains_unscanned_games(&self) -> bool {
        self.entries.iter().any(|x| !x.scanned)
    }

    pub fn save_layout(&mut self, game: &str) {
        let Some(index) = self.find_game(game) else { return };
        let entry = &mut self.entries[index];
        let Some(layout) = &mut entry.game_layout else { return };

        layout.save();
    }
}
