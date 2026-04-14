use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use iced::{keyboard, widget::scrollable, Alignment, Length, Subscription, Task};

use ludusavi::sync::bridge::register_game_after_backup;

use crate::{
    cloud::{rclone_monitor, Rclone, Remote},
    gui::{
        common::{
            BackupPhase, BrowseFileSubject, BrowseSubject, Flags, GameAction, GameSelection, Message, Operation,
            RestorePhase, Screen, ScrollSubject, UndoSubject, ValidatePhase,
        },
        modal::{self, CloudModalState, Modal, ModalField, ModalInputKind},
        notification::Notification,
        screen,
        shortcuts::{RootHistory, Shortcut, TextHistories, TextHistory},
        style,
        widget::{
            id, operation::container_scroll_offset, Column, Container, Element, IcedParentExt, Progress, Row, Stack,
        },
    },
    lang::TRANSLATOR,
    prelude::{
        app_dir, get_threads_from_env, initialize_rayon, EditAction, Error, Finality, RedirectEditActionField,
        Security, StrictPath, SyncDirection,
    },
    resource::{
        cache::{self, Cache},
        config::{self, Config, CustomGame, CustomGameKind, Root},
        manifest::Manifest,
        ResourceFile, SaveableResourceFile,
    },
    scan::{
        game_filter, layout::BackupLayout, prepare_backup_target, registry::RegistryItem, scan_game_for_backup,
        BackupId, Launchers, ScanKind, SteamShortcuts, TitleFinder,
    },
};

pub struct Executor(tokio::runtime::Runtime);

impl iced::Executor for Executor {
    fn new() -> Result<Self, iced::futures::io::Error> {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.enable_all();

        if let Some(threads) = get_threads_from_env().or_else(|| Config::load().ok().and_then(|x| x.runtime.threads)) {
            initialize_rayon(threads);
            builder.worker_threads(threads.get());
        }

        builder.build().map(Self)
    }

    #[allow(clippy::let_underscore_future)]
    fn spawn(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        let _ = tokio::runtime::Runtime::spawn(&self.0, future);
    }

    fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = tokio::runtime::Runtime::enter(&self.0);
        f()
    }

    fn block_on<T>(&self, future: impl std::prelude::rust_2024::Future<Output = T>) -> T {
        self.0.block_on(future)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SaveKind {
    Config,
    Cache,
    Backup(String),
}

#[derive(Clone, Debug, Default)]
pub struct LoadedManifest {
    pub primary: Manifest,
    pub extended: Manifest,
}

pub struct OperationStep {
    title: String,
    task: Task<Message>,
}

#[derive(Default)]
pub struct App {
    flags: Flags,
    config: Config,
    manifest: LoadedManifest,
    cache: Cache,
    operation: Operation,
    screen: Screen,
    modals: Vec<Modal>,
    backup_screen: screen::Backup,
    restore_screen: screen::Restore,
    custom_games_screen: screen::CustomGames,
    operation_should_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    operation_steps: Vec<OperationStep>,
    operation_steps_active: usize,
    progress: Progress,
    backups_to_restore: HashMap<String, BackupId>,
    updating_manifest: bool,
    notify_on_single_game_scanned: Option<(String, Screen)>,
    manifest_notification: Option<Notification>,
    timed_notification: Option<Notification>,
    scroll_offsets: HashMap<ScrollSubject, scrollable::AbsoluteOffset>,
    text_histories: TextHistories,
    rclone_monitor_sender: Option<iced::futures::channel::mpsc::Sender<rclone_monitor::Input>>,
    exiting: bool,
    pending_save: HashMap<SaveKind, Instant>,
    modifiers: keyboard::Modifiers,
    jump_to_game_after_scan: Option<String>,
    daemon_running: bool,
    sync_status: std::collections::HashMap<String, String>,
    games_search: String,
    pending_game_detail: Option<ludusavi::sync::sync_config::GameSyncConfig>,
    pending_game_detail_name: Option<String>,
    game_list: ludusavi::sync::game_list::GameListFile,
    sync_games_config: ludusavi::sync::sync_config::SyncGamesConfig,
    sync_in_progress: Option<String>,
    game_detail_files_expanded: bool,
}

impl App {
    fn go_idle(&mut self) {
        if self.exiting {
            self.save();
            std::process::exit(0);
        }

        self.operation = Operation::Idle;
        self.operation_steps.clear();
        self.operation_steps_active = 0;
        self.close_specific_modal_alt(modal::Kind::ActiveScanGames);
        self.progress.reset();
        self.operation_should_cancel
            .swap(false, std::sync::atomic::Ordering::Relaxed);
        self.notify_on_single_game_scanned = None;
    }

    fn show_modal(&mut self, modal: Modal) -> Task<Message> {
        let replace = self
            .modals
            .last()
            .map(|last| last.kind() == modal.kind() && !modal.stackable())
            .unwrap_or(false);

        if replace {
            self.modals.pop();
        }

        self.modals.push(modal);
        self.reset_scroll_position(ScrollSubject::Modal);
        self.refresh_scroll_position()
    }

    fn close_modal(&mut self) -> Task<Message> {
        if let Some(modal) = self.modals.pop() {
            self.reset_scroll_position(ScrollSubject::Modal);
            let need_cancel_cloud = modal.is_cloud_active();
            Task::batch([
                self.refresh_scroll_position(),
                if need_cancel_cloud {
                    self.cancel_operation()
                } else {
                    Task::none()
                },
            ])
        } else {
            Task::none()
        }
    }

    fn close_specific_modal(&mut self, kind: modal::Kind) -> Task<Message> {
        self.modals.retain(|modal| modal.kind() != kind);
        self.refresh_scroll_position()
    }

    fn close_specific_modal_alt(&mut self, kind: modal::Kind) {
        self.modals.retain(|modal| modal.kind() != kind);
    }

    fn show_error(&mut self, error: Error) -> Task<Message> {
        self.show_modal(Modal::Error { variant: error })
    }

    fn save(&mut self) {
        let threshold = Duration::from_secs(1);
        let now = Instant::now();

        self.pending_save.retain(|item, then| {
            if (now - *then) < threshold {
                return true;
            }

            match item {
                SaveKind::Config => self.config.save(),
                SaveKind::Cache => self.cache.save(),
                SaveKind::Backup(game) => self.restore_screen.log.save_layout(game),
            }

            false
        });
    }

    fn save_config(&mut self) {
        self.pending_save.insert(SaveKind::Config, Instant::now());
    }

    fn save_cache(&mut self) {
        self.pending_save.insert(SaveKind::Cache, Instant::now());
    }

    fn save_backup(&mut self, game: &str) {
        self.pending_save
            .insert(SaveKind::Backup(game.to_string()), Instant::now());
    }

    fn invalidate_path_caches(&self) {
        for x in &self.config.roots {
            x.path().invalidate_cache();
        }
        for x in &self.config.redirects {
            x.source.invalidate_cache();
            x.target.invalidate_cache();
        }
        self.config.backup.path.invalidate_cache();
        self.config.restore.path.invalidate_cache();
        self.config.backup.toggled_paths.invalidate_path_caches();
    }

    fn register_notify_on_single_game_scanned(&mut self) {
        if let Some(GameSelection::Single { game }) = self.operation.games() {
            self.notify_on_single_game_scanned = Some((game.clone(), self.screen.clone()));
        }
    }

    fn handle_notify_on_single_game_scanned(&mut self) -> bool {
        if let Some((name, screen)) = self.notify_on_single_game_scanned.as_ref() {
            let log = match self.operation {
                Operation::Backup { .. } => &self.backup_screen.log,
                Operation::Restore { .. } => &self.restore_screen.log,
                _ => return false,
            };
            let found = log.entries.iter().any(|x| &x.scan_info.game_name == name);

            if *screen != Screen::CustomGames && found {
                return found;
            }

            let msg = TRANSLATOR.notify_single_game_status(found);
            self.timed_notification = Some(Notification::new(msg).expires(3));
            return found;
        }

        false
    }

    fn start_sync_cloud(
        &mut self,
        local: &StrictPath,
        direction: SyncDirection,
        finality: Finality,
        games: Option<GameSelection>,
        standalone: bool,
    ) -> Result<(), Error> {
        let remote = crate::cloud::validate_cloud_config(&self.config, &self.config.cloud.path)?;

        let games = match games {
            Some(games) => {
                let layout = BackupLayout::new(local.clone());
                let games: Vec<_> = games.iter().filter_map(|x| layout.game_folder(x).leaf()).collect();
                games
            }
            None => vec![],
        };

        let rclone = Rclone::new(self.config.apps.rclone.clone(), remote);
        match rclone.sync(local, &self.config.cloud.path, direction, finality, &games) {
            Ok(process) => {
                if let Some(sender) = self.rclone_monitor_sender.as_mut() {
                    if standalone {
                        self.operation = Operation::new_cloud(direction, finality);
                    } else {
                        self.operation.update_integrated_cloud(finality);
                    }
                    self.progress.start();
                    let _ = sender.try_send(rclone_monitor::Input::Process(process));
                }
            }
            Err(e) => {
                return Err(Error::UnableToSynchronizeCloud(e));
            }
        }

        Ok(())
    }

    fn handle_backup(&mut self, phase: BackupPhase) -> Task<Message> {
        const SCAN_KIND: ScanKind = ScanKind::Backup;

        match phase {
            BackupPhase::Confirm { games } => self.show_modal(Modal::ConfirmBackup { games }),
            BackupPhase::Start {
                preview,
                repair,
                jump,
                mut games,
            } => {
                if !self.operation.idle() {
                    return Task::none();
                }

                let mut cleared_log = false;
                if games.is_none() {
                    if self.backup_screen.log.is_filtered() {
                        games = Some(GameSelection::group(self.backup_screen.log.visible_games(
                            SCAN_KIND,
                            &self.config,
                            &self.manifest.extended,
                            &self.backup_screen.duplicate_detector,
                        )));
                    } else {
                        self.backup_screen.log.clear();
                        self.backup_screen.duplicate_detector.clear();
                        self.reset_scroll_position(ScrollSubject::Backup);
                        cleared_log = true;
                    }
                }

                if jump {
                    if let Some(GameSelection::Single { game }) = &games {
                        self.jump_to_game_after_scan = Some(game.clone());
                    }
                }

                self.operation =
                    Operation::new_backup(if preview { Finality::Preview } else { Finality::Final }, games);
                self.operation.set_force_new_full_backups(repair);

                if !preview {
                    if let Err(e) = prepare_backup_target(&self.config.backup.path) {
                        self.go_idle();
                        return self.show_error(e);
                    }
                }

                Task::batch([
                    self.close_modal(),
                    if repair {
                        self.switch_screen(Screen::Backup)
                    } else {
                        Task::none()
                    },
                    self.refresh_scroll_position_on_log(cleared_log),
                    self.handle_backup(BackupPhase::CloudCheck),
                ])
            }
            BackupPhase::CloudCheck => {
                // Cloud upload deshabilitado: lo gestiona ludusavi-daemon via ZIP
                self.handle_backup(BackupPhase::Load)
            }
            BackupPhase::Load => {
                self.invalidate_path_caches();
                self.timed_notification = None;

                let preview = self.operation.preview();
                let full = self.operation.full();
                let games = self.operation.games().cloned();

                if preview && full {
                    self.backup_screen.previewed_games.clear();
                }

                let all_scanned = !self.backup_screen.log.contains_unscanned_games();
                if let Some(games) = &games {
                    self.backup_screen.log.unscan_games(games);
                }
                self.progress.start();

                let mut manifest = self.manifest.primary.clone();
                let config = self.config.clone();
                let previewed_games = self.backup_screen.previewed_games.clone();

                Task::perform(
                    async move {
                        manifest.incorporate_extensions(&config);
                        let subjects: HashSet<_> = if let Some(games) = &games {
                            manifest.0.keys().filter(|k| games.contains(k)).cloned().collect()
                        } else if !previewed_games.is_empty() && all_scanned {
                            manifest
                                .0
                                .keys()
                                .filter(|k| previewed_games.contains(*k))
                                .cloned()
                                .collect()
                        } else {
                            manifest.processable_titles().cloned().collect()
                        };

                        // HashSet -> Vec because randomized order looks nicer in the GUI.
                        let subjects: Vec<_> = subjects.into_iter().collect();

                        let roots = config.expanded_roots();
                        let layout = BackupLayout::new(config.backup.path.clone());
                        let title_finder = TitleFinder::new(&config, &manifest, layout.restorable_game_set());
                        let steam = SteamShortcuts::scan(&title_finder);
                        let launchers = Launchers::scan(&roots, &manifest, &subjects, &title_finder, None);

                        (subjects, manifest, layout, steam, launchers)
                    },
                    move |(subjects, manifest, layout, steam, heroic)| {
                        Message::Backup(BackupPhase::RegisterCommands {
                            subjects,
                            manifest,
                            layout: Box::new(layout),
                            steam,
                            launchers: heroic,
                        })
                    },
                )
            }
            BackupPhase::RegisterCommands {
                subjects,
                manifest,
                layout,
                steam,
                launchers,
            } => {
                log::info!("beginning backup with {} steps", subjects.len());
                let preview = self.operation.preview();
                let single = self.operation.games().is_some_and(|x| x.is_single());

                if self.operation_should_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    self.go_idle();
                    return Task::none();
                }

                if subjects.is_empty() {
                    if let Some(games) = self.operation.games() {
                        for game in games.iter() {
                            let duplicates = self.backup_screen.duplicate_detector.remove_game(game);
                            self.backup_screen.log.remove_game(
                                game,
                                &self.backup_screen.duplicate_detector,
                                &duplicates,
                                &self.config,
                                SCAN_KIND,
                            );
                        }
                        self.cache.backup.recent_games.retain(|x| !games.contains(x));
                        self.save_cache();
                    }
                    self.go_idle();
                    return Task::none();
                }

                self.progress.set_max(subjects.len() as f32);
                self.register_notify_on_single_game_scanned();

                let config = std::sync::Arc::new(self.config.clone());
                let roots = std::sync::Arc::new(config.expanded_roots());
                let layout = std::sync::Arc::new(*layout);
                let launchers = std::sync::Arc::new(launchers);
                let filter = std::sync::Arc::new(self.config.backup.filter.clone());
                let steam_shortcuts = std::sync::Arc::new(steam);
                let games_specified = self.operation.games_specified();
                let retention = config
                    .backup
                    .retention
                    .with_force_new_full(self.operation.should_force_new_full_backups());

                for key in subjects {
                    let game = manifest.0[&key].clone();
                    let config = config.clone();
                    let roots = roots.clone();
                    let launchers = launchers.clone();
                    let layout = layout.clone();
                    let filter = filter.clone();
                    let steam_shortcuts = steam_shortcuts.clone();
                    let cancel_flag = self.operation_should_cancel.clone();
                    self.operation_steps.push(OperationStep {
                        title: key.clone(),
                        task: Task::perform(
                            async move {
                                if key.trim().is_empty() {
                                    return (None, None);
                                }
                                if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                                    return (None, None);
                                }

                                let previous = layout.latest_backup(
                                    &key,
                                    SCAN_KIND,
                                    &config.redirects,
                                    config.restore.reverse_redirects,
                                    &config.restore.toggled_paths,
                                    config.backup.only_constructive,
                                );

                                if filter.excludes(games_specified, previous.is_some(), &game.cloud) {
                                    log::trace!("[{key}] excluded by backup filter");
                                    return (None, None);
                                }

                                let scan_info = scan_game_for_backup(
                                    &game,
                                    &key,
                                    &roots,
                                    &app_dir(),
                                    &launchers,
                                    &filter,
                                    None,
                                    &config.backup.toggled_paths,
                                    &config.backup.toggled_registry,
                                    previous.as_ref(),
                                    &config.redirects,
                                    config.restore.reverse_redirects,
                                    &steam_shortcuts,
                                    config.backup.only_constructive,
                                );
                                if !config.is_game_enabled_for_backup(&key) && !single {
                                    return (Some(scan_info), None);
                                }

                                let backup_info = if !preview {
                                    layout.game_layout(&key).back_up(
                                        &scan_info,
                                        &chrono::Utc::now(),
                                        &config.backup.format,
                                        retention,
                                        config.backup.only_constructive,
                                    )
                                } else {
                                    None
                                };
                                (Some(scan_info), backup_info)
                            },
                            move |(scan_info, backup_info)| {
                                Message::Backup(BackupPhase::GameScanned { scan_info, backup_info })
                            },
                        ),
                    });
                }

                self.operation_steps_active = 100.min(self.operation_steps.len());

                let mut tasks = vec![];
                for step in self.operation_steps.drain(..self.operation_steps_active) {
                    self.operation.add_active_game(step.title);
                    tasks.push(step.task);
                }
                Task::batch(tasks)
            }
            BackupPhase::GameScanned { scan_info, backup_info } => {
                self.progress.step();
                let full = self.operation.full();

                if let Some(mut scan_info) = scan_info {
                    log::trace!(
                        "step {} / {}: {}",
                        self.progress.current,
                        self.progress.max,
                        scan_info.game_name
                    );
                    self.operation.remove_active_game(&scan_info.game_name);
                    if scan_info.can_report_game() {
                        if let Some(backup_info) = backup_info.as_ref() {
                            if scan_info.needs_cloud_sync() {
                                self.operation.add_syncable_game(scan_info.game_name.clone());
                            }
                            // Puente EmuSync: registra el juego en el game-list.json del cloud
                            register_game_after_backup(&self.config, &scan_info);
                            scan_info.clear_processed_changes(backup_info, SCAN_KIND);
                        }

                        let duplicates = self.backup_screen.duplicate_detector.add_game(
                            &scan_info,
                            self.config
                                .is_game_enabled_for_operation(&scan_info.game_name, SCAN_KIND),
                        );
                        self.backup_screen.previewed_games.insert(scan_info.game_name.clone());
                        self.backup_screen.log.update_game(
                            scan_info,
                            backup_info,
                            &self.config.backup.sort,
                            &self.backup_screen.duplicate_detector,
                            &duplicates,
                            None,
                            &self.config,
                            SCAN_KIND,
                        );
                    } else if !full {
                        let duplicates = self.backup_screen.duplicate_detector.remove_game(&scan_info.game_name);
                        self.backup_screen.log.remove_game(
                            &scan_info.game_name,
                            &self.backup_screen.duplicate_detector,
                            &duplicates,
                            &self.config,
                            SCAN_KIND,
                        );
                        self.cache.backup.recent_games.remove(&scan_info.game_name);
                    }
                } else {
                    log::trace!(
                        "step {} / {}, awaiting {}",
                        self.progress.current,
                        self.progress.max,
                        self.operation_steps_active
                    );
                }

                match self.operation_steps.pop() {
                    Some(step) => {
                        self.operation.add_active_game(step.title);
                        step.task
                    }
                    None => {
                        self.operation_steps_active -= 1;
                        if self.operation_steps_active == 0 {
                            self.handle_backup(BackupPhase::CloudSync)
                        } else {
                            Task::none()
                        }
                    }
                }
            }
            BackupPhase::CloudSync => {
                // Cloud upload deshabilitado: lo gestiona ludusavi-daemon via ZIP
                self.handle_backup(BackupPhase::Done)
            }
            BackupPhase::Done => {
                log::info!("completed backup");
                let mut failed = false;
                let preview = self.operation.preview();
                let full = self.operation.full();

                let found_single = self.handle_notify_on_single_game_scanned();

                if full {
                    self.cache.backup.recent_games.clear();
                }

                for entry in &self.backup_screen.log.entries {
                    self.cache.backup.recent_games.insert(entry.scan_info.game_name.clone());
                    if let Some(backup_info) = &entry.backup_info {
                        if !backup_info.successful() {
                            failed = true;
                        }
                    }
                }

                if !preview && full {
                    self.backup_screen.previewed_games.clear();
                }

                self.save_cache();

                if failed {
                    self.operation.push_error(Error::SomeEntriesFailed);
                }

                let errors = self.operation.errors().cloned();
                self.go_idle();

                if let Some(errors) = errors {
                    if !errors.is_empty() {
                        return self.show_modal(Modal::Errors { errors });
                    }
                }

                if let Some(jump) = self.jump_to_game_after_scan.take() {
                    if found_single {
                        use crate::gui::widget::operation::container_scroll_offset;

                        self.backup_screen.log.expand_game(
                            &jump,
                            &self.backup_screen.duplicate_detector,
                            &self.config,
                            ScanKind::Backup,
                        );

                        return self
                            .switch_screen(Screen::Backup)
                            .chain(container_scroll_offset(jump.into()).map(move |offset| match offset {
                                Some(position) => Message::Scroll {
                                    subject: ScrollSubject::Backup,
                                    position,
                                },
                                None => Message::Ignore,
                            }));
                    }
                }

                Task::none()
            }
        }
    }

    fn handle_restore(&mut self, phase: RestorePhase) -> Task<Message> {
        const SCAN_KIND: ScanKind = ScanKind::Restore;

        match phase {
            RestorePhase::Confirm { games } => self.show_modal(Modal::ConfirmRestore { games }),
            RestorePhase::Start { preview, mut games } => {
                if !self.operation.idle() {
                    return Task::none();
                }

                let path = self.config.restore.path.clone();
                if !path.is_dir() {
                    return self.show_modal(Modal::Error {
                        variant: Error::RestorationSourceInvalid { path },
                    });
                }

                let mut cleared_log = false;
                if games.is_none() {
                    if self.restore_screen.log.is_filtered() {
                        games = Some(GameSelection::group(self.restore_screen.log.visible_games(
                            SCAN_KIND,
                            &self.config,
                            &self.manifest.extended,
                            &self.restore_screen.duplicate_detector,
                        )));
                    } else {
                        self.restore_screen.log.clear();
                        self.restore_screen.duplicate_detector.clear();
                        self.reset_scroll_position(ScrollSubject::Restore);
                        cleared_log = true;
                    }
                }

                self.operation =
                    Operation::new_restore(if preview { Finality::Preview } else { Finality::Final }, games);

                self.invalidate_path_caches();
                self.timed_notification = None;

                Task::batch([
                    self.close_modal(),
                    self.refresh_scroll_position_on_log(cleared_log),
                    self.handle_restore(RestorePhase::CloudCheck),
                ])
            }
            RestorePhase::CloudCheck => {
                if self.operation.preview()
                    || !self.config.cloud.synchronize
                    || crate::cloud::validate_cloud_config(&self.config, &self.config.cloud.path).is_err()
                {
                    return self.handle_restore(RestorePhase::Load);
                }

                let local = self.config.restore.path.clone();
                let games = self.operation.games();

                match self.start_sync_cloud(&local, SyncDirection::Upload, Finality::Preview, games.cloned(), false) {
                    Ok(_) => {
                        // waiting for background thread
                        Task::none()
                    }
                    Err(e) => {
                        self.operation.push_error(e);
                        self.handle_restore(RestorePhase::Load)
                    }
                }
            }
            RestorePhase::Load => {
                let restore_path = self.config.restore.path.clone();

                self.progress.start();

                Task::perform(
                    async move {
                        let layout = BackupLayout::new(restore_path);
                        let restorables = layout.restorable_games();
                        (layout, restorables)
                    },
                    move |(layout, restorables)| {
                        Message::Restore(RestorePhase::RegisterCommands { layout, restorables })
                    },
                )
            }
            RestorePhase::RegisterCommands {
                mut restorables,
                layout,
            } => {
                log::info!("beginning restore with {} steps", restorables.len());
                let preview = self.operation.preview();
                let games = self.operation.games();
                let single = games.is_some_and(|x| x.is_single());

                if self.operation_should_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    self.go_idle();
                    return Task::none();
                }

                if let Some(games) = &games {
                    restorables.retain(|v| games.contains(v));
                    self.restore_screen.log.unscan_games(games);
                }

                if restorables.is_empty() {
                    if let Some(games) = games {
                        for game in games.iter() {
                            let duplicates = self.restore_screen.duplicate_detector.remove_game(game);
                            self.restore_screen.log.remove_game(
                                game,
                                &self.restore_screen.duplicate_detector,
                                &duplicates,
                                &self.config,
                                SCAN_KIND,
                            );
                        }
                        self.cache.restore.recent_games.retain(|x| !games.contains(x));
                        self.save_cache();
                    }
                    self.go_idle();
                    return Task::none();
                }

                self.progress.set_max(restorables.len() as f32);

                self.register_notify_on_single_game_scanned();

                let config = std::sync::Arc::new(self.config.clone());
                let layout = std::sync::Arc::new(layout);

                for name in restorables {
                    let config = config.clone();
                    let layout = layout.clone();
                    let cancel_flag = self.operation_should_cancel.clone();
                    let backup_id = self.backups_to_restore.get(&name).cloned().unwrap_or(BackupId::Latest);
                    self.operation_steps.push(OperationStep {
                        title: name.clone(),
                        task: Task::perform(
                            async move {
                                let mut layout = layout.game_layout(&name);

                                if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                                    return (None, None, layout);
                                }

                                let scan_info = layout.scan_for_restoration(
                                    &name,
                                    &backup_id,
                                    &config.redirects,
                                    config.restore.reverse_redirects,
                                    &config.restore.toggled_paths,
                                    &config.restore.toggled_registry,
                                );
                                if !config.is_game_enabled_for_restore(&name) && !single {
                                    return (Some(scan_info), None, layout);
                                }

                                let backup_info = if scan_info.backup.is_some() && !preview {
                                    Some(layout.restore(&scan_info, &config.restore.toggled_registry))
                                } else {
                                    None
                                };
                                (Some(scan_info), backup_info, layout)
                            },
                            move |(scan_info, backup_info, game_layout)| {
                                Message::Restore(RestorePhase::GameScanned {
                                    scan_info,
                                    backup_info,
                                    game_layout: Box::new(game_layout),
                                })
                            },
                        ),
                    });
                }

                self.operation_steps_active = 100.min(self.operation_steps.len());

                let mut tasks = vec![];
                for step in self.operation_steps.drain(..self.operation_steps_active) {
                    self.operation.add_active_game(step.title);
                    tasks.push(step.task);
                }
                Task::batch(tasks)
            }
            RestorePhase::GameScanned {
                scan_info,
                backup_info,
                game_layout,
            } => {
                self.progress.step();
                let full = self.operation.full();

                if let Some(mut scan_info) = scan_info {
                    log::trace!(
                        "step {} / {}: {}",
                        self.progress.current,
                        self.progress.max,
                        scan_info.game_name
                    );
                    self.operation.remove_active_game(&scan_info.game_name);
                    if scan_info.can_report_game() {
                        if let Some(backup_info) = backup_info.as_ref() {
                            scan_info.clear_processed_changes(backup_info, SCAN_KIND);
                        }

                        let comment = scan_info.backup.as_ref().and_then(|x| x.comment()).map(|x| x.as_str());
                        self.text_histories.backup_comments.insert(
                            scan_info.game_name.clone(),
                            TextHistory::raw(comment.unwrap_or_default()),
                        );

                        let duplicates = self.restore_screen.duplicate_detector.add_game(
                            &scan_info,
                            self.config
                                .is_game_enabled_for_operation(&scan_info.game_name, SCAN_KIND),
                        );
                        self.restore_screen.log.update_game(
                            scan_info,
                            backup_info,
                            &self.config.backup.sort,
                            &self.restore_screen.duplicate_detector,
                            &duplicates,
                            Some(*game_layout),
                            &self.config,
                            SCAN_KIND,
                        );
                    } else if !full {
                        let duplicates = self.restore_screen.duplicate_detector.remove_game(&scan_info.game_name);
                        self.restore_screen.log.remove_game(
                            &scan_info.game_name,
                            &self.restore_screen.duplicate_detector,
                            &duplicates,
                            &self.config,
                            SCAN_KIND,
                        );
                        self.cache.restore.recent_games.remove(&scan_info.game_name);
                    }
                } else {
                    log::trace!(
                        "step {} / {}, awaiting {}",
                        self.progress.current,
                        self.progress.max,
                        self.operation_steps_active
                    );
                }

                match self.operation_steps.pop() {
                    Some(step) => {
                        self.operation.add_active_game(step.title);
                        step.task
                    }
                    None => {
                        self.operation_steps_active -= 1;
                        if self.operation_steps_active == 0 {
                            self.handle_restore(RestorePhase::Done)
                        } else {
                            Task::none()
                        }
                    }
                }
            }
            RestorePhase::Done => {
                log::info!("completed restore");
                let mut failed = false;
                let full = self.operation.full();

                self.handle_notify_on_single_game_scanned();

                if full {
                    self.cache.restore.recent_games.clear();
                }

                for entry in &self.restore_screen.log.entries {
                    self.cache
                        .restore
                        .recent_games
                        .insert(entry.scan_info.game_name.clone());
                    if let Some(backup_info) = &entry.backup_info {
                        if !backup_info.successful() {
                            failed = true;
                        }
                    }
                }

                self.save_cache();

                if failed {
                    self.operation.push_error(Error::SomeEntriesFailed);
                }

                let errors = self.operation.errors().cloned();
                self.go_idle();

                if let Some(errors) = errors {
                    if !errors.is_empty() {
                        return self.show_modal(Modal::Errors { errors });
                    }
                }

                Task::none()
            }
        }
    }

    fn handle_validation(&mut self, phase: ValidatePhase) -> Task<Message> {
        match phase {
            ValidatePhase::Start => {
                if !self.operation.idle() {
                    return Task::none();
                }

                let path = self.config.restore.path.clone();
                if !path.is_dir() {
                    return self.show_modal(Modal::Error {
                        variant: Error::RestorationSourceInvalid { path },
                    });
                }

                self.operation = Operation::new_validate_backups();

                self.invalidate_path_caches();
                self.timed_notification = None;

                Task::batch([self.close_modal(), self.handle_validation(ValidatePhase::Load)])
            }
            ValidatePhase::Load => {
                let restore_path = self.config.restore.path.clone();

                self.progress.start();

                Task::perform(
                    async move {
                        let layout = BackupLayout::new(restore_path);
                        let subjects = layout.restorable_games();
                        (layout, subjects)
                    },
                    move |(layout, subjects)| {
                        Message::ValidateBackups(ValidatePhase::RegisterCommands { layout, subjects })
                    },
                )
            }
            ValidatePhase::RegisterCommands { subjects, layout } => {
                log::info!("beginning validation with {} steps", subjects.len());

                if self.operation_should_cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    self.go_idle();
                    return Task::none();
                }

                if subjects.is_empty() {
                    self.go_idle();
                    return Task::none();
                }

                self.progress.set_max(subjects.len() as f32);

                let layout = std::sync::Arc::new(layout);

                for name in subjects {
                    let layout = layout.clone();
                    let cancel_flag = self.operation_should_cancel.clone();
                    let backup_id = self.backups_to_restore.get(&name).cloned().unwrap_or(BackupId::Latest);
                    self.operation_steps.push(OperationStep {
                        title: name.clone(),
                        task: Task::perform(
                            async move {
                                if cancel_flag.load(std::sync::atomic::Ordering::Relaxed) {
                                    return (name, true);
                                }

                                let Some(layout) = layout.try_game_layout(&name) else {
                                    return (name, false);
                                };

                                // TODO: Add an option to validate all backups at once.
                                let valid = layout.validate(backup_id);
                                (name, valid)
                            },
                            move |(game, valid)| Message::ValidateBackups(ValidatePhase::GameScanned { game, valid }),
                        ),
                    });
                }

                self.operation_steps_active = 100.min(self.operation_steps.len());

                let mut tasks = vec![];
                for step in self.operation_steps.drain(..self.operation_steps_active) {
                    self.operation.add_active_game(step.title);
                    tasks.push(step.task);
                }
                Task::batch(tasks)
            }
            ValidatePhase::GameScanned { game, valid } => {
                self.progress.step();
                log::trace!("step {} / {}: {}", self.progress.current, self.progress.max, &game);
                self.operation.remove_active_game(&game);

                if !valid {
                    if let Operation::ValidateBackups { faulty_games, .. } = &mut self.operation {
                        faulty_games.insert(game);
                    }
                }

                match self.operation_steps.pop() {
                    Some(step) => {
                        self.operation.add_active_game(step.title);
                        step.task
                    }
                    None => {
                        self.operation_steps_active -= 1;
                        if self.operation_steps_active == 0 {
                            self.handle_validation(ValidatePhase::Done)
                        } else {
                            Task::none()
                        }
                    }
                }
            }
            ValidatePhase::Done => {
                log::info!("completed validation");
                let faulty_games = if let Operation::ValidateBackups { faulty_games, .. } = &self.operation {
                    faulty_games.clone()
                } else {
                    Default::default()
                };
                self.go_idle();
                self.show_modal(Modal::BackupValidation { games: faulty_games })
            }
        }
    }

    fn transition_from_cloud_step(&mut self) -> Option<Task<Message>> {
        let synced = self.operation.cloud_changes() == 0;

        if self.operation.integrated_checking_cloud() {
            self.operation.transition_from_cloud_step(synced);

            match self.operation {
                Operation::Backup { .. } => Some(self.handle_backup(BackupPhase::Load)),
                Operation::Restore { .. } => Some(self.handle_restore(RestorePhase::Load)),
                Operation::Idle | Operation::ValidateBackups { .. } | Operation::Cloud { .. } => None,
            }
        } else if self.operation.integrated_syncing_cloud() {
            self.operation.transition_from_cloud_step(synced);
            match self.operation {
                Operation::Backup { .. } => Some(self.handle_backup(BackupPhase::Done)),
                Operation::Idle
                | Operation::ValidateBackups { .. }
                | Operation::Restore { .. }
                | Operation::Cloud { .. } => None,
            }
        } else {
            None
        }
    }

    fn cancel_operation(&mut self) -> Task<Message> {
        self.operation_should_cancel
            .swap(true, std::sync::atomic::Ordering::Relaxed);
        self.operation_steps.clear();
        self.operation.flag_cancel();
        if self.operation.is_cloud_active() {
            if let Some(sender) = self.rclone_monitor_sender.as_mut() {
                let _ = sender.try_send(rclone_monitor::Input::Cancel);
            }
        }
        Task::none()
    }

    fn make_custom_game(name: String, manifest: &LoadedManifest) -> CustomGame {
        if let Some(standard) = manifest.extended.0.get(&name) {
            CustomGame {
                name: name.clone(),
                ignore: false,
                integration: config::Integration::Override,
                alias: standard.alias.clone(),
                prefer_alias: false,
                files: standard.files.keys().cloned().collect(),
                registry: standard.registry.keys().cloned().collect(),
                install_dir: standard.install_dir.keys().filter(|x| *x != &name).cloned().collect(),
                wine_prefix: vec![],
                expanded: true,
            }
        } else {
            CustomGame {
                name: name.clone(),
                ignore: false,
                integration: config::Integration::Override,
                alias: None,
                prefer_alias: false,
                files: vec![],
                registry: vec![],
                install_dir: vec![],
                wine_prefix: vec![],
                expanded: true,
            }
        }
    }

    fn customize_game(&mut self, name: String) -> Task<Message> {
        let game = Self::make_custom_game(name, &self.manifest);

        self.text_histories.add_custom_game(&game);
        self.config.custom_games.push(game);
        self.save_config();

        self.scroll_offsets.insert(
            ScrollSubject::CustomGames,
            scrollable::AbsoluteOffset { x: 0.0, y: f32::MAX },
        );
        self.switch_screen(Screen::CustomGames)
    }

    fn customize_game_as_alias(&mut self, name: String) -> Task<Message> {
        let game = CustomGame {
            name: "".to_string(),
            ignore: false,
            integration: config::Integration::Override,
            alias: Some(name),
            prefer_alias: true,
            files: vec![],
            registry: vec![],
            install_dir: vec![],
            wine_prefix: vec![],
            expanded: true,
        };

        self.text_histories.add_custom_game(&game);
        self.config.custom_games.push(game);
        self.save_config();

        self.scroll_offsets.insert(
            ScrollSubject::CustomGames,
            scrollable::AbsoluteOffset { x: 0.0, y: f32::MAX },
        );
        self.switch_screen(Screen::CustomGames)
    }

    fn update_manifest(
        config: config::ManifestConfig,
        cache: cache::Manifests,
        force: bool,
        network_security: Security,
    ) -> Task<Message> {
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || Manifest::update(config, cache, force, network_security)).await
            },
            |join| match join {
                Ok(x) => Message::ManifestUpdated(x),
                Err(_) => Message::Ignore,
            },
        )
    }

    fn open_url(url: String) -> Task<Message> {
        let url2 = url.clone();
        Task::future(async move {
            let result = async { opener::open(url) }.await;

            match result {
                Ok(_) => Message::Ignore,
                Err(e) => {
                    log::error!("Unable to open URL: `{}` - {}", &url2, e);
                    Message::OpenUrlFailure { url: url2 }
                }
            }
        })
    }

    fn open_wiki(game: String) -> Task<Message> {
        let url = format!("https://www.pcgamingwiki.com/wiki/{}", game.replace(' ', "_"));
        Self::open_url(url)
    }

    fn toggle_backup_comment_editor(&mut self, name: String) -> Task<Message> {
        self.restore_screen.log.toggle_backup_comment_editor(&name);
        Task::none()
    }

    fn switch_screen(&mut self, screen: Screen) -> Task<Message> {
        self.screen = screen;
        self.refresh_scroll_position()
    }

    fn scroll_subject(&self) -> ScrollSubject {
        if !self.modals.is_empty() {
            ScrollSubject::Modal
        } else {
            ScrollSubject::from(self.screen.clone())
        }
    }

    fn refresh_scroll_position(&mut self) -> Task<Message> {
        let subject = self.scroll_subject();
        let offset = self.scroll_offsets.get(&subject).copied().unwrap_or_default();

        iced::widget::operation::scroll_to(subject.id(), offset)
    }

    fn refresh_scroll_position_on_log(&mut self, cleared: bool) -> Task<Message> {
        if cleared {
            self.refresh_scroll_position()
        } else {
            Task::none()
        }
    }

    fn reset_scroll_position(&mut self, subject: ScrollSubject) {
        self.scroll_offsets
            .insert(subject, scrollable::AbsoluteOffset::default());
    }

    fn configure_remote(&self, remote: Remote) -> Task<Message> {
        let rclone = self.config.apps.rclone.clone();
        let old_remote = self.config.cloud.remote.clone();
        let new_remote = remote.clone();
        Task::future(async move {
            let result = async {
                if let Some(old_remote) = old_remote {
                    _ = Rclone::new(rclone.clone(), old_remote).unconfigure_remote();
                }
                Rclone::new(rclone, new_remote).configure_remote()
            }
            .await;

            match result {
                Ok(_) => Message::ConfigureCloudSuccess(remote),
                Err(e) => Message::ConfigureCloudFailure(e),
            }
        })
    }

    pub fn new(flags: Flags) -> (Self, Task<Message>) {
        let mut errors = vec![];
        let mut commands = vec![
            iced::font::load(std::borrow::Cow::Borrowed(crate::gui::font::TEXT_DATA)).map(|_| Message::Ignore),
            iced::font::load(std::borrow::Cow::Borrowed(crate::gui::font::ICONS_DATA)).map(|_| Message::Ignore),
            iced::window::oldest().and_then(iced::window::gain_focus),
        ];

        let mut screen = Screen::default();
        let mut modals: Vec<Modal> = vec![];
        let mut pending_save = HashMap::new();

        let mut config = match Config::load() {
            Ok(x) => x,
            Err(x) => {
                errors.push(x);
                let _ = Config::archive_invalid();
                Config::default()
            }
        };
        let mut cache = Cache::load().unwrap_or_default().migrate_config(&mut config);
        TRANSLATOR.set_language(config.language);

        let manifest = if Manifest::path().exists() {
            match Manifest::load() {
                Ok(y) => LoadedManifest {
                    primary: y.clone(),
                    extended: y.with_extensions(&config),
                },
                Err(e) => {
                    errors.push(e);
                    LoadedManifest::default()
                }
            }
        } else {
            if flags.update_manifest {
                modals.push(Modal::UpdatingManifest);
            }
            LoadedManifest::default()
        };

        if let Some(custom_game) = flags.custom_game.as_ref() {
            screen = Screen::CustomGames;

            if let Some(entry) = config.custom_games.iter_mut().find(|entry| &entry.name == custom_game) {
                entry.expanded = true;
            } else {
                let game = Self::make_custom_game(custom_game.clone(), &manifest);
                config.custom_games.push(game);
                pending_save.insert(SaveKind::Config, Instant::now());
            }

            commands.push(
                container_scroll_offset(custom_game.clone().into()).map(move |offset| match offset {
                    Some(position) => Message::Scroll {
                        subject: ScrollSubject::CustomGames,
                        position,
                    },
                    None => Message::Ignore,
                }),
            );
        }

        if !errors.is_empty() {
            modals.push(Modal::Errors { errors });
        } else {
            let missing: Vec<_> = config
                .find_missing_roots()
                .iter()
                .filter(|x| !cache.has_root(x))
                .cloned()
                .collect();
            if !missing.is_empty() {
                cache.add_roots(&missing);
                cache.save();
                modals.push(Modal::ConfirmAddMissingRoots(missing));
            }
        }

        let text_histories = TextHistories::new(&config);

        log::debug!("Config on startup: {config:?}");

        if flags.update_manifest {
            commands.push(Self::update_manifest(
                config.manifest.clone(),
                cache.manifests.clone(),
                false,
                config.runtime.network_security,
            ));
        }

        if config.release.check && cache.should_check_app_update() {
            commands.push(Task::future(async move {
                let result = crate::metadata::Release::fetch(config.runtime.network_security).await;

                Message::AppReleaseChecked(result.map_err(|x| x.to_string()))
            }))
        }

        (
            Self {
                backup_screen: screen::Backup::new(&config, &cache),
                restore_screen: screen::Restore::new(&config, &cache),
                config,
                manifest,
                cache,
                modals,
                updating_manifest: flags.update_manifest,
                text_histories,
                flags,
                screen,
                pending_save,
                sync_games_config: ludusavi::sync::sync_config::SyncGamesConfig::load(),
                ..Self::default()
            },
            Task::batch(commands),
        )
    }

    pub fn title(&self) -> String {
        TRANSLATOR.window_title()
    }

    pub fn theme(&self) -> crate::gui::style::Theme {
        crate::gui::style::Theme::from(self.config.theme)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::EnableCloudSync(game_name) => {
                self.sync_games_config.games.insert(
                    game_name.clone(),
                    ludusavi::sync::sync_config::GameSyncConfig {
                        mode: ludusavi::sync::sync_config::SaveMode::Sync,
                        auto_sync: true,
                    },
                );
                self.sync_games_config.save();
                self.timed_notification = Some(Notification::new("✓ Sync enabled".to_string()).expires(2));
                Task::none()
            }
            Message::Ignore => Task::none(),
            Message::CloseModal => self.close_modal(),
            Message::Exit { user } => {
                if self.operation.idle() || (user && self.exiting) {
                    self.save();
                    std::process::exit(0)
                } else {
                    self.exiting = true;
                    Task::batch([self.show_modal(Modal::Exiting), self.cancel_operation()])
                }
            }
            Message::InstallService => {
                Task::perform(
                    async {
                        #[cfg(target_os = "windows")]
                        {
                            let exe = std::env::current_exe()
                                .ok()
                                .and_then(|p| p.parent().map(|d| d.join("ludusavi-daemon.exe")))
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| "ludusavi-daemon.exe".to_string());
            
                            let script = std::env::current_exe()
                                .ok()
                                .and_then(|p| p.parent().map(|d| d.join("install-service-windows.ps1")))
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| "install-service-windows.ps1".to_string());
            
                            std::process::Command::new("powershell.exe")
                                .args(["-ExecutionPolicy", "Bypass", "-File", &script, "-ExePath", &exe])
                                .output()
                                .map_err(|e| e.to_string())
                                .and_then(|out| if out.status.success() {
                                    Ok(())
                                } else {
                                    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
                                })
                        }
                        #[cfg(not(target_os = "windows"))]
                        {
                            let script = std::env::current_exe()
                                .ok()
                                .and_then(|p| p.parent().map(|d| d.join("install-service-linux.sh")))
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| "install-service-linux.sh".to_string());
            
                            std::process::Command::new("bash")
                                .arg(&script)
                                .output()
                                .map_err(|e| e.to_string())
                                .and_then(|out| if out.status.success() {
                                    Ok(())
                                } else {
                                    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
                                })
                        }
                    },
                    |result| match result {
                        Ok(_) => Message::ShowTimedNotification("✓ Service installed".to_string()),
                        Err(e) => Message::ShowTimedNotification(format!("✗ Install failed: {}", e)),
                    },
                )
            }
            Message::UninstallService => {
                Task::perform(
                    async {
                        #[cfg(target_os = "windows")]
                        {
                            let script = std::env::current_exe()
                                .ok()
                                .and_then(|p| p.parent().map(|d| d.join("uninstall-service-windows.ps1")))
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| "uninstall-service-windows.ps1".to_string());
            
                            std::process::Command::new("powershell.exe")
                                .args(["-ExecutionPolicy", "Bypass", "-File", &script])
                                .output()
                                .map_err(|e| e.to_string())
                                .and_then(|out| if out.status.success() {
                                    Ok(())
                                } else {
                                    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
                                })
                        }
                        #[cfg(not(target_os = "windows"))]
                        {
                            let script = std::env::current_exe()
                                .ok()
                                .and_then(|p| p.parent().map(|d| d.join("uninstall-service-linux.sh")))
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| "uninstall-service-linux.sh".to_string());
            
                            std::process::Command::new("bash")
                                .arg(&script)
                                .output()
                                .map_err(|e| e.to_string())
                                .and_then(|out| if out.status.success() {
                                    Ok(())
                                } else {
                                    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
                                })
                        }
                    },
                    |result| match result {
                        Ok(_) => Message::ShowTimedNotification("✓ Service uninstalled".to_string()),
                        Err(e) => Message::ShowTimedNotification(format!("✗ Uninstall failed: {}", e)),
                    },
                )
            }
            Message::Save => {
                self.save();
                Task::none()
            }
            Message::UpdateTime => {
                self.progress.update_time();
                Task::none()
            }
            Message::PruneNotifications => {
                if let Some(notification) = &self.timed_notification {
                    if notification.expired() {
                        self.timed_notification = None;
                    }
                }
                Task::none()
            }
            Message::DaemonStatusChecked(running, sync_status, game_list) => {
                self.daemon_running = running;
                self.sync_status = sync_status;
                self.game_list = game_list;
                Task::none()
            }
            Message::SetGameSaveMode(game, mode) => {
                let current = self.sync_games_config.games
                    .get(&game)
                    .cloned()
                    .unwrap_or_default();
                let mut pending = self.pending_game_detail.clone().unwrap_or(current);
                pending.mode = mode;
                self.pending_game_detail = Some(pending);
                self.pending_game_detail_name = Some(game);
                Task::none()
            }
            Message::GamesSearchChanged(query) => {
                self.games_search = query;
                Task::none()
            }
            Message::SaveGameDetail => {
                if let (Some(name), Some(pending)) = (self.pending_game_detail_name.clone(), self.pending_game_detail.clone()) {
                    let previous_mode = self.sync_games_config.games
                        .get(&name)
                        .map(|g| g.mode.clone())
                        .unwrap_or(ludusavi::sync::sync_config::SaveMode::None);

                    let new_mode = pending.mode.clone();

                    // Cambio destructivo: CLOUD/SYNC → LOCAL/NONE borra el ZIP del cloud
                    let cloud_to_local = matches!(previous_mode,
                        ludusavi::sync::sync_config::SaveMode::Cloud |
                        ludusavi::sync::sync_config::SaveMode::Sync
                    ) && matches!(new_mode,
                        ludusavi::sync::sync_config::SaveMode::Local |
                        ludusavi::sync::sync_config::SaveMode::None
                    );

                    // Cambio a NONE borra también el backup local
                    let to_none = matches!(new_mode, ludusavi::sync::sync_config::SaveMode::None)
                        && !matches!(previous_mode, ludusavi::sync::sync_config::SaveMode::None);

                    if cloud_to_local || to_none {
                        let warning = if to_none {
                            format!(
                                "Set \"{}\" to None?\n\nThe cloud backup and local backup will be deleted. This cannot be undone.",
                                name
                            )
                        } else {
                            format!(
                                "Switch \"{}\" to Local mode?\n\nThe cloud backup will be deleted. This cannot be undone.",
                                name
                            )
                        };
                        return self.show_modal(Modal::ConfirmSyncModeChange {
                            game: name,
                            warning,
                            previous_mode,
                        });
                    }

                    // Sin warning necesario — guardar directamente
                    self.pending_game_detail_name.take();
                    self.pending_game_detail.take();
                    self.sync_games_config.games.insert(name, pending);
                    self.sync_games_config.save();
                    self.timed_notification = Some(Notification::new("✓ Saved".to_string()).expires(2));
                }
                Task::none()
            }
            Message::SetGameAutoSync(game, auto_sync) => {
                let current = self.sync_games_config.games
                    .get(&game)
                    .cloned()
                    .unwrap_or_default();
                let mut pending = self.pending_game_detail.clone().unwrap_or(current);
                pending.auto_sync = auto_sync;
                self.pending_game_detail = Some(pending);
                self.pending_game_detail_name = Some(game);
                Task::none()
            }
            Message::Config { event } => {
                let mut task = None;

                match event {
                    config::Event::Theme(theme) => {
                        self.config.theme = theme;
                    }
                    config::Event::Language(language) => {
                        TRANSLATOR.set_language(language);
                        self.config.language = language;
                    }
                    config::Event::CheckRelease(enabled) => {
                        self.config.release.check = enabled;
                    }
                    config::Event::BackupTarget(text) => {
                        self.text_histories.backup_target.push(&text);
                        self.config.backup.path.reset(text);
                    }
                    config::Event::RestoreSource(text) => {
                        self.text_histories.restore_source.push(&text);
                        self.config.restore.path.reset(text);
                    }
                    config::Event::Root(action) => match action {
                        EditAction::Add => {
                            self.text_histories.roots.push(Default::default());
                            self.config.roots.push(Root::default());
                        }
                        EditAction::Change(index, value) => {
                            self.text_histories.roots[index].path.push(&value);
                            self.config.roots[index].path_mut().reset(value);
                        }
                        EditAction::Remove(index) => {
                            self.text_histories.roots.remove(index);
                            self.config.roots.remove(index);
                        }
                        EditAction::Move(index, direction) => {
                            let offset = direction.shift(index);
                            self.text_histories.roots.swap(index, offset);
                            self.config.roots.swap(index, offset);
                        }
                    },
                    config::Event::RootLutrisDatabase(index, value) => {
                        self.text_histories.roots[index].lutris_database.push(&value);
                        if let Root::Lutris(root) = &mut self.config.roots[index] {
                            root.database = if value.is_empty() { None } else { Some(value.into()) };
                        }
                    }
                    config::Event::SecondaryManifest(action) => match action {
                        EditAction::Add => {
                            self.text_histories.secondary_manifests.push(Default::default());
                            self.config.manifest.secondary.push(Default::default());
                        }
                        EditAction::Change(index, value) => {
                            self.text_histories.secondary_manifests[index].push(&value);
                            self.config.manifest.secondary[index].set(value);
                        }
                        EditAction::Remove(index) => {
                            self.text_histories.secondary_manifests.remove(index);
                            self.config.manifest.secondary.remove(index);
                        }
                        EditAction::Move(index, direction) => {
                            let offset = direction.shift(index);
                            self.text_histories.secondary_manifests.swap(index, offset);
                            self.config.manifest.secondary.swap(index, offset);
                        }
                    },
                    config::Event::RootStore(index, store) => {
                        self.text_histories.roots[index].clear_secondary();
                        self.config.roots[index].set_store(store);
                    }
                    config::Event::RedirectKind(index, kind) => {
                        self.config.redirects[index].kind = kind;
                    }
                    config::Event::SecondaryManifestKind(index, kind) => {
                        self.config.manifest.secondary[index].convert(kind);
                    }
                    config::Event::CustomGameKind(index, kind) => {
                        self.config.custom_games[index].convert(kind);
                        match kind {
                            CustomGameKind::Game => {
                                self.text_histories.custom_games[index].alias.clear();
                            }
                            CustomGameKind::Alias => {}
                        }
                    }
                    config::Event::CustomGameIntegration(index, integration) => {
                        self.config.custom_games[index].integration = integration;
                    }
                    config::Event::Redirect(action, field) => {
                        // TODO: Automatically refresh redirected paths in the game list.
                        match action {
                            EditAction::Add => {
                                self.text_histories.redirects.push(Default::default());
                                self.config.add_redirect(&StrictPath::default(), &StrictPath::default());
                            }
                            EditAction::Change(index, value) => match field {
                                Some(RedirectEditActionField::Source) => {
                                    self.text_histories.redirects[index].source.push(&value);
                                    self.config.redirects[index].source.reset(value);
                                }
                                Some(RedirectEditActionField::Target) => {
                                    self.text_histories.redirects[index].target.push(&value);
                                    self.config.redirects[index].target.reset(value);
                                }
                                _ => {}
                            },
                            EditAction::Remove(index) => {
                                self.text_histories.redirects.remove(index);
                                self.config.redirects.remove(index);
                            }
                            EditAction::Move(index, direction) => {
                                let offset = direction.shift(index);
                                self.text_histories.redirects.swap(index, offset);
                                self.config.redirects.swap(index, offset);
                            }
                        }
                    }
                    config::Event::ReverseRedirectsOnRestore(enabled) => {
                        self.config.restore.reverse_redirects = enabled;
                    }
                    config::Event::CustomGame(action) => {
                        let mut snap = false;
                        match action {
                            EditAction::Add => {
                                self.text_histories.custom_games.push(Default::default());
                                self.config.add_custom_game();
                                snap = true;
                            }
                            EditAction::Change(index, value) => {
                                self.text_histories.custom_games[index].name.push(&value);
                                self.config.custom_games[index].name = value;
                            }
                            EditAction::Remove(index) => {
                                self.text_histories.custom_games.remove(index);
                                self.config.custom_games.remove(index);
                            }
                            EditAction::Move(index, direction) => {
                                let offset = direction.shift(index);
                                self.text_histories.custom_games.swap(index, offset);
                                self.config.custom_games.swap(index, offset);
                            }
                        }
                        if snap {
                            self.scroll_offsets.insert(
                                ScrollSubject::CustomGames,
                                scrollable::AbsoluteOffset { x: 0.0, y: f32::MAX },
                            );
                            task = Some(self.refresh_scroll_position());
                        }
                    }
                    config::Event::CustomGameAlias(index, value) => {
                        self.text_histories.custom_games[index].alias.push(&value);
                        self.config.custom_games[index].alias = Some(value);
                    }
                    config::Event::CustomGaleAliasDisplay(index, value) => {
                        self.config.custom_games[index].prefer_alias = value;
                    }
                    config::Event::CustomGameFile(game_index, action) => match action {
                        EditAction::Add => {
                            self.text_histories.custom_games[game_index]
                                .files
                                .push(Default::default());
                            self.config.custom_games[game_index].files.push("".to_string());
                        }
                        EditAction::Change(index, value) => {
                            self.text_histories.custom_games[game_index].files[index].push(&value);
                            self.config.custom_games[game_index].files[index] = value;
                        }
                        EditAction::Remove(index) => {
                            self.text_histories.custom_games[game_index].files.remove(index);
                            self.config.custom_games[game_index].files.remove(index);
                        }
                        EditAction::Move(index, direction) => {
                            let offset = direction.shift(index);
                            self.text_histories.custom_games[game_index].files.swap(index, offset);
                            self.config.custom_games[game_index].files.swap(index, offset);
                        }
                    },
                    config::Event::CustomGameRegistry(game_index, action) => match action {
                        EditAction::Add => {
                            self.text_histories.custom_games[game_index]
                                .registry
                                .push(Default::default());
                            self.config.custom_games[game_index].registry.push("".to_string());
                        }
                        EditAction::Change(index, value) => {
                            self.text_histories.custom_games[game_index].registry[index].push(&value);
                            self.config.custom_games[game_index].registry[index] = value;
                        }
                        EditAction::Remove(index) => {
                            self.text_histories.custom_games[game_index].registry.remove(index);
                            self.config.custom_games[game_index].registry.remove(index);
                        }
                        EditAction::Move(index, direction) => {
                            let offset = direction.shift(index);
                            self.text_histories.custom_games[game_index]
                                .registry
                                .swap(index, offset);
                            self.config.custom_games[game_index].registry.swap(index, offset);
                        }
                    },
                    config::Event::CustomGameInstallDir(game_index, action) => match action {
                        EditAction::Add => {
                            self.text_histories.custom_games[game_index]
                                .install_dir
                                .push(Default::default());
                            self.config.custom_games[game_index].install_dir.push("".to_string());
                        }
                        EditAction::Change(index, value) => {
                            self.text_histories.custom_games[game_index].install_dir[index].push(&value);
                            self.config.custom_games[game_index].install_dir[index] = value;
                        }
                        EditAction::Remove(index) => {
                            self.text_histories.custom_games[game_index].install_dir.remove(index);
                            self.config.custom_games[game_index].install_dir.remove(index);
                        }
                        EditAction::Move(index, direction) => {
                            let offset = direction.shift(index);
                            self.text_histories.custom_games[game_index]
                                .install_dir
                                .swap(index, offset);
                            self.config.custom_games[game_index].install_dir.swap(index, offset);
                        }
                    },
                    config::Event::CustomGameWinePrefix(game_index, action) => match action {
                        EditAction::Add => {
                            self.text_histories.custom_games[game_index]
                                .wine_prefix
                                .push(Default::default());
                            self.config.custom_games[game_index].wine_prefix.push("".to_string());
                        }
                        EditAction::Change(index, value) => {
                            self.text_histories.custom_games[game_index].wine_prefix[index].push(&value);
                            self.config.custom_games[game_index].wine_prefix[index] = value;
                        }
                        EditAction::Remove(index) => {
                            self.text_histories.custom_games[game_index].wine_prefix.remove(index);
                            self.config.custom_games[game_index].wine_prefix.remove(index);
                        }
                        EditAction::Move(index, direction) => {
                            let offset = direction.shift(index);
                            self.text_histories.custom_games[game_index]
                                .wine_prefix
                                .swap(index, offset);
                            self.config.custom_games[game_index].wine_prefix.swap(index, offset);
                        }
                    },
                    config::Event::ExcludeStoreScreenshots(enabled) => {
                        self.config.backup.filter.exclude_store_screenshots = enabled;
                    }
                    config::Event::CloudFilter(filter) => {
                        self.config.backup.filter.cloud = filter;
                    }
                    config::Event::BackupFilterIgnoredPath(action) => {
                        match action {
                            EditAction::Add => {
                                self.text_histories.backup_filter_ignored_paths.push(Default::default());
                                self.config
                                    .backup
                                    .filter
                                    .ignored_paths
                                    .push(StrictPath::new("".to_string()));
                            }
                            EditAction::Change(index, value) => {
                                self.text_histories.backup_filter_ignored_paths[index].push(&value);
                                self.config.backup.filter.ignored_paths[index] = StrictPath::new(value);
                            }
                            EditAction::Remove(index) => {
                                self.text_histories.backup_filter_ignored_paths.remove(index);
                                self.config.backup.filter.ignored_paths.remove(index);
                            }
                            EditAction::Move(index, direction) => {
                                let offset = direction.shift(index);
                                self.text_histories.backup_filter_ignored_paths.swap(index, offset);
                                self.config.backup.filter.ignored_paths.swap(index, offset);
                            }
                        }
                        self.config.backup.filter.build_globs();
                    }
                    config::Event::BackupFilterIgnoredRegistry(action) => match action {
                        EditAction::Add => {
                            self.text_histories
                                .backup_filter_ignored_registry
                                .push(Default::default());
                            self.config
                                .backup
                                .filter
                                .ignored_registry
                                .push(RegistryItem::new("".to_string()));
                        }
                        EditAction::Change(index, value) => {
                            self.text_histories.backup_filter_ignored_registry[index].push(&value);
                            self.config.backup.filter.ignored_registry[index] = RegistryItem::new(value);
                        }
                        EditAction::Remove(index) => {
                            self.text_histories.backup_filter_ignored_registry.remove(index);
                            self.config.backup.filter.ignored_registry.remove(index);
                        }
                        EditAction::Move(index, direction) => {
                            let offset = direction.shift(index);
                            self.text_histories.backup_filter_ignored_registry.swap(index, offset);
                            self.config.backup.filter.ignored_registry.swap(index, offset);
                        }
                    },
                    config::Event::GameListEntryEnabled {
                        name,
                        enabled,
                        scan_kind,
                    } => {
                        match (scan_kind, enabled) {
                            (ScanKind::Backup, false) => self.config.disable_game_for_backup(&name),
                            (ScanKind::Backup, true) => self.config.enable_game_for_backup(&name),
                            (ScanKind::Restore, false) => self.config.disable_game_for_restore(&name),
                            (ScanKind::Restore, true) => self.config.enable_game_for_restore(&name),
                        };

                        match scan_kind {
                            ScanKind::Backup => {
                                self.backup_screen.log.refresh_game_tree(
                                    &name,
                                    &self.config,
                                    &mut self.backup_screen.duplicate_detector,
                                    scan_kind,
                                );
                            }
                            ScanKind::Restore => {
                                self.restore_screen.log.refresh_game_tree(
                                    &name,
                                    &self.config,
                                    &mut self.restore_screen.duplicate_detector,
                                    scan_kind,
                                );
                            }
                        }
                    }
                    config::Event::CustomGameEnabled { index, enabled } => {
                        if enabled {
                            self.config.enable_custom_game(index);
                        } else {
                            self.config.disable_custom_game(index);
                        }
                    }
                    config::Event::PrimaryManifestEnabled { enabled } => {
                        self.config.manifest.enable = enabled;
                    }
                    config::Event::SecondaryManifestEnabled { index, enabled } => {
                        self.config.manifest.secondary[index].enable(enabled);
                    }
                    config::Event::ToggleSpecificGamePathIgnored { name, path, scan_kind } => match scan_kind {
                        ScanKind::Backup => {
                            self.config.backup.toggled_paths.toggle(&name, &path);
                            self.backup_screen.log.refresh_game_tree(
                                &name,
                                &self.config,
                                &mut self.backup_screen.duplicate_detector,
                                scan_kind,
                            );
                        }
                        ScanKind::Restore => {
                            self.config.restore.toggled_paths.toggle(&name, &path);
                            self.restore_screen.log.refresh_game_tree(
                                &name,
                                &self.config,
                                &mut self.restore_screen.duplicate_detector,
                                scan_kind,
                            );
                        }
                    },
                    config::Event::ToggleSpecificGameRegistryIgnored {
                        name,
                        path,
                        value,
                        scan_kind,
                    } => match scan_kind {
                        ScanKind::Backup => {
                            self.config.backup.toggled_registry.toggle_owned(&name, &path, value);
                            self.backup_screen.log.refresh_game_tree(
                                &name,
                                &self.config,
                                &mut self.backup_screen.duplicate_detector,
                                scan_kind,
                            );
                        }
                        ScanKind::Restore => {
                            self.config.restore.toggled_registry.toggle_owned(&name, &path, value);
                            self.restore_screen.log.refresh_game_tree(
                                &name,
                                &self.config,
                                &mut self.restore_screen.duplicate_detector,
                                scan_kind,
                            );
                        }
                    },
                    config::Event::SortKey(value) => match self.screen {
                        Screen::Backup => {
                            self.config.backup.sort.key = value;
                            self.backup_screen.log.sort(&self.config.backup.sort, &self.config);
                        }
                        Screen::Restore => {
                            self.config.restore.sort.key = value;
                            self.restore_screen.log.sort(&self.config.restore.sort, &self.config);
                        }
                        _ => {}
                    },
                    config::Event::SortReversed(value) => match self.screen {
                        Screen::Backup => {
                            self.config.backup.sort.reversed = value;
                            self.backup_screen.log.sort(&self.config.backup.sort, &self.config);
                        }
                        Screen::Restore => {
                            self.config.restore.sort.reversed = value;
                            self.restore_screen.log.sort(&self.config.restore.sort, &self.config);
                        }
                        _ => {}
                    },
                    config::Event::FullRetention(value) => {
                        self.config.backup.retention.full = value;
                    }
                    config::Event::DiffRetention(value) => {
                        self.config.backup.retention.differential = value;
                    }
                    config::Event::BackupFormat(format) => {
                        self.config.backup.format.chosen = format;
                    }
                    config::Event::BackupCompression(compression) => {
                        self.config.backup.format.zip.compression = compression;
                    }
                    config::Event::CompressionLevel(value) => {
                        self.config.backup.format.set_level(value);
                    }
                    config::Event::ToggleCloudSynchronize => {
                        self.config.cloud.synchronize = !self.config.cloud.synchronize;
                    }
                    config::Event::ShowDeselectedGames(value) => {
                        self.config.scan.show_deselected_games = value;
                    }
                    config::Event::ShowUnchangedGames(value) => {
                        self.config.scan.show_unchanged_games = value;
                    }
                    config::Event::ShowUnscannedGames(value) => {
                        self.config.scan.show_unscanned_games = value;
                    }
                    config::Event::OverrideMaxThreads(overridden) => {
                        self.config.override_threads(overridden);
                    }
                    config::Event::MaxThreads(threads) => {
                        self.config.set_threads(threads);
                    }
                    config::Event::RcloneExecutable(text) => {
                        self.text_histories.rclone_executable.push(&text);
                        self.config.apps.rclone.path.reset(text);
                    }
                    config::Event::RcloneArguments(text) => {
                        self.text_histories.rclone_arguments.push(&text);
                        self.config.apps.rclone.arguments = text;
                    }
                    config::Event::CloudRemoteId(text) => {
                        self.text_histories.cloud_remote_id.push(&text);
                        if let Some(Remote::Custom { id }) = &mut self.config.cloud.remote {
                            *id = text;
                        }
                    }
                    config::Event::CloudPath(text) => {
                        self.text_histories.cloud_path.push(&text);
                        self.config.cloud.path = text;
                    }
                    config::Event::SortCustomGames => {
                        self.config.custom_games.sort_by(|x, y| x.name.cmp(&y.name));
                        self.text_histories
                            .custom_games
                            .sort_by(|x, y| x.name.current().cmp(&y.name.current()));
                    }
                    config::Event::OnlyConstructiveBackups(value) => {
                        self.config.backup.only_constructive = value;
                        for entry in &mut self.backup_screen.log.entries {
                            entry.scan_info.only_constructive_backups = value;
                        }
                    }
                }

                self.save_config();
                task.unwrap_or_else(Task::none)
            }
            Message::CheckAppRelease => {
                if !self.cache.should_check_app_update() {
                    return Task::none();
                }

                let security = self.config.runtime.network_security;

                Task::future(async move {
                    let result = crate::metadata::Release::fetch(security).await;

                    Message::AppReleaseChecked(result.map_err(|x| x.to_string()))
                })
            }
            Message::AppReleaseChecked(outcome) => {
                self.save_cache();
                self.cache.release.checked = chrono::offset::Utc::now();

                match outcome {
                    Ok(release) => {
                        let previous_latest = self.cache.release.latest.clone();
                        self.cache.release.latest = Some(release.version.clone());

                        if previous_latest.as_ref() != Some(&release.version) {
                            // The latest available version has changed (or this is our first time checking)
                            if release.is_update() {
                                return self.show_modal(Modal::AppUpdate { release });
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("App update check failed: {e:?}");
                    }
                }

                Task::none()
            }
            Message::UpdateManifest { force } => {
                if self.updating_manifest {
                    return Task::none();
                }

                self.updating_manifest = true;
                self.manifest_notification = Some(Notification::new(TRANSLATOR.updating_manifest()));
                Self::update_manifest(
                    self.config.manifest.clone(),
                    self.cache.manifests.clone(),
                    force,
                    self.config.runtime.network_security,
                )
            }
            Message::ManifestUpdated(updates) => {
                self.updating_manifest = false;
                self.manifest_notification = None;
                let mut errors = vec![];

                for update in updates {
                    match update {
                        Ok(Some(update)) => {
                            self.cache.update_manifest(update);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            errors.push(e);
                        }
                    }
                }

                self.save_cache();

                match Manifest::load() {
                    Ok(x) => {
                        self.manifest = LoadedManifest {
                            primary: x.clone(),
                            extended: x.with_extensions(&self.config),
                        };
                    }
                    Err(e) => {
                        errors.push(e);
                    }
                }

                let mut tasks = vec![self.close_specific_modal(modal::Kind::UpdatingManifest)];
                if !errors.is_empty() {
                    tasks.push(self.show_modal(Modal::Errors { errors }));
                }

                Task::batch(tasks)
            }
            Message::Backup(phase) => self.handle_backup(phase),
            Message::Restore(phase) => self.handle_restore(phase),
            Message::ValidateBackups(phase) => self.handle_validation(phase),
            Message::CancelOperation => self.cancel_operation(),
            Message::RequestSyncBackup(game_name) => {
                self.show_modal(Modal::ConfirmSyncBackup { game: game_name })
            }
            Message::RequestSyncRestore(game_name) => {
                self.show_modal(Modal::ConfirmSyncRestore { game: game_name })
            }
            Message::RequestForceUpload(game_name) => {
                self.show_modal(Modal::ConfirmForceUpload { game: game_name })
            }
            Message::RequestForceDownload(game_name) => {
                self.show_modal(Modal::ConfirmForceDownload { game: game_name })
            }
            Message::ConfirmSyncModeChange { game, previous_mode } => {
                // Ejecutar el cambio de modo que ya estaba pendiente
                if let (Some(name), Some(pending)) = (self.pending_game_detail_name.take(), self.pending_game_detail.take()) {
                    self.sync_games_config.games.insert(name.clone(), pending);
                    self.sync_games_config.save();
                    self.timed_notification = Some(Notification::new("✓ Saved".to_string()).expires(2));
                    // Si el modo anterior era CLOUD o SYNC, borrar ZIP del cloud
                    let should_delete = matches!(previous_mode,
                        ludusavi::sync::sync_config::SaveMode::Cloud |
                        ludusavi::sync::sync_config::SaveMode::Sync
                    );
                    if should_delete {
                        let config = self.config.clone();
                        let game_name = game.clone();
                        return Task::batch([
                            self.close_modal(),
                            Task::perform(
                                async move {
                                    ludusavi::sync::operations::delete_game_zip_from_cloud(&config, &game_name)
                                        .map_err(|e| e.to_string())
                                },
                                |result| match result {
                                    Ok(_) => Message::ShowTimedNotification("✓ Saved. Cloud backup removed.".to_string()),
                                    Err(e) => Message::ShowTimedNotification(format!("✓ Saved. Warning: could not remove cloud backup: {}", e)),
                                },
                            ),
                        ]);
                    }
                }
                self.close_modal()
            }
            Message::SyncBackupGame(game_name) => {
                self.sync_in_progress = Some("⏳ Backing up...".to_string());
                self.close_specific_modal_alt(modal::Kind::ConfirmSyncBackup);
                let config = self.config.clone();
                let app_dir = crate::prelude::app_dir();
                let game_list = self.game_list.clone();
                let sync_config = self.sync_games_config.clone();

                Task::perform(
                    async move {
                        let device = ludusavi::sync::device::DeviceIdentity::load_or_create(&app_dir);
                        let mode = sync_config.get_mode(&game_name);

                        // Obtener o resolver la ruta local
                        let local_path = if let Some(game) = game_list.games.iter().find(|g| g.id == game_name) {
                            if let Some(path) = game.path_by_device.get(&device.id) {
                                path.clone()
                            } else {
                                // Ruta no registrada para este device, intentar resolverla
                                ludusavi::sync::operations::resolve_game_path_from_manifest(&config, &game_name)
                                    .ok_or_else(|| format!("Cannot resolve save path for: {}", game_name))?
                            }
                        } else {
                            // Juego no está en el game-list, intentar resolverlo
                            ludusavi::sync::operations::resolve_game_path_from_manifest(&config, &game_name)
                                .ok_or_else(|| format!("Cannot resolve save path for: {}", game_name))?
                        };

                        log::info!("[SyncBackupGame] Resolved path for {}: {}", game_name, local_path);

                        // Leer o crear el game-list
                        let mut gl = ludusavi::sync::operations::read_game_list_from_cloud(&config)
                            .unwrap_or_default();

                        // Asegurar que el juego está registrado con la ruta correcta
                        match gl.get_game_mut(&game_name) {
                            Some(existing) => {
                                existing.path_by_device.insert(device.id.clone(), local_path.clone());
                            }
                            None => {
                                let mut meta = ludusavi::sync::game_list::GameMetaData::new(
                                    game_name.clone(),
                                    game_name.clone(),
                                );
                                meta.path_by_device.insert(device.id.clone(), local_path.clone());
                                gl.upsert_game(meta);
                            }
                        }

                        match mode {
                            ludusavi::sync::sync_config::SaveMode::Local => {
                                let zip_path = config.backup.path.joined(&format!("{}.zip", game_name));
                                ludusavi::sync::operations::create_zip_from_folder(&local_path, &zip_path)
                                    .map_err(|e| e.to_string())?;
                                log::info!("[SyncBackupGame] Local ZIP created for {}", game_name);
                                Ok(())
                            }
                            ludusavi::sync::sync_config::SaveMode::Cloud => {
                                let game_mut = gl.get_game_mut(&game_name)
                                    .ok_or_else(|| "Game not found after upsert".to_string())?;
                                ludusavi::sync::operations::upload_game(&config, &app_dir, &device, game_mut)
                                    .map_err(|e| e.to_string())?;
                                ludusavi::sync::operations::write_game_list_to_cloud(&config, &gl)
                                    .map_err(|e| e.to_string())?;
                                log::info!("[SyncBackupGame] Cloud upload complete for {}", game_name);
                                Ok(())
                            }
                            _ => Err(format!("SyncBackupGame called for unsupported mode: {:?}", mode)),
                        }
                    },
                    |result| match result {
                        Ok(_) => Message::ShowTimedNotification("✓ Backup completed".to_string()),
                        Err(e) => Message::ShowTimedNotification(format!("✗ Error: {}", e)),
                    },
                )
            }
            Message::SyncRestoreGame(game_name) => {
                self.sync_in_progress = Some("⏳ Restoring...".to_string());
                self.close_specific_modal_alt(modal::Kind::ConfirmSyncRestore);
                let config = self.config.clone();
                let app_dir = crate::prelude::app_dir();
                let game_list = self.game_list.clone();
                let sync_config = self.sync_games_config.clone();

                Task::perform(
                    async move {
                        let device = ludusavi::sync::device::DeviceIdentity::load_or_create(&app_dir);
                        let mode = sync_config.get_mode(&game_name);
                        let game = match game_list.games.iter().find(|g| g.id == game_name) {
                            Some(g) => g.clone(),
                            None => return Err(format!("Game not found in game list: {}", game_name)),
                        };

                        match mode {
                            ludusavi::sync::sync_config::SaveMode::Local => {
                                let zip_path = config.backup.path.joined(&format!("{}.zip", game_name));
                                let local_path = match game.path_by_device.get(&device.id) {
                                    Some(p) => p.clone(),
                                    None => return Err(format!("No local path for game: {}", game_name)),
                                };
                                ludusavi::sync::operations::extract_zip_to_directory(&zip_path, &local_path, None)
                                    .map_err(|e| e.to_string())
                            }
                            ludusavi::sync::sync_config::SaveMode::Cloud
                            | ludusavi::sync::sync_config::SaveMode::Sync => {
                                ludusavi::sync::operations::download_game(&config, &app_dir, &device, &game)
                                    .map_err(|e| e.to_string())
                            }
                            _ => Err(format!("SyncRestoreGame called for unsupported mode: {:?}", mode)),
                        }
                    },
                    |result| match result {
                        Ok(_) => Message::ShowTimedNotification("✓ Restore completed".to_string()),
                        Err(e) => Message::ShowTimedNotification(format!("✗ Error: {}", e)),
                    },
                )
            }
            Message::ShowTimedNotification(msg) => {
                self.timed_notification = Some(Notification::new(msg).expires(3));
                self.sync_in_progress = None;
                Task::none()
            }
            Message::SyncNow(game_name) => {
                self.sync_in_progress = Some("⏳ Syncing...".to_string());
                let config = self.config.clone();
                let app_dir = crate::prelude::app_dir();
                let game_list = self.game_list.clone();
                let sync_config = self.sync_games_config.clone();

                Task::perform(
                    async move {
                        let device = ludusavi::sync::device::DeviceIdentity::load_or_create(&app_dir);
                        let mode = sync_config.get_mode(&game_name);

                        let mut gl = ludusavi::sync::operations::read_game_list_from_cloud(&config)
                            .unwrap_or_default();

                        let local_path = if let Some(game) = game_list.games.iter().find(|g| g.id == game_name) {
                            if let Some(path) = game.path_by_device.get(&device.id) {
                                path.clone()
                            } else {
                                ludusavi::sync::operations::resolve_game_path_from_manifest(&config, &game_name)
                                    .ok_or_else(|| format!("Cannot resolve save path for: {}", game_name))?
                            }
                        } else {
                            ludusavi::sync::operations::resolve_game_path_from_manifest(&config, &game_name)
                                .ok_or_else(|| format!("Cannot resolve save path for: {}", game_name))?
                        };

                        match gl.get_game_mut(&game_name) {
                            Some(existing) => {
                                existing.path_by_device.insert(device.id.clone(), local_path.clone());
                            }
                            None => {
                                let mut meta = ludusavi::sync::game_list::GameMetaData::new(
                                    game_name.clone(), game_name.clone(),
                                );
                                meta.path_by_device.insert(device.id.clone(), local_path.clone());
                                gl.upsert_game(meta);
                            }
                        }

                        let game = gl.get_game_mut(&game_name)
                            .ok_or_else(|| "Game not found after upsert".to_string())?;

                        let scan = ludusavi::sync::conflict::DirectoryScanResult::scan(Some(&local_path));
                        let sync_type = ludusavi::sync::conflict::determine_sync_type(game, &scan);

                        match mode {
                            ludusavi::sync::sync_config::SaveMode::Local => {
                                match sync_type {
                                    ludusavi::sync::conflict::SyncStatus::RequiresUpload => {
                                        let zip_path = config.backup.path.joined(&format!("{}.zip", game_name));
                                        ludusavi::sync::operations::create_zip_from_folder(&local_path, &zip_path)
                                            .map_err(|e| e.to_string())?;
                                        Ok("✓ Backup complete".to_string())
                                    }
                                    ludusavi::sync::conflict::SyncStatus::InSync => {
                                        Ok("✓ Already in sync".to_string())
                                    }
                                    _ => Ok("✓ Already in sync".to_string()),
                                }
                            }
                            ludusavi::sync::sync_config::SaveMode::Cloud |
                            ludusavi::sync::sync_config::SaveMode::Sync => {
                                match sync_type {
                                    ludusavi::sync::conflict::SyncStatus::RequiresUpload => {
                                        ludusavi::sync::operations::upload_game(&config, &app_dir, &device, game)
                                            .map_err(|e| e.to_string())?;
                                        ludusavi::sync::operations::write_game_list_to_cloud(&config, &gl)
                                            .map_err(|e| e.to_string())?;
                                        Ok("✓ Upload complete".to_string())
                                    }
                                    ludusavi::sync::conflict::SyncStatus::RequiresDownload => {
                                        ludusavi::sync::operations::download_game(&config, &app_dir, &device, game)
                                            .map_err(|e| e.to_string())?;
                                        Ok("✓ Download complete".to_string())
                                    }
                                    ludusavi::sync::conflict::SyncStatus::InSync => {
                                        Ok("✓ Already in sync".to_string())
                                    }
                                    _ => Ok("✓ Already in sync".to_string()),
                                }
                            }
                            _ => Err(format!("SyncNow called for unsupported mode: {:?}", mode)),
                        }
                    },
                    |result| match result {
                        Ok(msg) => Message::ShowTimedNotification(msg),
                        Err(e) => Message::ShowTimedNotification(format!("✗ Error: {}", e)),
                    },
                )
            }
            Message::ForceUploadGame(game_name) => {
                self.sync_in_progress = Some("⏳ Uploading...".to_string());
                self.close_specific_modal_alt(modal::Kind::ConfirmForceUpload);
                let config = self.config.clone();
                let app_dir = crate::prelude::app_dir();
                let _game_list = self.game_list.clone();

                Task::perform(
                    async move {
                        let device = ludusavi::sync::device::DeviceIdentity::load_or_create(&app_dir);
                        let mut gl = ludusavi::sync::operations::read_game_list_from_cloud(&config)
                            .unwrap_or_default();
                        let game = match gl.get_game_mut(&game_name) {
                            Some(g) => g,
                            None => {
                                // Intentar resolver la ruta y registrar
                                let path = ludusavi::sync::operations::resolve_game_path_from_manifest(&config, &game_name)
                                    .ok_or_else(|| format!("Cannot resolve path for: {}", game_name))?;
                                let mut meta = ludusavi::sync::game_list::GameMetaData::new(
                                    game_name.clone(), game_name.clone(),
                                );
                                meta.path_by_device.insert(device.id.clone(), path);
                                gl.upsert_game(meta);
                                gl.get_game_mut(&game_name).unwrap()
                            }
                        };
                        ludusavi::sync::operations::upload_game(&config, &app_dir, &device, game)
                            .map_err(|e| e.to_string())?;
                        ludusavi::sync::operations::write_game_list_to_cloud(&config, &gl)
                            .map_err(|e| e.to_string())
                    },
                    |result| match result {
                        Ok(_) => Message::ShowTimedNotification("✓ Upload completed".to_string()),
                        Err(e) => Message::ShowTimedNotification(format!("✗ Error: {}", e)),
                    },
                )
            }
            Message::ForceDownloadGame(game_name) => {
                self.sync_in_progress = Some("⏳ Downloading...".to_string());
                self.close_specific_modal_alt(modal::Kind::ConfirmForceDownload);
                let config = self.config.clone();
                let app_dir = crate::prelude::app_dir();
                let game_list = self.game_list.clone();

                Task::perform(
                    async move {
                        let device = ludusavi::sync::device::DeviceIdentity::load_or_create(&app_dir);
                        let game = game_list.games.iter().find(|g| g.id == game_name)
                            .ok_or_else(|| format!("Game not found in game list: {}", game_name))?
                            .clone();
                        ludusavi::sync::operations::download_game(&config, &app_dir, &device, &game)
                            .map_err(|e| e.to_string())
                    },
                    |result| match result {
                        Ok(_) => Message::ShowTimedNotification("✓ Download completed".to_string()),
                        Err(e) => Message::ShowTimedNotification(format!("✗ Error: {}", e)),
                    },
                )
            }
            Message::ShowGameNotes { game, notes } => self.show_modal(Modal::GameNotes { game, notes }),
            Message::FindRoots => {
                let missing = self.config.find_missing_roots();
                if missing.is_empty() {
                    self.show_modal(Modal::NoMissingRoots)
                } else {
                    self.cache.add_roots(&missing);
                    self.save_cache();
                    self.show_modal(Modal::ConfirmAddMissingRoots(missing))
                }
            }
            Message::ConfirmAddMissingRoots(missing) => {
                for root in missing {
                    let path = root.path().render();
                    let lutris_database = root.lutris_database().map(|x| x.render()).unwrap_or_default();

                    if let Some(updated) = self.config.merge_root(&root) {
                        self.text_histories.roots[updated].path.push(&path);
                        self.text_histories.roots[updated]
                            .lutris_database
                            .push(&lutris_database);
                    } else {
                        self.text_histories.roots.push(RootHistory {
                            path: TextHistory::raw(&path),
                            lutris_database: TextHistory::raw(&lutris_database),
                        });
                        self.config.roots.push(root);
                    }
                }
                self.save_config();
                self.close_specific_modal(modal::Kind::ConfirmAddMissingRoots)
            }
            Message::SwitchScreen(screen) => {
                if !matches!(screen, Screen::GameDetail(_)) {
                    self.pending_game_detail = None;
                    self.pending_game_detail_name = None;
                }
                // Lanzar scan automático al entrar a GameDetail
                if let Screen::GameDetail(ref game_name) = screen {
                        self.game_detail_files_expanded = false;
                        let scan_task = self.handle_backup(BackupPhase::Start {
                        preview: true,
                        repair: false,
                        jump: false,
                        games: Some(GameSelection::single(game_name.clone())),
                    });
                    return Task::batch([self.switch_screen(screen), scan_task]);
                }
                self.switch_screen(screen)
            }
            Message::ToggleGameListEntryExpanded { name } => {
                match self.screen {
                    Screen::Backup => {
                        self.backup_screen.log.toggle_game_expanded(
                            &name,
                            &self.backup_screen.duplicate_detector,
                            &self.config,
                            ScanKind::Backup,
                        );
                    }
                    Screen::Restore => {
                        self.restore_screen.log.toggle_game_expanded(
                            &name,
                            &self.restore_screen.duplicate_detector,
                            &self.config,
                            ScanKind::Restore,
                        );
                    }
                    _ => {}
                }
                Task::none()
            }
            Message::ToggleGameListEntryTreeExpanded { name, keys } => {
                match self.screen {
                    Screen::Backup => {
                        for entry in &mut self.backup_screen.log.entries {
                            if entry.scan_info.game_name == name {
                                if let Some(tree) = entry.tree.as_mut() {
                                    tree.expand_or_collapse_keys(&keys);
                                }
                            }
                        }
                    }
                    Screen::Restore => {
                        for entry in &mut self.restore_screen.log.entries {
                            if entry.scan_info.game_name == name {
                                if let Some(tree) = entry.tree.as_mut() {
                                    tree.expand_or_collapse_keys(&keys);
                                }
                            }
                        }
                    }
                    Screen::GameDetail(_) => {
                        eprintln!("[DEBUG] ToggleGameListEntryTreeExpanded: {}", name);
                        for entry in &mut self.backup_screen.log.entries {
                            if entry.scan_info.game_name == name {
                                if let Some(tree) = entry.tree.as_mut() {
                                    tree.expand_or_collapse_keys(&keys);
                                }
                            }
                        }
                    }
                    _ => {}
                }
                Task::none()
            }
            Message::ToggleCustomGameExpanded { index, expanded } => {
                self.config.custom_games[index].expanded = expanded;
                self.save_config();
                Task::none()
            }
            Message::Filter { event } => {
                let mut task = None;

                match event {
                    game_filter::Event::Toggled => match self.screen {
                        Screen::Backup => {
                            self.backup_screen.log.search.show = !self.backup_screen.log.search.show;
                            task = Some(iced::widget::operation::focus(id::backup_search()));
                        }
                        Screen::Restore => {
                            self.restore_screen.log.search.show = !self.restore_screen.log.search.show;
                            task = Some(iced::widget::operation::focus(id::restore_search()));
                        }
                        Screen::CustomGames => {
                            self.custom_games_screen.filter.enabled = !self.custom_games_screen.filter.enabled;
                            task = Some(iced::widget::operation::focus(id::custom_games_search()));
                        }
                        Screen::Games => {
                            self.backup_screen.log.search.show = !self.backup_screen.log.search.show;
                            task = Some(iced::widget::operation::focus(id::backup_search()));
                        }
                        Screen::Other | Screen::GameDetail(_) | Screen::ThisDevice | Screen::AllDevices => {}
                    },
                    game_filter::Event::ToggledFilter { filter, enabled } => match self.screen {
                        Screen::Backup => {
                            self.backup_screen.log.search.toggle_filter(filter, enabled);
                        }
                        Screen::Restore => {
                            self.restore_screen.log.search.toggle_filter(filter, enabled);
                        }
                        Screen::CustomGames => {}
                        Screen::Other | Screen::Games | Screen::GameDetail(_) | Screen::ThisDevice | Screen::AllDevices => {}
                    },
                    game_filter::Event::EditedGameName(value) => match self.screen {
                        Screen::Backup => {
                            self.text_histories.backup_search_game_name.push(&value);
                            self.backup_screen.log.search.game_name = value;
                        }
                        Screen::Restore => {
                            self.text_histories.restore_search_game_name.push(&value);
                            self.restore_screen.log.search.game_name = value;
                        }
                        Screen::CustomGames => {
                            self.text_histories.custom_games_search_game_name.push(&value);
                            self.custom_games_screen.filter.name = value;
                        }
                        Screen::Other | Screen::Games | Screen::GameDetail(_) | Screen::ThisDevice | Screen::AllDevices => {}
                    },
                    game_filter::Event::Reset => match self.screen {
                        Screen::Backup => {
                            self.backup_screen.log.search.reset();
                            self.text_histories.backup_search_game_name.push("");
                        }
                        Screen::Restore => {
                            self.restore_screen.log.search.reset();
                            self.text_histories.restore_search_game_name.push("");
                        }
                        Screen::CustomGames => {
                            self.custom_games_screen.filter.reset();
                            self.text_histories.custom_games_search_game_name.push("");
                        }
                        Screen::Other | Screen::Games | Screen::GameDetail(_) | Screen::ThisDevice | Screen::AllDevices => {}
                    },
                    game_filter::Event::EditedFilterUniqueness(value) => match self.screen {
                        Screen::Backup => {
                            self.backup_screen.log.search.uniqueness.choice = value;
                        }
                        Screen::Restore => {
                            self.restore_screen.log.search.uniqueness.choice = value;
                        }
                        Screen::CustomGames => {}
                        Screen::Other | Screen::Games | Screen::GameDetail(_) | Screen::ThisDevice | Screen::AllDevices => {}
                    },
                    game_filter::Event::EditedFilterCompleteness(value) => match self.screen {
                        Screen::Backup => {
                            self.backup_screen.log.search.completeness.choice = value;
                        }
                        Screen::Restore => {
                            self.restore_screen.log.search.completeness.choice = value;
                        }
                        Screen::CustomGames => {}
                        Screen::Other | Screen::Games | Screen::GameDetail(_) | Screen::ThisDevice | Screen::AllDevices => {}
                    },
                    game_filter::Event::EditedFilterEnablement(value) => match self.screen {
                        Screen::Backup => {
                            self.backup_screen.log.search.enablement.choice = value;
                        }
                        Screen::Restore => {
                            self.restore_screen.log.search.enablement.choice = value;
                        }
                        Screen::CustomGames => {}
                        Screen::Other | Screen::Games | Screen::GameDetail(_) | Screen::ThisDevice | Screen::AllDevices => {}
                    },
                    game_filter::Event::EditedFilterChange(value) => match self.screen {
                        Screen::Backup => {
                            self.backup_screen.log.search.change.choice = value;
                        }
                        Screen::Restore => {
                            self.restore_screen.log.search.change.choice = value;
                        }
                        Screen::CustomGames => {}
                        Screen::Other | Screen::Games | Screen::GameDetail(_) | Screen::ThisDevice | Screen::AllDevices => {}
                    },
                    game_filter::Event::EditedFilterManifest(value) => match self.screen {
                        Screen::Backup => {
                            self.backup_screen.log.search.manifest.choice = value;
                        }
                        Screen::Restore => {
                            self.restore_screen.log.search.manifest.choice = value;
                        }
                        Screen::CustomGames => {}
                        Screen::Other | Screen::Games | Screen::GameDetail(_) | Screen::ThisDevice | Screen::AllDevices => {}
                    },
                }

                task.unwrap_or_else(Task::none)
            }
            Message::BrowseDir(subject) => Task::future(async move {
                let choice = async move { rfd::AsyncFileDialog::new().pick_folder().await }.await;

                Message::browsed_dir(subject, choice.map(|x| x.path().to_path_buf()))
            }),
            Message::BrowseFile(subject) => Task::future(async move {
                let choice = async move { rfd::AsyncFileDialog::new().pick_file().await }.await;

                Message::browsed_file(subject, choice.map(|x| x.path().to_path_buf()))
            }),
            Message::SelectedFile(subject, path) => {
                match subject {
                    BrowseFileSubject::RcloneExecutable => {
                        self.text_histories.rclone_executable.push(path.raw());
                        self.config.apps.rclone.path = path;
                    }
                    BrowseFileSubject::RootLutrisDatabase(i) => {
                        self.text_histories.roots[i].lutris_database.push(path.raw());
                        if let Root::Lutris(root) = &mut self.config.roots[i] {
                            root.database = Some(path);
                        }
                    }
                    BrowseFileSubject::SecondaryManifest(i) => {
                        self.text_histories.secondary_manifests[i].push(path.raw());
                        self.config.manifest.secondary[i].set(path.raw().into());
                    }
                }
                self.save_config();
                Task::none()
            }
            Message::SelectAllGames => {
                match self.screen {
                    Screen::Backup => {
                        for name in self.backup_screen.log.visible_games(
                            ScanKind::Backup,
                            &self.config,
                            &self.manifest.extended,
                            &self.backup_screen.duplicate_detector,
                        ) {
                            self.config.enable_game_for_backup(&name);
                        }
                    }
                    Screen::Restore => {
                        for name in self.restore_screen.log.visible_games(
                            ScanKind::Restore,
                            &self.config,
                            &self.manifest.extended,
                            &self.restore_screen.duplicate_detector,
                        ) {
                            self.config.enable_game_for_restore(&name);
                        }
                    }
                    Screen::CustomGames => {
                        for i in self.custom_games_screen.visible_games(&self.config) {
                            self.config.enable_custom_game(i);
                        }
                    }
                    _ => {}
                }
                self.save_config();
                Task::none()
            }
            Message::DeselectAllGames => {
                match self.screen {
                    Screen::Backup => {
                        for name in self.backup_screen.log.visible_games(
                            ScanKind::Backup,
                            &self.config,
                            &self.manifest.extended,
                            &self.backup_screen.duplicate_detector,
                        ) {
                            self.config.disable_game_for_backup(&name);
                        }
                    }
                    Screen::Restore => {
                        for name in self.restore_screen.log.visible_games(
                            ScanKind::Restore,
                            &self.config,
                            &self.manifest.extended,
                            &self.restore_screen.duplicate_detector,
                        ) {
                            self.config.disable_game_for_restore(&name);
                        }
                    }
                    Screen::CustomGames => {
                        for i in self.custom_games_screen.visible_games(&self.config) {
                            self.config.disable_custom_game(i);
                        }
                    }
                    _ => {}
                }
                self.save_config();
                Task::none()
            }
            Message::OpenDir { path } => {
                let path2 = path.clone();
                Task::future(async move {
                    let result = async { opener::open(path.resolve()) }.await;

                    match result {
                        Ok(_) => Message::Ignore,
                        Err(e) => {
                            log::error!("Unable to open directory: `{}` - {:?}", path2.resolve(), e);
                            Message::OpenDirFailure { path: path2 }
                        }
                    }
                })
            }
            Message::OpenDirSubject(subject) => {
                let path = match subject {
                    BrowseSubject::BackupTarget => self.config.backup.path.clone(),
                    BrowseSubject::RestoreSource => self.config.restore.path.clone(),
                    BrowseSubject::Root(i) => self.config.roots[i].path().clone(),
                    BrowseSubject::RedirectSource(i) => self.config.redirects[i].source.clone(),
                    BrowseSubject::RedirectTarget(i) => self.config.redirects[i].target.clone(),
                    BrowseSubject::CustomGameFile(i, j) => {
                        StrictPath::new(self.config.custom_games[i].files[j].clone())
                    }
                    BrowseSubject::BackupFilterIgnoredPath(i) => self.config.backup.filter.ignored_paths[i].clone(),
                };

                match path.parent_if_file() {
                    Ok(path) => self.update(Message::OpenDir { path }),
                    Err(_) => self.show_error(Error::UnableToOpenDir(path)),
                }
            }
            Message::OpenFileSubject(subject) => {
                let path = match subject {
                    BrowseFileSubject::RcloneExecutable => self.config.apps.rclone.path.clone(),
                    BrowseFileSubject::RootLutrisDatabase(i) => {
                        let Root::Lutris(root) = &self.config.roots[i] else {
                            return Task::none();
                        };
                        let Some(database) = root.database.as_ref() else {
                            return Task::none();
                        };
                        database.clone()
                    }
                    BrowseFileSubject::SecondaryManifest(i) => {
                        let Some(path) = self.config.manifest.secondary[i].path() else {
                            return Task::none();
                        };
                        path.clone()
                    }
                };

                match path.parent_if_file() {
                    Ok(path) => self.update(Message::OpenDir { path }),
                    Err(_) => self.show_error(Error::UnableToOpenDir(path)),
                }
            }
            Message::OpenDirFailure { path } => self.show_modal(Modal::Error {
                variant: Error::UnableToOpenDir(path),
            }),
            Message::OpenUrlFailure { url } => self.show_modal(Modal::Error {
                variant: Error::UnableToOpenUrl(url),
            }),
            Message::KeyboardEvent(event) => {
                if let iced::keyboard::Event::ModifiersChanged(modifiers) = event {
                    self.modifiers = modifiers;
                }
                match event {
                    iced::keyboard::Event::KeyPressed {
                        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab),
                        modifiers,
                        ..
                    } => {
                        if modifiers.shift() {
                            iced::widget::operation::focus_previous()
                        } else {
                            iced::widget::operation::focus_next()
                        }
                    }
                    _ => Task::none(),
                }
            }
            Message::UndoRedo(action, subject) => {
                let shortcut = Shortcut::from(action);
                match subject {
                    UndoSubject::BackupTarget => shortcut.apply_to_strict_path_field(
                        &mut self.config.backup.path,
                        &mut self.text_histories.backup_target,
                    ),
                    UndoSubject::RestoreSource => shortcut.apply_to_strict_path_field(
                        &mut self.config.restore.path,
                        &mut self.text_histories.restore_source,
                    ),
                    UndoSubject::BackupSearchGameName => shortcut.apply_to_string_field(
                        &mut self.backup_screen.log.search.game_name,
                        &mut self.text_histories.backup_search_game_name,
                    ),
                    UndoSubject::RestoreSearchGameName => shortcut.apply_to_string_field(
                        &mut self.restore_screen.log.search.game_name,
                        &mut self.text_histories.restore_search_game_name,
                    ),
                    UndoSubject::CustomGamesSearchGameName => shortcut.apply_to_string_field(
                        &mut self.custom_games_screen.filter.name,
                        &mut self.text_histories.custom_games_search_game_name,
                    ),
                    UndoSubject::RootPath(i) => shortcut.apply_to_strict_path_field(
                        self.config.roots[i].path_mut(),
                        &mut self.text_histories.roots[i].path,
                    ),
                    UndoSubject::RootLutrisDatabase(i) => {
                        if let Root::Lutris(root) = &mut self.config.roots[i] {
                            shortcut.apply_to_option_strict_path_field(
                                &mut root.database,
                                &mut self.text_histories.roots[i].lutris_database,
                            )
                        }
                    }
                    UndoSubject::SecondaryManifest(i) => {
                        let history = &mut self.text_histories.secondary_manifests[i];
                        match shortcut {
                            Shortcut::Undo => {
                                self.config.manifest.secondary[i].set(history.undo());
                            }
                            Shortcut::Redo => {
                                self.config.manifest.secondary[i].set(history.redo());
                            }
                        }
                    }
                    UndoSubject::RedirectSource(i) => shortcut.apply_to_strict_path_field(
                        &mut self.config.redirects[i].source,
                        &mut self.text_histories.redirects[i].source,
                    ),
                    UndoSubject::RedirectTarget(i) => shortcut.apply_to_strict_path_field(
                        &mut self.config.redirects[i].target,
                        &mut self.text_histories.redirects[i].target,
                    ),
                    UndoSubject::CustomGameName(i) => shortcut.apply_to_string_field(
                        &mut self.config.custom_games[i].name,
                        &mut self.text_histories.custom_games[i].name,
                    ),
                    UndoSubject::CustomGameAlias(i) => {
                        if let Some(alias) = self.config.custom_games[i].alias.as_mut() {
                            shortcut.apply_to_string_field(alias, &mut self.text_histories.custom_games[i].alias)
                        }
                    }
                    UndoSubject::CustomGameFile(i, j) => shortcut.apply_to_string_field(
                        &mut self.config.custom_games[i].files[j],
                        &mut self.text_histories.custom_games[i].files[j],
                    ),
                    UndoSubject::CustomGameRegistry(i, j) => shortcut.apply_to_string_field(
                        &mut self.config.custom_games[i].registry[j],
                        &mut self.text_histories.custom_games[i].registry[j],
                    ),
                    UndoSubject::CustomGameInstallDir(i, j) => shortcut.apply_to_string_field(
                        &mut self.config.custom_games[i].install_dir[j],
                        &mut self.text_histories.custom_games[i].install_dir[j],
                    ),
                    UndoSubject::CustomGameWinePrefix(i, j) => shortcut.apply_to_string_field(
                        &mut self.config.custom_games[i].wine_prefix[j],
                        &mut self.text_histories.custom_games[i].wine_prefix[j],
                    ),
                    UndoSubject::BackupFilterIgnoredPath(i) => shortcut.apply_to_strict_path_field(
                        &mut self.config.backup.filter.ignored_paths[i],
                        &mut self.text_histories.backup_filter_ignored_paths[i],
                    ),
                    UndoSubject::BackupFilterIgnoredRegistry(i) => shortcut.apply_to_registry_path_field(
                        &mut self.config.backup.filter.ignored_registry[i],
                        &mut self.text_histories.backup_filter_ignored_registry[i],
                    ),
                    UndoSubject::RcloneExecutable => shortcut.apply_to_strict_path_field(
                        &mut self.config.apps.rclone.path,
                        &mut self.text_histories.rclone_executable,
                    ),
                    UndoSubject::RcloneArguments => shortcut.apply_to_string_field(
                        &mut self.config.apps.rclone.arguments,
                        &mut self.text_histories.rclone_arguments,
                    ),
                    UndoSubject::CloudRemoteId => {
                        if let Some(Remote::Custom { id }) = &mut self.config.cloud.remote {
                            shortcut.apply_to_string_field(id, &mut self.text_histories.cloud_remote_id)
                        }
                    }
                    UndoSubject::CloudPath => {
                        shortcut.apply_to_string_field(&mut self.config.cloud.path, &mut self.text_histories.cloud_path)
                    }
                    UndoSubject::ModalField(field) => {
                        match field {
                            ModalInputKind::Url => self.text_histories.modal.url.apply(shortcut),
                            ModalInputKind::Host => self.text_histories.modal.host.apply(shortcut),
                            ModalInputKind::Port => self.text_histories.modal.port.apply(shortcut),
                            ModalInputKind::Username => self.text_histories.modal.username.apply(shortcut),
                            ModalInputKind::Password => self.text_histories.modal.password.apply(shortcut),
                        }
                        return Task::none();
                    }
                    UndoSubject::BackupComment(game) => {
                        if let Some(info) = self.text_histories.backup_comments.get_mut(&game) {
                            let comment = match shortcut {
                                Shortcut::Undo => info.undo(),
                                Shortcut::Redo => info.redo(),
                            };

                            let updated = self.restore_screen.log.set_comment(&game, comment);
                            if updated {
                                self.save_backup(&game);
                            }
                        }
                    }
                }
                self.save_config();
                Task::none()
            }
            Message::SelectedBackupToRestore { game, backup } => {
                self.backups_to_restore.insert(game.clone(), backup.id());
                self.handle_restore(RestorePhase::Start {
                    preview: true,
                    games: Some(GameSelection::single(game)),
                })
            }
            Message::GameAction { action, game } => match action {
                GameAction::PreviewBackup => self.handle_backup(BackupPhase::Start {
                    preview: true,
                    repair: false,
                    jump: false,
                    games: Some(GameSelection::single(game)),
                }),
                GameAction::Backup { confirm } => {
                    if confirm {
                        self.handle_backup(BackupPhase::Confirm {
                            games: Some(GameSelection::single(game)),
                        })
                    } else {
                        self.handle_backup(BackupPhase::Start {
                            preview: false,
                            repair: false,
                            jump: false,
                            games: Some(GameSelection::single(game)),
                        })
                    }
                }
                GameAction::PreviewRestore => self.handle_restore(RestorePhase::Start {
                    preview: true,
                    games: Some(GameSelection::single(game)),
                }),
                GameAction::Restore { confirm } => {
                    if confirm {
                        self.handle_restore(RestorePhase::Confirm {
                            games: Some(GameSelection::single(game)),
                        })
                    } else {
                        self.handle_restore(RestorePhase::Start {
                            preview: false,
                            games: Some(GameSelection::single(game)),
                        })
                    }
                }
                GameAction::Customize => self.customize_game(game),
                GameAction::Wiki => Self::open_wiki(game),
                GameAction::Comment => self.toggle_backup_comment_editor(game),
                GameAction::Lock | GameAction::Unlock => {
                    let updated = self.restore_screen.log.toggle_locked(&game);
                    if updated {
                        self.save_backup(&game);
                    }
                    Task::none()
                }
                GameAction::MakeAlias => self.customize_game_as_alias(game),
            },
            Message::Scrolled { subject, position } => {
                self.scroll_offsets.insert(subject, position);
                Task::none()
            }
            Message::Scroll { subject, position } => {
                self.scroll_offsets.insert(subject, position);
                iced::widget::operation::scroll_to(subject.id(), position)
            }
            Message::EditedBackupComment { game, action } => {
                if let Some(comment) = self.restore_screen.log.apply_comment_action(&game, action) {
                    self.save_backup(&game);
                    if let Some(info) = self.text_histories.backup_comments.get_mut(&game) {
                        info.push(&comment);
                    }
                }

                Task::none()
            }
            Message::FilterDuplicates { scan_kind, game } => {
                let log = match scan_kind {
                    ScanKind::Backup => &mut self.backup_screen.log,
                    ScanKind::Restore => &mut self.restore_screen.log,
                };
                log.filter_duplicates_of = game;
                Task::none()
            }
            Message::OpenUrl(url) => Self::open_url(url),
            Message::OpenUrlAndCloseModal(url) => Task::batch([Self::open_url(url), self.close_modal()]),
            Message::GameDetailFilesToggled => {
                self.game_detail_files_expanded = !self.game_detail_files_expanded;
                if self.game_detail_files_expanded {
                    if let Screen::GameDetail(ref game_name) = self.screen.clone() {
                        self.backup_screen.log.expand_game(
                            game_name,
                            &self.backup_screen.duplicate_detector,
                            &self.config,
                            ScanKind::Backup,
                        );
                    }
                }
                Task::none()
            }
            Message::EditedCloudRemote(choice) => {
                if let Ok(remote) = Remote::try_from(choice) {
                    match &remote {
                        Remote::Custom { id } => {
                            self.text_histories.cloud_remote_id.push(id);
                            self.config.cloud.remote = Some(remote);
                            self.save_config();
                            Task::none()
                        }
                        Remote::Ftp {
                            id: _,
                            host,
                            port,
                            username,
                            password,
                        } => {
                            self.text_histories.modal.host.initialize(host.clone());
                            self.text_histories.modal.port.initialize(port.to_string());
                            self.text_histories.modal.username.initialize(username.clone());
                            self.text_histories.modal.password.initialize(password.clone());

                            self.show_modal(Modal::ConfigureFtpRemote)
                        }
                        Remote::Smb {
                            id: _,
                            host,
                            port,
                            username,
                            password,
                        } => {
                            self.text_histories.modal.host.initialize(host.clone());
                            self.text_histories.modal.port.initialize(port.to_string());
                            self.text_histories.modal.username.initialize(username.clone());
                            self.text_histories.modal.password.initialize(password.clone());

                            self.show_modal(Modal::ConfigureSmbRemote)
                        }
                        Remote::WebDav {
                            id: _,
                            url,
                            username,
                            password,
                            provider,
                        } => {
                            self.text_histories.modal.url.initialize(url.clone());
                            self.text_histories.modal.username.initialize(username.clone());
                            self.text_histories.modal.password.initialize(password.clone());

                            self.show_modal(Modal::ConfigureWebDavRemote { provider: *provider })
                        }
                        Remote::Box { .. }
                        | Remote::Dropbox { .. }
                        | Remote::GoogleDrive { .. }
                        | Remote::OneDrive { .. } => self.configure_remote(remote),
                    }
                } else {
                    self.config.cloud.remote = None;
                    self.save_config();
                    Task::none()
                }
            }
            Message::ConfigureCloudSuccess(remote) => {
                self.text_histories.clear_modal_fields();

                self.config.cloud.remote = Some(remote);
                self.save_config();
                self.close_modal()
            }
            Message::ConfigureCloudFailure(error) => {
                self.text_histories.clear_modal_fields();

                self.config.cloud.remote = None;
                self.save_config();
                self.show_error(Error::UnableToConfigureCloud(error))
            }
            Message::ConfirmSynchronizeCloud { direction } => {
                let local = self.config.backup.path.clone();

                self.show_modal(Modal::ConfirmCloudSync {
                    local: local.render(),
                    cloud: self.config.cloud.path.clone(),
                    direction,
                    changes: vec![],
                    page: 0,
                    state: CloudModalState::Initial,
                })
            }
            Message::SynchronizeCloud { direction, finality } => {
                let local = self.config.backup.path.clone();

                if let Err(e) = self.start_sync_cloud(&local, direction, finality, None, true) {
                    return self.show_error(e);
                }

                self.show_modal(Modal::ConfirmCloudSync {
                    local: local.render(),
                    cloud: self.config.cloud.path.clone(),
                    direction,
                    changes: vec![],
                    page: 0,
                    state: match finality {
                        Finality::Preview => CloudModalState::Previewing,
                        Finality::Final => CloudModalState::Syncing,
                    },
                })
            }
            Message::RcloneMonitor(event) => {
                match event {
                    rclone_monitor::Event::Ready(sender) => {
                        self.rclone_monitor_sender = Some(sender);
                    }
                    rclone_monitor::Event::Data(events) => {
                        for event in events {
                            match event {
                                crate::cloud::RcloneProcessEvent::Progress { current, max } => {
                                    self.progress.set(current, max);
                                }
                                crate::cloud::RcloneProcessEvent::Change(change) => {
                                    self.operation.add_cloud_change();
                                    if let Some(modal) = self.modals.last_mut() {
                                        modal.add_cloud_change(change);
                                    }
                                }
                            }
                        }
                    }
                    rclone_monitor::Event::Succeeded => {
                        if let Some(cmd) = self.transition_from_cloud_step() {
                            return cmd;
                        }

                        if let Some(modal) = self.modals.last_mut() {
                            self.operation = Operation::Idle;
                            self.progress.reset();
                            modal.finish_cloud_scan();
                        } else {
                            self.go_idle();
                        }
                    }
                    rclone_monitor::Event::Failed(e) => {
                        self.operation.push_error(Error::UnableToSynchronizeCloud(e.clone()));
                        if let Some(cmd) = self.transition_from_cloud_step() {
                            return cmd;
                        }

                        self.go_idle();
                        return Task::batch([
                            self.close_specific_modal(modal::Kind::ConfirmCloudSync),
                            self.show_error(Error::UnableToSynchronizeCloud(e)),
                        ]);
                    }
                    rclone_monitor::Event::Cancelled => {
                        self.go_idle();
                        return self.close_specific_modal(modal::Kind::ConfirmCloudSync);
                    }
                }
                Task::none()
            }
            Message::EditedModalField(field) => {
                match field {
                    ModalField::Url(new) => {
                        self.text_histories.modal.url.push(&new);
                    }
                    ModalField::Host(new) => {
                        self.text_histories.modal.host.push(&new);
                    }
                    ModalField::Port(new) => {
                        self.text_histories.modal.port.push(&new);
                    }
                    ModalField::Username(new) => {
                        self.text_histories.modal.username.push(&new);
                    }
                    ModalField::Password(new) => {
                        self.text_histories.modal.password.push(&new);
                    }
                    ModalField::WebDavProvider(new) => {
                        if let Some(Modal::ConfigureWebDavRemote { provider }) = self.modals.last_mut() {
                            *provider = new;
                        }
                    }
                }
                Task::none()
            }
            Message::FinalizeRemote(remote) => self.configure_remote(remote),
            Message::ModalChangePage(page) => {
                if let Some(modal) = self.modals.last_mut() {
                    modal.set_page(page);
                }
                Task::none()
            }
            Message::ShowCustomGame { name } => {
                use crate::gui::widget::operation::container_scroll_offset;

                let subject = ScrollSubject::CustomGames;

                self.scroll_offsets.remove(&subject);
                self.screen = Screen::CustomGames;

                container_scroll_offset(name.clone().into()).map(move |offset| match offset {
                    Some(position) => Message::Scroll { subject, position },
                    None => Message::Ignore,
                })
            }
            Message::ShowScanActiveGames => self.show_modal(Modal::ActiveScanGames),
            Message::CopyText(text) => iced::clipboard::write(text),
            #[cfg_attr(not(windows), allow(unused))]
            Message::OpenRegistry(item) => {
                #[cfg(windows)]
                {
                    use windows::{
                        core::s,
                        Win32::UI::{
                            Shell::{ShellExecuteExA, SHELLEXECUTEINFOA},
                            WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL},
                        },
                    };

                    let mut system = sysinfo::System::new_all();
                    system.refresh_all();
                    if system.processes_by_exact_name("regedit.exe".as_ref()).next().is_some() {
                        let mut info = SHELLEXECUTEINFOA {
                            cbSize: size_of::<SHELLEXECUTEINFOA>() as u32,
                            lpVerb: s!("runas"),
                            lpFile: s!("taskkill.exe"),
                            lpParameters: s!("/im regedit.exe"),
                            nShow: SW_HIDE.0,
                            ..Default::default()
                        };
                        unsafe {
                            if let Err(e) = ShellExecuteExA(&mut info) {
                                log::error!("Failed to close Regedit: {e:?}");
                                return Task::none();
                            }
                        }

                        // When already running as admin (i.e., no UAC prompts),
                        // this is needed or else Regedit won't reopen.
                        // Maybe `taskkill` returns while the process is still shutting down?
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }

                    let hive = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
                    let Ok(key) = hive.create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Applets\Regedit")
                    else {
                        return Task::none();
                    };
                    if let Err(e) = key.0.set_value("LastKey", &format!("Computer\\{}", item.interpret())) {
                        log::error!("Failed to edit Regedit last key: {e:?}");
                        return Task::none();
                    }

                    let mut info = SHELLEXECUTEINFOA {
                        cbSize: size_of::<SHELLEXECUTEINFOA>() as u32,
                        lpVerb: s!("runas"),
                        lpFile: s!("regedit.exe"),
                        nShow: SW_SHOWNORMAL.0,
                        ..Default::default()
                    };
                    unsafe {
                        if let Err(e) = ShellExecuteExA(&mut info) {
                            log::error!("Failed to open Regedit: {e:?}");
                            return Task::none();
                        }
                    }
                }
                Task::none()
            }
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subscriptions = vec![
            iced::event::listen_with(|event, _status, _window| match event {
                iced::Event::Keyboard(event) => Some(Message::KeyboardEvent(event)),
                iced::Event::Window(iced::window::Event::CloseRequested) => Some(Message::Exit { user: true }),
                _ => None,
            }),
            rclone_monitor::run().map(Message::RcloneMonitor),
        ];

        if self.timed_notification.is_some() {
            subscriptions.push(iced::time::every(Duration::from_millis(250)).map(|_| Message::PruneNotifications));
        }

        if self.progress.visible() {
            subscriptions.push(iced::time::every(Duration::from_millis(100)).map(|_| Message::UpdateTime));
        }

        if !self.pending_save.is_empty() {
            subscriptions.push(iced::time::every(Duration::from_millis(200)).map(|_| Message::Save));
        }

        if self.flags.update_manifest {
            subscriptions.push(
                iced::time::every(Duration::from_secs(60 * 60 * 24)).map(|_| Message::UpdateManifest { force: false }),
            );
        }

        if self.config.release.check {
            subscriptions.push(iced::time::every(Duration::from_secs(60 * 60 * 24)).map(|_| Message::CheckAppRelease));
        }

        if self.exiting {
            subscriptions.push(iced::time::every(Duration::from_millis(50)).map(|_| Message::Exit { user: false }));
        }

        subscriptions.push(iced::time::every(Duration::from_secs(5)).map(|_| {
            let mut system = sysinfo::System::new();
            system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
            #[cfg(target_os = "windows")]
            let daemon_name = "ludusavi-daemon.exe";
            #[cfg(not(target_os = "windows"))]
            let daemon_name = "ludusavi-daemon";
            let running = system.processes_by_exact_name(daemon_name.as_ref()).next().is_some();

            let sync_status = {
                let path = crate::prelude::app_dir().joined("daemon-status.json");
                if let Some(content) = path.read() {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                        json.get("games")
                            .and_then(|g| g.as_object())
                            .map(|obj| {
                                obj.iter()
                                    .map(|(k, v)| {
                                        let status = v.get("status")
                                            .and_then(|s| s.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        (k.clone(), status)
                                    })
                                    .collect::<std::collections::HashMap<String, String>>()
                            })
                            .unwrap_or_default()
                    } else {
                        std::collections::HashMap::new()
                    }
                } else {
                    std::collections::HashMap::new()
                }
            };

            let game_list = {
                let path = crate::prelude::app_dir().joined("ludusavi-game-list.json");
                if let Some(content) = path.read() {
                    serde_json::from_str::<ludusavi::sync::game_list::GameListFile>(&content)
                        .unwrap_or_default()
                } else {
                    ludusavi::sync::game_list::GameListFile::default()
                }
            };
            Message::DaemonStatusChecked(running, sync_status, game_list)
        }));

        iced::Subscription::batch(subscriptions)
    }

    pub fn view(&self) -> Element {
        // --- SIDEBAR ---
        let sidebar = {
            // Logo
            let logo = Container::new(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(
                        Container::new(
                            crate::gui::widget::text("💾").size(16)
                        )
                        .width(28)
                        .height(28)
                        .center_x(28)
                        .center_y(28)
                        .class(style::Container::Badge),
                    )
                    .push(
                        Column::new()
                            .push(crate::gui::widget::text("Ludusavi").size(15))
                            .push(crate::gui::widget::text("SAVE SYNC").size(10).class(style::Text::Muted)),
                    ),
            )
            .width(Length::Fill)
            .padding([16, 14]);

            // Nav items
            let nav_item = |label: &'static str, screen: Screen| -> Element {
                let active = self.screen == screen;
                crate::gui::widget::Button::new(
                    crate::gui::widget::text(label).size(13),
                )
                .on_press(Message::SwitchScreen(screen))
                .width(Length::Fill)
                .padding([8, 10])
                .class(if active {
                    style::Button::SidebarItemActive
                } else {
                    style::Button::SidebarItem
                })
                .into()
            };

            let nav = Column::new()
                .padding([8, 8])
                .spacing(2)
                .push(nav_item("🎮  Games", Screen::Games))
                .push(nav_item("🖥  This device", Screen::ThisDevice))
                .push(nav_item("📡  All devices", Screen::AllDevices))
                .push(
                    Container::new(
                        crate::gui::widget::text("ADVANCED")
                            .size(10)
                            .class(style::Text::Muted)
                            .width(Length::Fill),
                    )
                    .padding(iced::padding::top(20).bottom(6).left(10).right(10)),
                )
                .push(nav_item("📦  Backup", Screen::Backup))
                .push(nav_item("↩  Restore", Screen::Restore))
                .push(nav_item("🎮  Custom games", Screen::CustomGames))
                .push(nav_item("⚙  Settings", Screen::Other));

            // Daemon status pill
            let daemon_pill = Container::new(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(
                        Container::new(crate::gui::widget::Space::new())
                            .width(7)
                            .height(7)
                            .class(if self.daemon_running {
                                style::Container::DaemonDotActive
                            } else {
                                style::Container::DaemonDotInactive
                            }),
                    )
                    .push(
                        crate::gui::widget::text(if self.daemon_running {
                            "Sync daemon running"
                        } else {
                            "Sync daemon stopped"
                        })
                        .size(12)
                        .class(if self.daemon_running {
                            style::Text::Green
                        } else {
                            style::Text::Muted
                        }),
                    ),
            )
            .width(Length::Fill)
            .padding([10, 12])
            .class(if self.daemon_running {
                style::Container::DaemonStatus
            } else {
                style::Container::GameListEntry
            });

            Container::new(
                Column::new()
                    .height(Length::Fill)
                    .push(logo)
                    .push(nav.height(Length::Fill))
                    .push(Container::new(daemon_pill).padding([8, 8]).width(Length::Fill)),
            )
            .width(200)
            .height(Length::Fill)
            .class(style::Container::Sidebar)
        };

        // --- MAIN CONTENT ---
        let main_content = match self.screen {
            Screen::Backup => self.backup_screen.view(
                &self.config,
                &self.manifest.extended,
                &self.operation,
                &self.text_histories,
                &self.modifiers,
                self.daemon_running,
                &self.sync_status,
            ),
            Screen::Restore => self.restore_screen.view(
                &self.config,
                &self.manifest.extended,
                &self.operation,
                &self.text_histories,
                &self.modifiers,
                &self.sync_status,
                self.daemon_running,
            ),
            Screen::CustomGames => self.custom_games_screen.view(
                &self.config,
                &self.manifest.extended,
                !self.operation.idle(),
                &self.text_histories,
                &self.modifiers,
            ),
            Screen::Games => {
                let entries: Vec<_> = self.backup_screen.log.entries.iter().collect();
                let game_list = &self.game_list;
                let sync_status = &self.sync_status;
                let sync_config = &self.sync_games_config;

            let header = Row::new()
                    .padding([0, 24])
                    .height(52)
                    .align_y(Alignment::Center)
                    .push(crate::gui::widget::text("Games").size(15).width(Length::Fill))
                    .push(
                        crate::gui::widget::Button::new(crate::gui::widget::text("🔍 Search").size(13))
                            .padding([7, 14])
                            .class(style::Button::Ghost)
                            .on_press(Message::Filter { event: crate::scan::game_filter::Event::Toggled }),
                    )
                    .push(crate::gui::widget::Space::new().width(8))
                    .push(
                        crate::gui::widget::Button::new(crate::gui::widget::text("Scan now").size(13))
                            .padding([7, 14])
                            .class(style::Button::Ghost)
                            .on_press(Message::Backup(BackupPhase::Start {
                                preview: true,
                                repair: false,
                                jump: false,
                                games: None,
                            })),
                    )
                    .push(crate::gui::widget::Space::new().width(8))
                    .push(
                        crate::gui::widget::Button::new(crate::gui::widget::text("+ Add game").size(13))
                            .padding([7, 14])
                            .class(style::Button::Primary)
                            .on_press(Message::SwitchScreen(Screen::Backup)),
                    );
                
                let search_row = crate::gui::widget::TextInput::new(
                    "Search games...",
                    &self.games_search,
                )
                .on_input(Message::GamesSearchChanged)
                .padding([8, 12])
                .size(13);
                
                let table_header = Row::new()
                    .padding([8, 16])
                    .push(crate::gui::widget::text("").width(20))
                    .push(crate::gui::widget::text("NAME").size(11).class(style::Text::Muted).width(Length::Fill))
                    .push(crate::gui::widget::text("MODE").size(11).class(style::Text::Muted).width(80))
                    .push(crate::gui::widget::text("AUTO SYNC").size(11).class(style::Text::Muted).width(100))
                    .push(crate::gui::widget::text("LAST SYNCED FROM").size(11).class(style::Text::Muted).width(160))
                    .push(crate::gui::widget::text("LAST SYNCED").size(11).class(style::Text::Muted).width(140))
                    .push(crate::gui::widget::text("").width(50));

                let mut rows = Column::new().width(Length::Fill);

                // Juegos disponibles en el cloud sin SYNC local
                let cloud_available: Vec<&ludusavi::sync::game_list::GameMetaData> = self.game_list.games
                    .iter()
                    .filter(|g| {
                        let local_mode = self.sync_games_config.get_mode(&g.id);
                        !matches!(local_mode, ludusavi::sync::sync_config::SaveMode::Sync)
                        && !entries.iter().any(|e| e.scan_info.game_name == g.id)
                    })
                    .collect();
                
                if entries.is_empty() && cloud_available.is_empty() {
                    rows = rows.push(
                        Container::new(
                            crate::gui::widget::text("No games found. Run a backup scan first.")
                                .size(13)
                                .class(style::Text::Muted),
                        )
                        .width(Length::Fill)
                        .padding([24, 16]),
                    );
                } else {
                    let search_lower = self.games_search.to_lowercase();
                    let mut peekable_entries = entries.iter().peekable();
                    while let Some(entry) = peekable_entries.next() {
                        let is_last = peekable_entries.peek().is_none();
                        let name = &entry.scan_info.game_name;
                        if !search_lower.is_empty() && !name.to_lowercase().contains(&search_lower) {
                            continue;
                        }
                        let mode = sync_config.get_mode(name);
                        let status = sync_status.get(name).map(|s| s.as_str()).unwrap_or("");
                        let meta = game_list.get_game(name);

                        // Status dot color
                        let dot_class = match mode {
                            ludusavi::sync::sync_config::SaveMode::None => style::Container::DaemonDotInactive,
                            ludusavi::sync::sync_config::SaveMode::Local |
                            ludusavi::sync::sync_config::SaveMode::Cloud => style::Container::DaemonDotInactive,
                            ludusavi::sync::sync_config::SaveMode::Sync => {
                                if status == "synced" {
                                    style::Container::DaemonDotActive
                                } else {
                                    style::Container::DaemonDotPending
                                }
                            }
                        };

                        // Mode badge text
                         let mode_text = match mode {
                            ludusavi::sync::sync_config::SaveMode::None => "—",
                            ludusavi::sync::sync_config::SaveMode::Local => "LOCAL",
                            ludusavi::sync::sync_config::SaveMode::Cloud => "CLOUD",
                            ludusavi::sync::sync_config::SaveMode::Sync => "SYNC",
                        };

                        // Last synced from — solo aplica a CLOUD y SYNC
                        let last_from = match mode {
                            ludusavi::sync::sync_config::SaveMode::Cloud |
                            ludusavi::sync::sync_config::SaveMode::Sync => {
                                meta.and_then(|m| m.last_synced_from.as_deref())
                                    .map(|id| game_list.get_device_name(id).to_string())
                                    .unwrap_or_else(|| "—".to_string())
                            }
                            _ => "—".to_string(),
                        };

                        // Last synced time — solo aplica a CLOUD y SYNC
                        let last_synced = match mode {
                            ludusavi::sync::sync_config::SaveMode::Cloud |
                            ludusavi::sync::sync_config::SaveMode::Sync => {
                                meta
                                    .and_then(|m| m.last_sync_time_utc)
                                    .map(|t| {
                                        let now = chrono::Utc::now();
                                        let diff = now.signed_duration_since(t);
                                        if diff.num_minutes() < 1 {
                                            "just now".to_string()
                                        } else if diff.num_hours() < 1 {
                                            format!("{} min ago", diff.num_minutes())
                                        } else if diff.num_hours() < 24 {
                                            format!("{} hours ago", diff.num_hours())
                                        } else {
                                            format!("{} days ago", diff.num_days())
                                        }
                                    })
                                    .unwrap_or_else(|| "Never".to_string())
                            }
                            _ => "—".to_string(),
                        };

                        let row = Container::new(
                            Row::new()
                                .padding([12, 16])
                                .align_y(Alignment::Center)
                                .push(
                                    Container::new(crate::gui::widget::Space::new())
                                        .width(10)
                                        .height(10)
                                        .class(dot_class),
                                )
                                .push(crate::gui::widget::Space::new().width(16))
                                .push(
                                    crate::gui::widget::text(name.clone())
                                        .size(13)
                                        .width(Length::Fill),
                                )
                                .push(
                                    crate::gui::widget::text(mode_text)
                                        .size(11)
                                        .class(style::Text::Muted)
                                        .width(80),
                                )
                                .push(
                                    crate::gui::widget::text(
                                        match mode {
                                            ludusavi::sync::sync_config::SaveMode::None => "—",
                                            ludusavi::sync::sync_config::SaveMode::Sync => "On",
                                            ludusavi::sync::sync_config::SaveMode::Local |
                                            ludusavi::sync::sync_config::SaveMode::Cloud => {
                                                if self.sync_games_config.get_auto_sync(name) {
                                                    "On"
                                                } else {
                                                    "Off"
                                                }
                                            }
                                        }
                                    )
                                    .size(12)
                                    .class(style::Text::Muted)
                                    .width(100),
                                )
                                .push(
                                    crate::gui::widget::text(last_from.clone())
                                        .size(12)
                                        .class(style::Text::Muted)
                                        .width(160),
                                )
                                .push(
                                    crate::gui::widget::text(last_synced)
                                        .size(12)
                                        .class(style::Text::Muted)
                                        .width(140),
                                )
                                .push({
                                    let _game_name = name.clone();
                                    let game_for_menu = name.clone();
                                    let options = match mode {
                                        ludusavi::sync::sync_config::SaveMode::None => vec![],
                                        ludusavi::sync::sync_config::SaveMode::Local => {
                                            if self.sync_games_config.get_auto_sync(name) {
                                                vec!["Sync now", "Backup", "Restore"]
                                            } else {
                                                vec!["Backup", "Restore"]
                                            }
                                        }
                                        ludusavi::sync::sync_config::SaveMode::Cloud => {
                                            if self.sync_games_config.get_auto_sync(name) {
                                                vec!["Sync now", "Force upload", "Force download", "Backup", "Restore"]
                                            } else {
                                                vec!["Force upload", "Force download", "Backup", "Restore"]
                                            }
                                        }
                                        ludusavi::sync::sync_config::SaveMode::Sync => vec![
                                            "Sync now", "Force upload", "Force download", "Backup", "Restore",
                                        ],
                                    };
                                    Container::new(
                                        crate::gui::popup_menu::PopupMenu::new(
                                            options,
                                            move |action| match action {
                                                "Backup" => Message::RequestSyncBackup(game_for_menu.clone()),
                                                "Restore" => Message::RequestSyncRestore(game_for_menu.clone()),
                                                "Sync now" => Message::SyncNow(game_for_menu.clone()),
                                                "Force upload" => Message::RequestForceUpload(game_for_menu.clone()),
                                                "Force download" => Message::RequestForceDownload(game_for_menu.clone()),
                                                _ => Message::Ignore,
                                            },
                                        )
                                        .width(50)
                                        .class(style::PickList::Popup),
                                    )
                                    .width(50)
                                }),
                        )
                        .width(Length::Fill)
                        .class(style::Container::GamesTableRow);
                        let name_for_click = name.clone();
                        let clickable_row = crate::gui::widget::Button::new(row)
                            .on_press(Message::SwitchScreen(Screen::GameDetail(name_for_click)))
                            .width(Length::Fill)
                            .padding(0)
                            .class(style::Button::SidebarItem);

                        rows = rows.push(clickable_row);
                        if !is_last {
                            rows = rows.push(
                                Container::new(crate::gui::widget::Space::new())
                                    .width(Length::Fill)
                                    .height(1)
                                    .class(style::Container::Divider),
                            );
                        }
                    }
                    // Filas de juegos disponibles en el cloud
                    for cloud_game in &cloud_available {
                        let name = &cloud_game.id;
                        if !search_lower.is_empty() && !name.to_lowercase().contains(&search_lower) {
                            continue;
                        }
                
                        let row = Container::new(
                            Row::new()
                                .padding([12, 16])
                                .align_y(Alignment::Center)
                                .push(
                                    Container::new(crate::gui::widget::Space::new())
                                        .width(10)
                                        .height(10)
                                        .class(style::Container::DaemonDotInactive),
                                )
                                .push(crate::gui::widget::Space::new().width(16))
                                .push(
                                    crate::gui::widget::text(name.clone())
                                        .size(13)
                                        .width(Length::Fill),
                                )
                                .push(
                                    crate::gui::widget::text("☁ Available")
                                        .size(11)
                                        .class(style::Text::Accent)
                                        .width(80),
                                )
                                .push(crate::gui::widget::text("—").size(12).class(style::Text::Muted).width(100))
                                .push(crate::gui::widget::text("—").size(12).class(style::Text::Muted).width(160))
                                .push(crate::gui::widget::text("—").size(12).class(style::Text::Muted).width(140))
                                .push(crate::gui::widget::Space::new().width(50)),
                        )
                        .width(Length::Fill)
                        .class(style::Container::GamesTableRow);
                
                        let name_for_click = name.clone();
                        let clickable_row = crate::gui::widget::Button::new(row)
                            .on_press(Message::SwitchScreen(Screen::GameDetail(name_for_click)))
                            .width(Length::Fill)
                            .padding(0)
                            .class(style::Button::SidebarItem);
                
                        rows = rows.push(
                            Container::new(crate::gui::widget::Space::new())
                                .width(Length::Fill)
                                .height(1)
                                .class(style::Container::Divider),
                        );
                        rows = rows.push(clickable_row);
                    }
                }
                let table = Container::new(
                    Column::new()
                        .push(
                            Container::new(table_header)
                                .width(Length::Fill)
                                .class(style::Container::GamesTableRow),
                        )
                        .push(
                            Container::new(crate::gui::widget::Space::new())
                                .width(Length::Fill)
                                .height(1)
                                .class(style::Container::Divider),
                        )
                        .push(rows),
                )
                .width(Length::Fill)
                .class(style::Container::GamesTable);

                let content = Column::new()
                    .push(
                        Container::new(header)
                            .width(Length::Fill)
                            .class(style::Container::TopBar),
                    )
                    .push_if(self.backup_screen.log.search.show, || {
                        Container::new(search_row)
                            .width(Length::Fill)
                            .padding([8, 24])
                            .class(style::Container::TopBar)
                    })
                    .push(
                        Container::new(table)
                            .width(Length::Fill)
                            .padding([24, 24]),
                    );

                ScrollSubject::Other.into_widget(content).into()
            }
            Screen::GameDetail(ref game_name) => {
                let game_name = game_name.clone();
                let meta = self.game_list.get_game(&game_name);
                let mode = self.pending_game_detail
                    .as_ref()
                    .filter(|_| self.pending_game_detail_name.as_deref() == Some(&game_name))
                    .map(|p| &p.mode)
                    .unwrap_or_else(|| self.sync_games_config.get_mode(&game_name));
                let status = self.sync_status.get(&game_name).map(|s| s.as_str()).unwrap_or("");
                let device_id = ludusavi::sync::device::DeviceIdentity::load_or_create(&crate::prelude::app_dir()).id;

                let auto_sync_current = self.pending_game_detail
                    .as_ref()
                    .filter(|_| self.pending_game_detail_name.as_deref() == Some(&game_name))
                    .map(|p| p.auto_sync)
                    .unwrap_or_else(|| self.sync_games_config.get_auto_sync(&game_name));

                let header = Container::new(
                    Row::new()
                        .padding([0, 24])
                        .height(52)
                        .align_y(Alignment::Center)
                        .push(crate::gui::widget::text(game_name.clone()).size(15).width(Length::Fill))
                        .push_if(
                                    self.sync_in_progress.is_some() || self.timed_notification.is_some(),
                                    || {
                                        let msg = self.sync_in_progress.clone()
                                            .or_else(|| self.timed_notification.as_ref().map(|n| n.text.clone()))
                                            .unwrap_or_default();
                                        crate::gui::widget::text(msg)
                                            .size(12)
                                            .class(style::Text::Muted)
                                    }
                                )
                        .push(crate::gui::widget::Space::new().width(16))
                        .push(
                            crate::gui::widget::Button::new(crate::gui::widget::text("← Back").size(13))
                                .padding([7, 14])
                                .class(style::Button::Ghost)
                                .on_press(Message::SwitchScreen(Screen::Games)),
                        )
                        .push(crate::gui::widget::Space::new().width(8))
                        .push_if(
                            matches!(mode, ludusavi::sync::sync_config::SaveMode::Sync),
                            || {
                                Row::new()
                                    .spacing(8)
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Sync now").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Primary)
                                        .on_press(Message::SyncNow(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Force upload").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestForceUpload(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Force download").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestForceDownload(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Backup").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestSyncBackup(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Restore").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestSyncRestore(game_name.clone()))
                                    )
                            }
                        )
                        .push_if(
                            matches!(mode, ludusavi::sync::sync_config::SaveMode::Local) && auto_sync_current,
                            || {
                                Row::new()
                                    .spacing(8)
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Sync now").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Primary)
                                        .on_press(Message::SyncNow(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Backup").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestSyncBackup(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Restore").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestSyncRestore(game_name.clone()))
                                    )
                            }
                        )
                        .push_if(
                            matches!(mode, ludusavi::sync::sync_config::SaveMode::Cloud) && auto_sync_current,
                            || {
                                Row::new()
                                    .spacing(8)
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Sync now").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Primary)
                                        .on_press(Message::SyncNow(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Force upload").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestForceUpload(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Force download").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestForceDownload(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Backup").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestSyncBackup(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Restore").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestSyncRestore(game_name.clone()))
                                    )
                            }
                        )
                        .push_if(
                            matches!(mode, ludusavi::sync::sync_config::SaveMode::Local) && !auto_sync_current,
                            || {
                                Row::new()
                                    .spacing(8)
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Backup").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Primary)
                                        .on_press(Message::RequestSyncBackup(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Restore").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestSyncRestore(game_name.clone()))
                                    )
                            }
                        )
                        .push_if(
                            matches!(mode, ludusavi::sync::sync_config::SaveMode::Cloud) && !auto_sync_current,
                            || {
                                Row::new()
                                    .spacing(8)
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Backup").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Primary)
                                        .on_press(Message::RequestSyncBackup(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Restore").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestSyncRestore(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Force upload").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestForceUpload(game_name.clone()))
                                    )
                                    .push(
                                        crate::gui::widget::Button::new(
                                            crate::gui::widget::text("Force download").size(13)
                                        )
                                        .padding([7, 14])
                                        .class(style::Button::Ghost)
                                        .on_press(Message::RequestForceDownload(game_name.clone()))
                                    )
                            }
                        ),
                )
                .width(Length::Fill)
                .class(style::Container::TopBar);

                // Sync status card
                // Detectar si el juego está disponible en el cloud pero no tiene SYNC local
                let is_cloud_available = self.game_list.games.iter().any(|g| g.id == game_name)
                    && !matches!(mode, ludusavi::sync::sync_config::SaveMode::Sync);
                let status_card = {
                    let last_sync_str = meta
                        .and_then(|m| m.last_sync_time_utc)
                        .map(|t| {
                            let now = chrono::Utc::now();
                            let diff = now.signed_duration_since(t);
                            if diff.num_minutes() < 1 {
                                "just now".to_string()
                            } else if diff.num_hours() < 1 {
                                format!("{} min ago", diff.num_minutes())
                            } else if diff.num_hours() < 24 {
                                format!("{} hours ago", diff.num_hours())
                            } else {
                                format!("{} days ago", diff.num_days())
                            }
                        });

                    let last_sync_from = meta
                        .and_then(|m| m.last_synced_from.as_deref())
                        .map(|id| self.game_list.get_device_name(id).to_string());

                    let (status_text, status_detail) = match mode {
                        ludusavi::sync::sync_config::SaveMode::None => (
                            "— Not managed",
                            "This game is not managed by Save Sync.".to_string(),
                        ),
                        ludusavi::sync::sync_config::SaveMode::Local => {
                            match status {
                                "synced" => ("💾 Backed up", "Save files are backed up locally.".to_string()),
                                "pending_backup" => ("⏳ Pending backup", "Save files have changed since the last backup.".to_string()),
                                "pending_restore" => ("⚠ Pending restore", "A newer backup exists. Restore to get the latest saves.".to_string()),
                                _ => ("💾 Local", "Local backup mode.".to_string()),
                            }
                        }
                        ludusavi::sync::sync_config::SaveMode::Cloud => {
                            match status {
                                "synced" => ("☁ Backed up", format!("Last backed up{}{}.",
                                    last_sync_str.as_deref().map(|w| format!(" {}", w)).unwrap_or_default(),
                                    last_sync_from.as_deref().map(|f| format!(" from {}", f)).unwrap_or_default(),
                                )),
                                "pending_backup" => ("⏳ Pending backup", "Save files have changed since the last backup.".to_string()),
                                "pending_restore" => ("⚠ Pending restore", "A newer backup exists in the cloud. Restore to get the latest saves.".to_string()),
                                _ => ("☁ Cloud", "Never backed up.".to_string()),
                            }
                        }
                        ludusavi::sync::sync_config::SaveMode::Sync => {
                            match status {
                                "synced" => ("✓ Up to date", format!("Last synced{}{}.",
                                    last_sync_str.as_deref().map(|w| format!(" {}", w)).unwrap_or_default(),
                                    last_sync_from.as_deref().map(|f| format!(" from {}", f)).unwrap_or_default(),
                                )),
                                "pending_backup" => ("⏳ Pending upload", "Local saves are newer than the cloud. Syncing soon.".to_string()),
                                "pending_restore" => ("⬇ Pending download", "Cloud saves are newer than local. Syncing soon.".to_string()),
                                _ => ("⚠ Never synced", "This game has not been synced yet. Run a sync to get started.".to_string()),
                            }
                        }
                    };

                    Container::new(
                        Column::new()
                            .spacing(6)
                            .push(crate::gui::widget::text(status_text).size(13))
                            .push(crate::gui::widget::text(status_detail).size(12).class(style::Text::Muted))
                            .push_if(meta.map(|m| m.storage_bytes > 0).unwrap_or(false), || {
                                crate::gui::widget::text(
                                    TRANSLATOR.adjusted_size(meta.unwrap().storage_bytes)
                                )
                                .size(11)
                                .class(style::Text::Muted)
                            }),
                    )
                    .width(Length::Fill)
                    .padding(14)
                    .class(style::Container::GameListEntry)
                };

                let game_for_mode = game_name.clone();
                let current_mode = mode.clone();
                let settings_card = Container::new(
                    Column::new()
                        .spacing(10)
                        .push(crate::gui::widget::text("SAVE MODE").size(11).class(style::Text::Muted))
                        .push(
                            Row::new()
                                .spacing(8)
                                .push({
                                    let g = game_for_mode.clone();
                                    crate::gui::widget::Button::new(
                                        crate::gui::widget::text("NONE").size(12)
                                    )
                                    .padding([6, 14])
                                    .class(if matches!(current_mode, ludusavi::sync::sync_config::SaveMode::None) {
                                        style::Button::Primary
                                    } else {
                                        style::Button::Ghost
                                    })
                                    .on_press(Message::SetGameSaveMode(g, ludusavi::sync::sync_config::SaveMode::None))
                                })
                                .push({
                                    let g = game_for_mode.clone();
                                    crate::gui::widget::Button::new(
                                        crate::gui::widget::text("LOCAL").size(12)
                                    )
                                    .padding([6, 14])
                                    .class(if matches!(current_mode, ludusavi::sync::sync_config::SaveMode::Local) {
                                        style::Button::Primary
                                    } else {
                                        style::Button::Ghost
                                    })
                                    .on_press(Message::SetGameSaveMode(g, ludusavi::sync::sync_config::SaveMode::Local))
                                })
                                .push({
                                    let g = game_for_mode.clone();
                                    crate::gui::widget::Button::new(
                                        crate::gui::widget::text("CLOUD").size(12)
                                    )
                                    .padding([6, 14])
                                    .class(if matches!(current_mode, ludusavi::sync::sync_config::SaveMode::Cloud) {
                                        style::Button::Primary
                                    } else {
                                        style::Button::Ghost
                                    })
                                    .on_press(Message::SetGameSaveMode(g, ludusavi::sync::sync_config::SaveMode::Cloud))
                                })
                                .push({
                                    let g = game_for_mode.clone();
                                    crate::gui::widget::Button::new(
                                        crate::gui::widget::text("SYNC").size(12)
                                    )
                                    .padding([6, 14])
                                    .class(if matches!(current_mode, ludusavi::sync::sync_config::SaveMode::Sync) {
                                        style::Button::Primary
                                    } else {
                                        style::Button::Ghost
                                    })
                                    .on_press(Message::SetGameSaveMode(g, ludusavi::sync::sync_config::SaveMode::Sync))
                                }),
                        )
                        .push(crate::gui::widget::text("SAVE LOCATION").size(11).class(style::Text::Muted))
                        .push(
                            meta.and_then(|m| m.path_by_device.get(&device_id))
                                .map(|path| {
                                    let p = path.clone();
                                    Container::new(
                                        Row::new()
                                            .spacing(8)
                                            .align_y(Alignment::Center)
                                            .push(
                                                crate::gui::widget::text(p.clone())
                                                    .size(12)
                                                    .class(style::Text::Dim)
                                                    .width(Length::Fill)
                                            )
                                            .push(
                                                crate::gui::widget::Button::new(
                                                    crate::gui::widget::text("Open").size(12)
                                                )
                                                .padding([4, 10])
                                                .class(style::Button::Ghost)
                                                .on_press(Message::OpenDir {
                                                    path: crate::prelude::StrictPath::new(p),
                                                })
                                            )
                                    )
                                    .width(Length::Fill)
                                    .padding([4, 10])
                                    .class(style::Container::GamesTableRow)
                                })
                                .unwrap_or_else(|| {
                                    Container::new(
                                        crate::gui::widget::text("No save location detected")
                                            .size(12)
                                            .class(style::Text::Muted),
                                    )
                                    .width(Length::Fill)
                                    .padding([8, 10])
                                    .class(style::Container::GamesTableRow)
                                }),
                        )
                        .push_if(
                            matches!(current_mode, ludusavi::sync::sync_config::SaveMode::Local | ludusavi::sync::sync_config::SaveMode::Cloud),
                            || {
                                let auto_sync = self.pending_game_detail
                                    .as_ref()
                                    .filter(|_| self.pending_game_detail_name.as_deref() == Some(&game_name))
                                    .map(|p| p.auto_sync)
                                    .unwrap_or_else(|| self.sync_games_config.get_auto_sync(&game_name));
                                let g = game_name.clone();
                                Column::new()
                                    .spacing(6)
                                    .push(crate::gui::widget::text("AUTO SYNC").size(11).class(style::Text::Muted))
                                    .push(
                                        Row::new()
                                            .spacing(10)
                                            .align_y(Alignment::Center)
                                            .push(
                                                crate::gui::widget::Button::new(
                                                    crate::gui::widget::text(if auto_sync { "ON" } else { "OFF" }).size(12)
                                                )
                                                .padding([6, 14])
                                                .class(if auto_sync {
                                                    style::Button::Primary
                                                } else {
                                                    style::Button::Ghost
                                                })
                                                .on_press(Message::SetGameAutoSync(g, !auto_sync))
                                            )
                                            .push(
                                                crate::gui::widget::text(
                                                    if matches!(current_mode, ludusavi::sync::sync_config::SaveMode::Local) {
                                                        "Automatically backup when saves change"
                                                    } else {
                                                        "Automatically backup and upload when saves change"
                                                    }
                                                )
                                                .size(12)
                                                .class(style::Text::Muted)
                                            ),
                                    )
                            }
                        ),
                )
                .width(Length::Fill)
                .padding(16)
                .class(style::Container::GamesTable);

                let has_pending = self.pending_game_detail_name.as_deref() == Some(&game_name)
                    && self.pending_game_detail.as_ref().map(|pending| {
                        let original = self.sync_games_config.games
                            .get(&game_name)
                            .cloned()
                            .unwrap_or_default();
                        pending.mode != original.mode || pending.auto_sync != original.auto_sync
                    }).unwrap_or(false);

                let save_button = crate::gui::widget::Button::new(
                    crate::gui::widget::text("Save changes").size(13)
                )
                .padding([8, 20])
                .class(if has_pending {
                    style::Button::Primary
                } else {
                    style::Button::Ghost
                })
                .on_press_maybe(has_pending.then_some(Message::SaveGameDetail));

                // Devices section
                let devices_card = {
                    let mut devices_col = Column::new()
                        .spacing(6)
                        .push(crate::gui::widget::text("DEVICES").size(11).class(style::Text::Muted));

                    if let Some(meta) = meta {
                        for (dev_id, path) in &meta.path_by_device {
                            let is_this = dev_id == &device_id;
                            devices_col = devices_col.push(
                                Container::new(
                                    Row::new()
                                        .spacing(10)
                                        .align_y(Alignment::Center)
                                        .push(
                                            Column::new()
                                                .push(
                                                    Row::new()
                                                        .spacing(6)
                                                        .push(crate::gui::widget::text(dev_id.clone()).size(13))
                                                        .push_if(is_this, || {
                                                            crate::gui::widget::text("THIS DEVICE")
                                                                .size(10)
                                                                .class(style::Text::Accent)
                                                        }),
                                                )
                                                .push(
                                                    crate::gui::widget::text(path.clone())
                                                        .size(11)
                                                        .class(style::Text::Muted),
                                                ),
                                        ),
                                )
                                .width(Length::Fill)
                                .padding([8, 10])
                                .class(style::Container::GamesTableRow),
                            );
                        }
                    } else {
                        devices_col = devices_col.push(
                            crate::gui::widget::text("No devices found. Run a sync to register this device.")
                                .size(12)
                                .class(style::Text::Muted),
                        );
                    }

                    Container::new(devices_col)
                        .width(Length::Fill)
                        .padding(16)
                        .class(style::Container::GamesTable)
                };

                // FILES expandible
                let entry = self.backup_screen.log.entries.iter().find(|e| e.scan_info.game_name == game_name);

                let is_scanning = !self.operation.idle()
                    && self.operation.games().is_some_and(|g| g.contains(&game_name));

                let files_card = {
                    let header_button = crate::gui::widget::Button::new(
                        Row::new()
                            .spacing(8)
                            .align_y(Alignment::Center)
                            .push(crate::gui::widget::text("FILES").size(11).class(style::Text::Muted))
                            .push_if(is_scanning, || {
                                crate::gui::widget::text("Scanning...").size(11).class(style::Text::Muted)
                            })
                    )
                    .padding([6, 0])
                    .class(style::Button::Bare)
                    .on_press(Message::GameDetailFilesToggled);

                    let files_content = if self.game_detail_files_expanded {
                        match entry {
                            Some(e) if e.scanned => {
                                if e.scan_info.found_files.is_empty() {
                                    Some(Container::new(
                                        crate::gui::widget::text("No save files detected.")
                                            .size(12)
                                            .class(style::Text::Muted)
                                    ).padding([8, 0]))
                                } else {
                                    e.tree.as_ref().map(|tree| {
                                        Container::new(
                                            tree.view(&game_name, &self.config, ScanKind::Backup)
                                                .width(Length::Fill)
                                        ).padding([8, 0])
                                    })
                                }
                            }
                            _ => Some(Container::new(
                                crate::gui::widget::text("Scanning...")
                                    .size(12)
                                    .class(style::Text::Muted)
                            ).padding([8, 0])),
                        }
                    } else {
                        None
                    };

                    Container::new(
                        Column::new()
                            .push(header_button)
                            .push_if(files_content.is_some(), || files_content.unwrap())
                    )
                    .width(Length::Fill)
                    .padding(16)
                    .class(style::Container::GamesTable)
                };

                let content = Column::new()
                    .push(header)
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(16)
                                .padding([24, 24])
                                .push_if(is_cloud_available, || {
                                    Container::new(
                                        Row::new()
                                            .spacing(12)
                                            .align_y(Alignment::Center)
                                            .push(
                                                crate::gui::widget::text("☁ This game is synced from another device.")
                                                    .size(13)
                                                    .width(Length::Fill)
                                            )
                                            .push(
                                                crate::gui::widget::Button::new(
                                                    crate::gui::widget::text("Enable Sync").size(13)
                                                )
                                                .padding([7, 14])
                                                .class(style::Button::Primary)
                                                .on_press(Message::EnableCloudSync(game_name.clone()))
                                            )
                                    )
                                    .width(Length::Fill)
                                    .padding([12, 16])
                                    .class(style::Container::GameListEntry)
                                })
                                .push(status_card)
                                .push(settings_card)
                                .push(devices_card)
                                .push(save_button)
                                .push(files_card),
                        )
                        .width(Length::Fill),
                    );

                ScrollSubject::GameDetail.into_widget(content).into()
            }
            Screen::ThisDevice => {
                let device = ludusavi::sync::device::DeviceIdentity::load_or_create(&crate::prelude::app_dir());
                let device_id_short = device.id.clone();

                let monitored_games: Vec<String> = self.game_list.games.iter()
                    .filter(|g| g.path_by_device.contains_key(&device.id))
                    .map(|g| g.name.clone())
                    .collect();

                let last_sync = {
                    let path = crate::prelude::app_dir().joined("daemon-state.json");
                    path.read()
                        .and_then(|content| {
                            serde_json::from_str::<serde_json::Value>(&content).ok()
                        })
                        .and_then(|json| json.get("last_known_mod_time").and_then(|v| v.as_str()).map(|s| s.to_string()))
                        .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
                        .map(|t| {
                            let now = chrono::Utc::now();
                            let diff = now.signed_duration_since(t.with_timezone(&chrono::Utc));
                            if diff.num_minutes() < 1 {
                                "just now".to_string()
                            } else if diff.num_hours() < 1 {
                                format!("{} min ago", diff.num_minutes())
                            } else if diff.num_hours() < 24 {
                                format!("{} hours ago", diff.num_hours())
                            } else {
                                format!("{} days ago", diff.num_days())
                            }
                        })
                        .unwrap_or_else(|| "Never".to_string())
                };

                let provider_name = match &self.config.cloud.remote {
                    Some(crate::cloud::Remote::GoogleDrive { .. }) => "Google Drive",
                    Some(crate::cloud::Remote::Dropbox { .. }) => "Dropbox",
                    Some(crate::cloud::Remote::OneDrive { .. }) => "OneDrive",
                    Some(crate::cloud::Remote::Box { .. }) => "Box",
                    Some(crate::cloud::Remote::Ftp { .. }) => "FTP",
                    Some(crate::cloud::Remote::Smb { .. }) => "SMB",
                    Some(crate::cloud::Remote::WebDav { .. }) => "WebDAV",
                    Some(crate::cloud::Remote::Custom { .. }) => "Custom",
                    None => "Not configured",
                };

                let remote_id = self.config.cloud.remote.as_ref()
                    .map(|r| r.id().to_string())
                    .unwrap_or_else(|| "—".to_string());

                let rclone_ok = self.config.apps.rclone.is_valid();

                let header = Container::new(
                    Row::new()
                        .padding([0, 24])
                        .height(52)
                        .align_y(Alignment::Center)
                        .push(crate::gui::widget::text("This device").size(15).width(Length::Fill)),
                )
                .width(Length::Fill)
                .class(style::Container::TopBar);

                // DEVICE card
                let _log_path = crate::prelude::app_dir().joined("daemon.log").render();
                let device_card = Container::new(
                    Column::new()
                        .spacing(10)
                        .push(crate::gui::widget::text("DEVICE").size(11).class(style::Text::Muted))
                        .push(
                            Row::new()
                                .spacing(16)
                                .align_y(Alignment::Center)
                                .push(
                                    Column::new()
                                        .width(Length::Fill)
                                        .spacing(4)
                                        .push(crate::gui::widget::text(device.name.clone()).size(14))
                                        .push(crate::gui::widget::text(device_id_short).size(11).class(style::Text::Muted)),
                                )
                                .push(
                                    crate::gui::widget::Button::new(
                                        crate::gui::widget::text("Open logs").size(12)
                                    )
                                    .padding([6, 14])
                                    .class(style::Button::Ghost)
                                    .on_press(Message::OpenDir {
                                        path: crate::prelude::app_dir(),
                                    }),
                                ),
                        ),
                )
                .width(Length::Fill)
                .padding(16)
                .class(style::Container::GamesTable);

                // SYNC DAEMON card
                let daemon_card = Container::new(
                    Column::new()
                        .spacing(10)
                        .push(crate::gui::widget::text("SYNC DAEMON").size(11).class(style::Text::Muted))
                        .push(
                            Row::new()
                                .spacing(8)
                                .align_y(Alignment::Center)
                                .push(
                                    Container::new(crate::gui::widget::Space::new())
                                        .width(8)
                                        .height(8)
                                        .class(if self.daemon_running {
                                            style::Container::DaemonDotActive
                                        } else {
                                            style::Container::DaemonDotInactive
                                        }),
                                )
                                .push(
                                    crate::gui::widget::text(if self.daemon_running {
                                        "Daemon is running"
                                    } else {
                                        "Daemon is stopped"
                                    })
                                    .size(13)
                                    .class(if self.daemon_running {
                                        style::Text::Green
                                    } else {
                                        style::Text::Muted
                                    }),
                                ),
                        )
                        .push(
                            Row::new()
                                .spacing(6)
                                .push(crate::gui::widget::text("Last sync:").size(12).class(style::Text::Muted))
                                .push(crate::gui::widget::text(last_sync).size(12).class(style::Text::Dim)),
                        )
                        .push(
                            Column::new()
                                .spacing(4)
                                .push(crate::gui::widget::text("Monitoring:").size(12).class(style::Text::Muted))
                                .push(if monitored_games.is_empty() {
                                    crate::gui::widget::text("No games registered for this device")
                                        .size(12)
                                        .class(style::Text::Muted)
                                } else {
                                    crate::gui::widget::text(monitored_games.join(", "))
                                        .size(12)
                                        .class(style::Text::Dim)
                                }),
                        ),
                )
                .width(Length::Fill)
                .padding(16)
                .class(style::Container::GamesTable);

                // CLOUD STORAGE card
                let cloud_card = Container::new(
                    Column::new()
                        .spacing(10)
                        .push(crate::gui::widget::text("CLOUD STORAGE").size(11).class(style::Text::Muted))
                        .push(
                            Row::new()
                                .spacing(6)
                                .push(crate::gui::widget::text("Provider:").size(12).class(style::Text::Muted))
                                .push(crate::gui::widget::text(provider_name).size(12).class(style::Text::Dim)),
                        )
                        .push(
                            Row::new()
                                .spacing(6)
                                .push(crate::gui::widget::text("Remote:").size(12).class(style::Text::Muted))
                                .push(crate::gui::widget::text(remote_id).size(12).class(style::Text::Dim)),
                        )
                        .push(
                            Row::new()
                                .spacing(6)
                                .push(crate::gui::widget::text("Path:").size(12).class(style::Text::Muted))
                                .push(crate::gui::widget::text(self.config.cloud.path.clone()).size(12).class(style::Text::Dim)),
                        )
                        .push(
                            Row::new()
                                .spacing(8)
                                .align_y(Alignment::Center)
                                .push(
                                    Container::new(crate::gui::widget::Space::new())
                                        .width(8)
                                        .height(8)
                                        .class(if rclone_ok {
                                            style::Container::DaemonDotActive
                                        } else {
                                            style::Container::DaemonDotInactive
                                        }),
                                )
                                .push(
                                    crate::gui::widget::text(if rclone_ok {
                                        "rclone configured"
                                    } else {
                                        "rclone not configured"
                                    })
                                    .size(12)
                                    .class(if rclone_ok {
                                        style::Text::Green
                                    } else {
                                        style::Text::Muted
                                    }),
                                ),
                        ),
                )
                .width(Length::Fill)
                .padding(16)
                .class(style::Container::GamesTable);

                let content = Column::new()
                    .push(header)
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(16)
                                .padding([24, 24])
                                .push(device_card)
                                .push(daemon_card)
                                .push(cloud_card),
                        )
                        .width(Length::Fill),
                    );

                ScrollSubject::Other.into_widget(content).into()
            }
            Screen::AllDevices => {
                let device = ludusavi::sync::device::DeviceIdentity::load_or_create(&crate::prelude::app_dir());

                // Recopilar todos los devices del game-list
                let mut device_map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
                for game in &self.game_list.games {
                    for dev_id in game.path_by_device.keys() {
                        device_map.entry(dev_id.clone()).or_default().push(game.name.clone());
                    }
                }

                let total_devices = device_map.len();

                let header = Container::new(
                    Row::new()
                        .padding([0, 24])
                        .height(52)
                        .align_y(Alignment::Center)
                        .push(crate::gui::widget::text("All devices").size(15).width(Length::Fill))
                        .push(
                            crate::gui::widget::text(format!("{} device{} registered",
                                total_devices,
                                if total_devices == 1 { "" } else { "s" }
                            ))
                            .size(12)
                            .class(style::Text::Muted),
                        ),
                )
                .width(Length::Fill)
                .class(style::Container::TopBar);

                let mut devices_col = Column::new().spacing(12);

                if device_map.is_empty() {
                    devices_col = devices_col.push(
                        crate::gui::widget::text("No devices found. Run a sync to register devices.")
                            .size(13)
                            .class(style::Text::Muted),
                    );
                } else {
                    // Ordenar — this device primero
                    let mut device_ids: Vec<String> = device_map.keys().cloned().collect();
                    device_ids.sort_by(|a, b| {
                        if a == &device.id { std::cmp::Ordering::Less }
                        else if b == &device.id { std::cmp::Ordering::Greater }
                        else { a.cmp(b) }
                    });

                    for dev_id in device_ids {
                        let is_this = dev_id == device.id;
                        let games = device_map.get(&dev_id).cloned().unwrap_or_default();

                        // Último sync desde este device
                        let last_sync = self.game_list.games.iter()
                            .filter(|g| g.last_synced_from.as_deref() == Some(&dev_id))
                            .filter_map(|g| g.last_sync_time_utc)
                            .max()
                            .map(|t| {
                                let now = chrono::Utc::now();
                                let diff = now.signed_duration_since(t);
                                if diff.num_minutes() < 1 {
                                    "just now".to_string()
                                } else if diff.num_hours() < 1 {
                                    format!("{} min ago", diff.num_minutes())
                                } else if diff.num_hours() < 24 {
                                    format!("{} hours ago", diff.num_hours())
                                } else {
                                    format!("{} days ago", diff.num_days())
                                }
                            })
                            .unwrap_or_else(|| "Never".to_string());

                        let uuid_display = self.game_list.get_device_name(&dev_id).to_string();

                        let device_card = Container::new(
                            Column::new()
                                .spacing(8)
                                .push(
                                    Row::new()
                                        .spacing(8)
                                        .align_y(Alignment::Center)
                                        .push(
                                            crate::gui::widget::text(uuid_display)
                                                .size(13)
                                                .class(style::Text::Dim)
                                                .width(Length::Fill),
                                        )
                                        .push_if(is_this, || {
                                            crate::gui::widget::text("THIS DEVICE")
                                                .size(10)
                                                .class(style::Text::Accent)
                                        }),
                                )
                                .push(
                                    Row::new()
                                        .spacing(6)
                                        .push(
                                            crate::gui::widget::text(
                                                format!("{} game{}: {}",
                                                    games.len(),
                                                    if games.len() == 1 { "" } else { "s" },
                                                    games.join(", ")
                                                )
                                            )
                                            .size(12)
                                            .class(style::Text::Muted),
                                        ),
                                )
                                .push(
                                    Row::new()
                                        .spacing(6)
                                        .push(crate::gui::widget::text("Last sync:").size(11).class(style::Text::Muted))
                                        .push(crate::gui::widget::text(last_sync).size(11).class(style::Text::Dim)),
                                ),
                        )
                        .width(Length::Fill)
                        .padding(16)
                        .class(style::Container::GamesTable);

                        devices_col = devices_col.push(device_card);
                    }
                }

                let content = Column::new()
                    .push(header)
                    .push(
                        Container::new(devices_col)
                            .width(Length::Fill)
                            .padding([24, 24]),
                    );

                ScrollSubject::Other.into_widget(content).into()
            }
            Screen::Other => screen::other(
                self.updating_manifest,
                &self.config,
                &self.cache,
                &self.operation,
                &self.text_histories,
                &self.modifiers,
            ),
        };

        let body = Row::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(sidebar)
            .push(
                Container::new(main_content)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .class(style::Container::ContentArea),
            );

        let stack = Stack::new()
            .push(Container::new(body).class(style::Container::Primary))
            .push(
                self.modals
                    .last()
                    .map(|modal| modal.view(&self.config, &self.text_histories, &self.operation)),
            );

        Column::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(stack)
            .push_if(self.progress.visible(), || self.progress.view(&self.operation))
            .push(self.manifest_notification.as_ref().map(|x| x.view()))
            .into()
    }
}
