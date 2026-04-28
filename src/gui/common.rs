use std::collections::{HashMap, HashSet};

use iced::Length;

use crate::{
    cloud::{rclone_monitor, Remote, RemoteChoice},
    gui::{
        modal::{ModalField, ModalInputKind},
    },
    prelude::{CommandError, EditAction, Error, Finality, StrictPath, SyncDirection},
    resource::{
        config::{self, Root},
        manifest::{Manifest, ManifestUpdate},
    },
    scan::{
        game_filter,
        layout::{BackupLayout, GameLayout},
        registry::RegistryItem,
        BackupInfo, Launchers, ScanInfo, SteamShortcuts,
    },
};

#[derive(Clone, Debug, Default)]
pub struct Flags {
    pub update_manifest: bool,
    pub custom_game: Option<String>,
}

#[derive(Debug, Clone)]
pub enum BackupPhase {
    Start {
        preview: bool,
        /// Was this backup triggered by a validation check?
        repair: bool,
        /// Jump to the first game in the list after executing.
        jump: bool,
        games: Option<GameSelection>,
    },
    CloudCheck,
    Load,
    RegisterCommands {
        subjects: Vec<String>,
        manifest: Manifest,
        layout: Box<BackupLayout>,
        steam: SteamShortcuts,
        launchers: Launchers,
    },
    GameScanned {
        scan_info: Option<ScanInfo>,
        backup_info: Option<BackupInfo>,
    },
    CloudSync,
    Done,
}

#[derive(Debug, Clone)]
pub enum Message {
    Ignore,
    Exit {
        user: bool,
    },
    Save,
    CloseModal,
    UpdateTime,
    PruneNotifications,
    DaemonStatusChecked(bool, std::collections::HashMap<String, crate::gui::app::GameStatusInfo>, ludusavi::sync::game_list::GameListFile, bool, bool),
    SetGameSaveMode(String, ludusavi::sync::sync_config::SaveMode),
    SetGameAutoSync(String, bool),
    GamesSearchChanged(String),
    SaveGameDetail,
    Config {
        event: config::Event,
    },
    UpdateManifest {
        force: bool,
    },
    ManifestUpdated(Vec<Result<Option<ManifestUpdate>, Error>>),
    Backup(BackupPhase),
    FindRoots,
    ConfirmAddMissingRoots(Vec<Root>),
    SwitchScreen(Screen),
    ToggleGameListEntryTreeExpanded {
        name: String,
        keys: Vec<TreeNodeKey>,
    },
    Filter {
        event: game_filter::Event,
    },
    BrowseDir(BrowseSubject),
    BrowseFile(BrowseFileSubject),
    SelectedFile(BrowseFileSubject, StrictPath),
    OpenDir {
        path: StrictPath,
    },
    OpenDirSubject(BrowseSubject),
    OpenFileSubject(BrowseFileSubject),
    OpenDirFailure {
        path: StrictPath,
    },
    OpenUrlFailure {
        url: String,
    },
    KeyboardEvent(iced::keyboard::Event),
    UndoRedo(crate::gui::undoable::Action, UndoSubject),
    Scrolled {
        subject: ScrollSubject,
        position: iced::widget::scrollable::AbsoluteOffset,
    },
    Scroll {
        subject: ScrollSubject,
        position: iced::widget::scrollable::AbsoluteOffset,
    },
    OpenUrl(String),
    EditedCloudRemote(RemoteChoice),
    ConfigureCloudSuccess(Remote),
    ConfigureCloudFailure(CommandError),
    SynchronizeCloud {
        direction: SyncDirection,
        finality: Finality,
    },
    RcloneMonitor(rclone_monitor::Event),
    FinalizeRemote(Remote),
    EditedModalField(ModalField),
    ModalChangePage(usize),
    ShowScanActiveGames,
    CopyText(String),
    OpenRegistry(RegistryItem),
    RequestSyncBackup(String),
    RequestSyncRestore(String),
    RequestForceUpload(String),
    RequestForceDownload(String),
    SyncBackupGame(String),
    ConfirmSyncModeChange {
        game: String,
        previous_mode: ludusavi::sync::sync_config::SaveMode,
    },
    SyncRestoreGame(String),
    SyncNow(String),
    /// Toggle del flag global "safety backups enabled" en sync-games.json.
    ToggleSafetyBackupsEnabled(bool),
    /// Toggle global para mostrar notificaciones nativas del SO desde el daemon.
    ToggleSystemNotificationsEnabled(bool),
    /// Pide confirmación antes de restaurar un safety backup.
    RequestRestoreSafetyBackup(String),
    /// Ejecuta el restore del safety backup del juego indicado.
    RestoreSafetyBackup(String),
    /// Pide confirmación antes de borrar un safety backup.
    RequestDeleteSafetyBackup(String),
    /// Borra el safety backup del juego indicado.
    /// El usuario pulsa "Keep local" en el banner de conflict — fuerza upload.
    ResolveConflictKeepLocal(String),
    /// El usuario pulsa "Keep cloud" en el banner de conflict — fuerza download.
    ResolveConflictKeepCloud(String),
    /// Pide confirmación antes de Keep both (operación con disk side-effects).
    RequestResolveConflictKeepBoth(String),
    /// Ejecuta el Keep both: snapshot permanente del local + download del cloud.
    ResolveConflictKeepBoth(String),
    DeleteSafetyBackup(String),
    ForceUploadGame(String),
    ForceDownloadGame(String),
    ShowTimedNotification(String),
    GameDetailFilesToggled,
    InstallService,
    UninstallService,
    /// Arranca el daemon (Windows: Start-ScheduledTask)
    StartDaemon,
    /// Para el daemon (Windows: kill del proceso)
    StopDaemon,
    EnableCloudSync(String),
    AddGameRequested,
    AddGameNameChanged(String),
    AddGamePathChanged(String),
    AddGameConfirm,
    RemoveCustomGameRequested(String),
    RemoveCustomGameConfirm(String),
}

impl Message {
    pub fn browsed_dir(subject: BrowseSubject, choice: Option<std::path::PathBuf>) -> Self {
        match choice {
            Some(path) => match subject {
                BrowseSubject::AddGamePath => {
                    return Message::AddGamePathChanged(crate::path::render_pathbuf(&path));
                }
                BrowseSubject::BackupTarget => config::Event::BackupTarget(crate::path::render_pathbuf(&path)),
                BrowseSubject::Root(i) => config::Event::Root(EditAction::Change(
                    i,
                    globetter::Pattern::escape(&crate::path::render_pathbuf(&path)),
                )),
            }
            .into(),
            None => Message::Ignore,
        }
    }

    pub fn browsed_file(subject: BrowseFileSubject, choice: Option<std::path::PathBuf>) -> Self {
        match choice {
            Some(path) => Message::SelectedFile(subject, StrictPath::from(path)),
            None => Message::Ignore,
        }
    }

    pub fn config<T>(convert: impl Fn(T) -> config::Event) -> impl Fn(T) -> Self {
        move |value: T| Self::Config { event: convert(value) }
    }
}

impl From<config::Event> for Message {
    fn from(event: config::Event) -> Self {
        Self::Config { event }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameSelection {
    Single { game: String },
    Group { games: HashSet<String> },
}

impl GameSelection {
    pub fn single(game: String) -> Self {
        Self::Single { game }
    }

    pub fn group(games: HashSet<String>) -> Self {
        Self::Group { games }
    }

    pub fn is_single(&self) -> bool {
        matches!(self, Self::Single { .. })
    }

    pub fn contains(&self, query: &str) -> bool {
        match self {
            Self::Single { game } => game == query,
            Self::Group { games } => games.contains(query),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Single { .. } => false,
            Self::Group { games } => games.is_empty(),
        }
    }

    pub fn iter<'a>(&'a self) -> Box<dyn Iterator<Item = &'a String> + 'a> {
        match self {
            Self::Single { game } => Box::new(std::iter::once(game)),
            Self::Group { games } => Box::new(games.iter()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Operation {
    #[default]
    Idle,
    Backup {
        finality: Finality,
        cancelling: bool,
        checking_cloud: bool,
        syncing_cloud: bool,
        should_sync_cloud_after: bool,
        games: Option<GameSelection>,
        errors: Vec<Error>,
        cloud_changes: i64,
        force_new_full_backup: bool,
        syncable_games: HashSet<String>,
        active_games: HashMap<String, chrono::DateTime<chrono::Utc>>,
    },
    Restore {
        finality: Finality,
        cancelling: bool,
        checking_cloud: bool,
        games: Option<GameSelection>,
        errors: Vec<Error>,
        cloud_changes: i64,
        active_games: HashMap<String, chrono::DateTime<chrono::Utc>>,
    },
    Cloud {
        direction: SyncDirection,
        finality: Finality,
        cancelling: bool,
        errors: Vec<Error>,
        cloud_changes: i64,
    },
}

impl Operation {
    pub fn idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    pub fn new_backup(finality: Finality, games: Option<GameSelection>) -> Self {
        Self::Backup {
            finality,
            cancelling: false,
            checking_cloud: false,
            syncing_cloud: false,
            should_sync_cloud_after: false,
            games,
            errors: vec![],
            cloud_changes: 0,
            force_new_full_backup: false,
            syncable_games: HashSet::new(),
            active_games: HashMap::new(),
        }
    }

    pub fn new_restore(finality: Finality, games: Option<GameSelection>) -> Self {
        Self::Restore {
            finality,
            cancelling: false,
            checking_cloud: false,
            games,
            errors: vec![],
            cloud_changes: 0,
            active_games: HashMap::new(),
        }
    }

    pub fn new_cloud(direction: SyncDirection, finality: Finality) -> Self {
        Self::Cloud {
            direction,
            finality,
            cancelling: false,
            errors: vec![],
            cloud_changes: 0,
        }
    }

    pub fn preview(&self) -> bool {
        match self {
            Operation::Idle => true,
            Operation::Backup { finality, .. } => finality.preview(),
            Operation::Restore { finality, .. } => finality.preview(),
            Operation::Cloud { finality, .. } => finality.preview(),
        }
    }

    pub fn full(&self) -> bool {
        match self {
            Operation::Idle => false,
            Operation::Backup { games, .. } => games.is_none(),
            Operation::Restore { games, .. } => games.is_none(),
            Operation::Cloud { .. } => true,
        }
    }

    pub fn games(&self) -> Option<&GameSelection> {
        match self {
            Operation::Idle => None,
            Operation::Backup { games, .. } => games.as_ref(),
            Operation::Restore { games, .. } => games.as_ref(),
            Operation::Cloud { .. } => None,
        }
    }

    pub fn games_specified(&self) -> bool {
        match self {
            Operation::Idle => false,
            Operation::Backup { games, .. } => games.as_ref().is_some_and(|xs| !xs.is_empty()),
            Operation::Restore { games, .. } => games.as_ref().is_some_and(|xs| !xs.is_empty()),
            Operation::Cloud { .. } => false,
        }
    }

    pub fn flag_cancel(&mut self) {
        match self {
            Operation::Idle => (),
            Operation::Backup { cancelling, .. } => *cancelling = true,
            Operation::Restore { cancelling, .. } => *cancelling = true,
            Operation::Cloud { cancelling, .. } => *cancelling = true,
        }
    }

    pub fn errors(&self) -> Option<&Vec<Error>> {
        match self {
            Operation::Idle => None,
            Operation::Backup { errors, .. } => Some(errors),
            Operation::Restore { errors, .. } => Some(errors),
            Operation::Cloud { errors, .. } => Some(errors),
        }
    }

    pub fn push_error(&mut self, error: Error) {
        match self {
            Operation::Idle => (),
            Operation::Backup { errors, .. } => errors.push(error),
            Operation::Restore { errors, .. } => errors.push(error),
            Operation::Cloud { errors, .. } => errors.push(error),
        }
    }

    pub fn update_integrated_cloud(&mut self, finality: Finality) {
        match self {
            Operation::Idle => (),
            Operation::Backup {
                checking_cloud,
                syncing_cloud,
                ..
            } => match finality {
                Finality::Preview => *checking_cloud = true,
                Finality::Final => *syncing_cloud = true,
            },
            Operation::Restore { checking_cloud, .. } => match finality {
                Finality::Preview => *checking_cloud = true,
                Finality::Final => (),
            },
            Operation::Cloud { .. } => (),
        }
    }

    pub fn transition_from_cloud_step(&mut self, synced: bool) {
        let preview = self.preview();

        match self {
            Operation::Idle => (),
            Operation::Backup {
                checking_cloud,
                syncing_cloud,
                should_sync_cloud_after,
                ..
            } => {
                if *checking_cloud {
                    *checking_cloud = false;
                    *should_sync_cloud_after = synced && !preview;
                    if !synced {
                        self.push_error(Error::CloudConflict);
                    }
                } else if *syncing_cloud {
                    *syncing_cloud = false;
                }
            }
            Operation::Restore { checking_cloud, .. } => {
                if *checking_cloud {
                    *checking_cloud = false;
                }
            }
            Operation::Cloud { .. } => (),
        }
    }

    pub fn is_cloud_active(&self) -> bool {
        match self {
            Operation::Idle => false,
            Operation::Backup {
                checking_cloud,
                syncing_cloud,
                ..
            } => *checking_cloud || *syncing_cloud,
            Operation::Restore { checking_cloud, .. } => *checking_cloud,
            Operation::Cloud { .. } => true,
        }
    }

    pub fn integrated_checking_cloud(&self) -> bool {
        match self {
            Operation::Idle => false,
            Operation::Backup { checking_cloud, .. } => *checking_cloud,
            Operation::Restore { checking_cloud, .. } => *checking_cloud,
            Operation::Cloud { .. } => false,
        }
    }

    pub fn integrated_syncing_cloud(&self) -> bool {
        match self {
            Operation::Idle => false,
            Operation::Backup { syncing_cloud, .. } => *syncing_cloud,
            Operation::Restore { .. } => false,
            Operation::Cloud { .. } => false,
        }
    }

    #[allow(dead_code)]
    pub fn should_sync_cloud_after(&self) -> bool {
        match self {
            Operation::Idle => false,
            Operation::Backup {
                should_sync_cloud_after,
                ..
            } => *should_sync_cloud_after,
            Operation::Restore { .. } => false,
            Operation::Cloud { .. } => false,
        }
    }

    pub fn cloud_changes(&self) -> i64 {
        match self {
            Operation::Idle => 0,
            Operation::Backup { cloud_changes, .. } => *cloud_changes,
            Operation::Restore { cloud_changes, .. } => *cloud_changes,
            Operation::Cloud { cloud_changes, .. } => *cloud_changes,
        }
    }

    pub fn add_cloud_change(&mut self) {
        match self {
            Operation::Idle => (),
            Operation::Backup { cloud_changes, .. } => *cloud_changes += 1,
            Operation::Restore { cloud_changes, .. } => *cloud_changes += 1,
            Operation::Cloud { cloud_changes, .. } => *cloud_changes += 1,
        }
    }

    pub fn should_force_new_full_backups(&mut self) -> bool {
        match self {
            Operation::Idle => false,
            Operation::Backup {
                force_new_full_backup, ..
            } => *force_new_full_backup,
            Operation::Restore { .. } => false,
            Operation::Cloud { .. } => false,
        }
    }

    pub fn set_force_new_full_backups(&mut self, value: bool) {
        match self {
            Operation::Idle => (),
            Operation::Backup {
                force_new_full_backup, ..
            } => *force_new_full_backup = value,
            Operation::Restore { .. } => (),
            Operation::Cloud { .. } => (),
        }
    }

    #[allow(dead_code)]
    pub fn syncable_games(&self) -> Option<&HashSet<String>> {
        match self {
            Operation::Idle => None,
            Operation::Backup { syncable_games, .. } => Some(syncable_games),
            Operation::Restore { .. } => None,
            Operation::Cloud { .. } => None,
        }
    }

    pub fn add_syncable_game(&mut self, title: String) {
        match self {
            Operation::Idle => {}
            Operation::Backup { syncable_games, .. } => {
                syncable_games.insert(title);
            }
            Operation::Restore { .. } => {}
            Operation::Cloud { .. } => {}
        }
    }

    pub fn active_games(&self) -> Option<&HashMap<String, chrono::DateTime<chrono::Utc>>> {
        match self {
            Operation::Idle => None,
            Operation::Backup { active_games, .. } => Some(active_games),
            Operation::Restore { active_games, .. } => Some(active_games),
            Operation::Cloud { .. } => None,
        }
    }

    pub fn add_active_game(&mut self, title: String) {
        match self {
            Operation::Idle | Operation::Cloud { .. } => {}
            Operation::Backup { active_games, .. }
            | Operation::Restore { active_games, .. }
                active_games.insert(title, chrono::Utc::now());
            }
        }
    }

    pub fn remove_active_game(&mut self, title: &str) {
        match self {
            Operation::Idle | Operation::Cloud { .. } => {}
            Operation::Backup { active_games, .. }
            | Operation::Restore { active_games, .. }
                active_games.remove(title);
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Games,
    GameDetail(String),
    ThisDevice,
    AllDevices,
    Backup,
    CustomGames,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseSubject {
    BackupTarget,
    Root(usize),
    AddGamePath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseFileSubject {
    RcloneExecutable,
    RootLutrisDatabase(usize),
    SecondaryManifest(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UndoSubject {
    BackupTarget,
    RootPath(usize),
    RootLutrisDatabase(usize),
    SecondaryManifest(usize),
    RcloneExecutable,
    RcloneArguments,
    CloudRemoteId,
    CloudPath,
    ModalField(ModalInputKind),
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ScrollSubject {
    Backup,
    Restore,
    CustomGames,
    Other,
    Modal,
    GameDetail,
}

impl ScrollSubject {
    pub fn id(&self) -> iced::widget::Id {
        match self {
            Self::Backup => crate::gui::widget::id::backup_scroll(),
            Self::Restore => crate::gui::widget::id::restore_scroll(),
            Self::CustomGames => crate::gui::widget::id::custom_games_scroll(),
            Self::Other => crate::gui::widget::id::other_scroll(),
            Self::Modal => crate::gui::widget::id::modal_scroll(),
            Self::GameDetail => crate::gui::widget::id::game_detail_scroll(),
        }
    }

    pub fn into_widget<'a>(
        self,
        content: impl Into<crate::gui::widget::Element<'a>>,
    ) -> crate::gui::widget::Scrollable<'a> {
        crate::gui::widget::Scrollable::new(content)
            .height(Length::Fill)
            .class(crate::gui::style::Scrollable)
            .id(self.id())
            .on_scroll(move |viewport| Message::Scrolled {
                subject: self,
                position: viewport.absolute_offset(),
            })
    }
}

impl From<Screen> for ScrollSubject {
    fn from(value: Screen) -> Self {
        match value {
            Screen::Backup => Self::Backup,
            Screen::GameDetail(_) => Self::GameDetail,
            Screen::Other | Screen::Games | Screen::ThisDevice | Screen::AllDevices | Screen::CustomGames => Self::Other,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum TreeNodeKey {
    File(String),
    RegistryKey(String),
    RegistryValue(String),
}

impl TreeNodeKey {
    pub fn raw(&self) -> &str {
        match self {
            Self::File(x) => x,
            Self::RegistryKey(x) => x,
            Self::RegistryValue(x) => x,
        }
    }
}
