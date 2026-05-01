use std::collections::{HashMap, HashSet};

use iced::Length;

use crate::{
    cloud::{Remote, RemoteChoice},
    gui::{
        modal::{ModalField, ModalInputKind},
    },
    prelude::{CommandError, EditAction, Error, Finality, StrictPath},
    resource::{
        config::{self, Root},
        manifest::{Manifest, ManifestUpdate},
    },
    scan::{
        game_filter,
        layout::BackupLayout,
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
    FinalizeRemote(Remote),
    EditedModalField(ModalField),
    ShowScanActiveGames,
    CopyText(String),
    OpenRegistry(RegistryItem),
    RequestSyncBackup(String),
    RequestSyncRestore(String),
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
        games: Option<GameSelection>,
        errors: Vec<Error>,
        force_new_full_backup: bool,
        syncable_games: HashSet<String>,
        active_games: HashMap<String, chrono::DateTime<chrono::Utc>>,
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
            games,
            errors: vec![],
            force_new_full_backup: false,
            syncable_games: HashSet::new(),
            active_games: HashMap::new(),
        }
    }

    pub fn preview(&self) -> bool {
        match self {
            Operation::Idle => true,
            Operation::Backup { finality, .. } => finality.preview(),
        }
    }

    pub fn full(&self) -> bool {
        match self {
            Operation::Idle => false,
            Operation::Backup { games, .. } => games.is_none(),
        }
    }

    pub fn games(&self) -> Option<&GameSelection> {
        match self {
            Operation::Idle => None,
            Operation::Backup { games, .. } => games.as_ref(),
        }
    }

    pub fn games_specified(&self) -> bool {
        match self {
            Operation::Idle => false,
            Operation::Backup { games, .. } => games.as_ref().is_some_and(|xs| !xs.is_empty()),
        }
    }

    pub fn flag_cancel(&mut self) {
        match self {
            Operation::Idle => (),
            Operation::Backup { cancelling, .. } => *cancelling = true,
        }
    }

    pub fn errors(&self) -> Option<&Vec<Error>> {
        match self {
            Operation::Idle => None,
            Operation::Backup { errors, .. } => Some(errors),
        }
    }

    pub fn push_error(&mut self, error: Error) {
        match self {
            Operation::Idle => (),
            Operation::Backup { errors, .. } => errors.push(error),
        }
    }

    pub fn should_force_new_full_backups(&mut self) -> bool {
        match self {
            Operation::Idle => false,
            Operation::Backup {
                force_new_full_backup, ..
            } => *force_new_full_backup,
        }
    }

    pub fn set_force_new_full_backups(&mut self, value: bool) {
        match self {
            Operation::Idle => (),
            Operation::Backup {
                force_new_full_backup, ..
            } => *force_new_full_backup = value,
        }
    }

    #[allow(dead_code)]
    pub fn syncable_games(&self) -> Option<&HashSet<String>> {
        match self {
            Operation::Idle => None,
            Operation::Backup { syncable_games, .. } => Some(syncable_games),
        }
    }

    pub fn add_syncable_game(&mut self, title: String) {
        match self {
            Operation::Idle => {}
            Operation::Backup { syncable_games, .. } => {
                syncable_games.insert(title);
            }
        }
    }

    pub fn active_games(&self) -> Option<&HashMap<String, chrono::DateTime<chrono::Utc>>> {
        match self {
            Operation::Idle => None,
            Operation::Backup { active_games, .. } => Some(active_games),
        }
    }

    pub fn add_active_game(&mut self, title: String) {
        match self {
            Operation::Idle => {}
            Operation::Backup { active_games, .. } => {
                active_games.insert(title, chrono::Utc::now());
            }
        }
    }
    pub fn remove_active_game(&mut self, title: &str) {
        match self {
            Operation::Idle => {}
            Operation::Backup { active_games, .. } => {
                active_games.remove(title);
            }
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
    CustomGames,
    Other,
    Modal,
    GameDetail,
}

impl ScrollSubject {
    pub fn id(&self) -> iced::widget::Id {
        match self {
            Self::Backup => crate::gui::widget::id::backup_scroll(),
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
