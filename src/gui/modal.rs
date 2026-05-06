use iced::{
    padding,
    widget::{mouse_area, opaque},
    Alignment, Length,
};
use itertools::Itertools;

use crate::{
    cloud::{Remote, WebDavProvider},
    gui::{
        button,
        common::{Message, Operation, ScrollSubject, UndoSubject},
        shortcuts::TextHistories,
        style,
        widget::{pick_list, text, Column, Container, Element, Row, Space},
    },
    lang::TRANSLATOR,
    prelude::Error,
    resource::config::{Config, Root},
};

pub enum ModalVariant {
    Loading,
    Info,
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModalInputKind {
    Url,
    Host,
    Port,
    Username,
    Password,
}

#[derive(Debug, Clone)]
pub enum ModalField {
    Url(String),
    Host(String),
    Port(String),
    Username(String),
    Password(String),
    WebDavProvider(WebDavProvider),
}

impl ModalField {
    pub fn view<'a>(kind: ModalInputKind, histories: &TextHistories) -> Row<'a> {
        let label = match kind {
            ModalInputKind::Url => TRANSLATOR.url_field(),
            ModalInputKind::Host => TRANSLATOR.host_label(),
            ModalInputKind::Port => TRANSLATOR.port_label(),
            ModalInputKind::Username => TRANSLATOR.username_label(),
            ModalInputKind::Password => TRANSLATOR.password_label(),
        };

        Row::new()
            .align_y(Alignment::Center)
            .spacing(12)
            .push(text(label).size(12).class(style::Text::Muted).width(150))
            .push(histories.input_small(UndoSubject::ModalField(kind)))
    }

    pub fn view_pick_list<'a, T>(label: String, value: &'a T, choices: &'a [T], change: fn(T) -> Self) -> Row<'a>
    where
        T: Copy + Eq + PartialEq + ToString + 'static,
    {
        Row::new()
            .align_y(Alignment::Center)
            .spacing(12)
            .push(text(label).size(12).class(style::Text::Muted).width(150))
            .push(
                Container::new(
                    pick_list(choices, Some(*value), move |x| {
                        Message::EditedModalField(change(x))
                    })
                    .text_size(13)
                    .padding([5, 5]),
                )
                .width(Length::Fill),
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Error,
    Errors,
    Exiting,
    NoMissingRoots,
    ConfirmAddMissingRoots,
    UpdatingManifest,
    ConfigureFtpRemote,
    ConfigureSmbRemote,
    ConfigureWebDavRemote,
    ActiveScanGames,
    ConfirmSyncBackup,
    ConfirmSyncRestore,
    ConfirmSyncModeChange,
    AddGame,
    ConfirmRemoveCustomGame,
    ConfirmRestoreSafetyBackup,
    ConfirmDeleteSafetyBackup,
    ConfirmResolveConflictKeepBoth,
    ConfirmAccelaAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Error {
        variant: Error,
    },
    Errors {
        errors: Vec<Error>,
    },
    Exiting,
    NoMissingRoots,
    ConfirmAddMissingRoots(Vec<Root>),
    UpdatingManifest,
    ConfigureFtpRemote,
    ConfigureSmbRemote,
    ConfigureWebDavRemote {
        provider: WebDavProvider,
    },
    ActiveScanGames,
    ConfirmSyncBackup {
        game: String,
    },
    ConfirmSyncRestore {
        game: String,
    },
    ConfirmRestoreSafetyBackup { 
        game: String 
    },
    ConfirmDeleteSafetyBackup {
        game: String 
    },
    ConfirmResolveConflictKeepBoth {
        game: String,
    },
    ConfirmSyncModeChange {
        game: String,
        warning: String,
        previous_mode: ludusavi::sync::sync_config::SaveMode,
    },
    AddGame {
        name: String,
        path: String,
        error: Option<String>,
    },
    ConfirmRemoveCustomGame {
    game: String,
    },
    /// Confirmation for one of the ACCELA library-mode actions
    /// (Uninstall / Fix Install / Apply Goldberg / Remove Goldberg /
    /// Run Steamless). The flags are captured at modal-open time from
    /// `AccelaScreen.install_remove_*` and stay frozen until confirm.
    ConfirmAccelaAction {
        install: crate::gui::accela::AccelaInstall,
        action: crate::gui::accela::InstallAction,
        remove_compatdata: bool,
        remove_saves: bool,
    },
}

impl Modal {
    pub fn kind(&self) -> Kind {
        match self {
            Modal::Error { .. } => Kind::Error,
            Modal::Errors { .. } => Kind::Errors,
            Modal::Exiting => Kind::Exiting,
            Modal::NoMissingRoots => Kind::NoMissingRoots,
            Modal::ConfirmAddMissingRoots(..) => Kind::ConfirmAddMissingRoots,
            Modal::UpdatingManifest => Kind::UpdatingManifest,
            Modal::ConfigureFtpRemote => Kind::ConfigureFtpRemote,
            Modal::ConfigureSmbRemote => Kind::ConfigureSmbRemote,
            Modal::ConfigureWebDavRemote { .. } => Kind::ConfigureWebDavRemote,
            Modal::ActiveScanGames => Kind::ActiveScanGames,
            Modal::ConfirmSyncBackup { .. } => Kind::ConfirmSyncBackup,
            Modal::ConfirmSyncRestore { .. } => Kind::ConfirmSyncRestore,
            Modal::ConfirmSyncModeChange { .. } => Kind::ConfirmSyncModeChange,
            Modal::AddGame { .. } => Kind::AddGame,
            Modal::ConfirmRemoveCustomGame { .. } => Kind::ConfirmRemoveCustomGame,
            Modal::ConfirmRestoreSafetyBackup { .. } => Kind::ConfirmRestoreSafetyBackup,
            Modal::ConfirmDeleteSafetyBackup { .. } => Kind::ConfirmDeleteSafetyBackup,
            Modal::ConfirmResolveConflictKeepBoth { .. } => Kind::ConfirmResolveConflictKeepBoth,
            Modal::ConfirmAccelaAction { .. } => Kind::ConfirmAccelaAction,
        }
    }

    /// Should we allow two of the same kind back-to-back in the list?
    pub fn stackable(&self) -> bool {
        match self {
            Modal::Error { .. } => true,
            Modal::Errors { .. } => true,
            Modal::Exiting => false,
            Modal::NoMissingRoots => false,
            Modal::ConfirmAddMissingRoots(..) => false,
            Modal::UpdatingManifest => false,
            Modal::ConfigureFtpRemote => false,
            Modal::ConfigureSmbRemote => false,
            Modal::ConfigureWebDavRemote { .. } => false,
            Modal::ActiveScanGames => false,
            Modal::ConfirmSyncBackup { .. } => false,
            Modal::ConfirmSyncRestore { .. } => false,
            Modal::ConfirmSyncModeChange { .. } => false,
            Modal::AddGame { .. } => false,
            Modal::ConfirmRemoveCustomGame { .. } => false,
            Modal::ConfirmRestoreSafetyBackup { .. } => false,
            Modal::ConfirmDeleteSafetyBackup { .. } => false,
            Modal::ConfirmResolveConflictKeepBoth { .. } => false,
            Modal::ConfirmAccelaAction { .. } => false,
        }
    }

    pub fn variant(&self) -> ModalVariant {
        match self {
            Self::Exiting | Self::UpdatingManifest => ModalVariant::Loading,
            Self::Error { .. }
            | Self::Errors { .. }
            | Self::NoMissingRoots
            | Self::ActiveScanGames => ModalVariant::Info,
            Self::ConfirmAddMissingRoots(..)
            | Self::ConfigureFtpRemote { .. }
            | Self::ConfigureSmbRemote { .. }
            | Self::ConfigureWebDavRemote { .. } => ModalVariant::Confirm,
            Self::ConfirmSyncBackup { .. }
            | Self::ConfirmSyncRestore { .. }
            | Self::ConfirmSyncModeChange { .. }
            | Self::AddGame { .. }
            | Self::ConfirmRemoveCustomGame { .. }
            | Self::ConfirmRestoreSafetyBackup { .. }
            | Self::ConfirmDeleteSafetyBackup { .. }
            | Self::ConfirmResolveConflictKeepBoth { .. }
            | Self::ConfirmAccelaAction { .. } => ModalVariant::Confirm,
        }
    }

    pub fn text(&self, _config: &Config) -> String {
        match self {
            Self::Error { variant } => TRANSLATOR.handle_error(variant),
            Self::Errors { errors } => errors.iter().map(|x| TRANSLATOR.handle_error(x)).join("\n\n"),
            Self::Exiting => TRANSLATOR.cancelling_button(),
            Self::NoMissingRoots => TRANSLATOR.no_missing_roots(),
            Self::ConfirmAddMissingRoots(missing) => TRANSLATOR.confirm_add_missing_roots(missing),
            Self::UpdatingManifest => TRANSLATOR.updating_manifest(),
            Self::ConfigureFtpRemote { .. } => "FTP\n\nEnter the host, port, and credentials to connect to the server.".to_string(),
            Self::ConfigureSmbRemote { .. } => "SMB\n\nEnter the host, port, and credentials to connect to the share.".to_string(),
            Self::ConfigureWebDavRemote { .. } => "WebDAV\n\nEnter the server URL and credentials.".to_string(),
            Self::ActiveScanGames => "".to_string(),
            Self::ConfirmSyncBackup { game } => {
                format!("Back up saves for \"{}\"?\n\nThis will overwrite the existing backup with the current save files.", game)
            }
            Self::ConfirmSyncRestore { game } => {
                format!("Restore saves for \"{}\"?\n\nThis will overwrite your current save files with the backup. This cannot be undone.", game)
            }
            Self::ConfirmSyncModeChange { warning, .. } => warning.clone(),
            Self::AddGame { .. } => "Add game".to_string(),
            Self::ConfirmRemoveCustomGame { game } => {
                format!("Remove \"{}\"?\n\nThis will delete the game from the sync system, all backups (local and cloud), and cannot be undone.", game)
            }
            Self::ConfirmRestoreSafetyBackup { game } => {
                format!(
                    "Restore safety backup for \"{}\"?\n\nThe current saves will be replaced with the safety backup. A new safety backup will be created with the current saves before the restore, so you can revert again if needed.",
                    game
                )
            }
            Self::ConfirmDeleteSafetyBackup { game } => {
                format!(
                    "Delete safety backup for \"{}\"?\n\nThe safety backup will be permanently deleted. This cannot be undone.",
                    game
                )
            }
            Self::ConfirmResolveConflictKeepBoth { game } => {
                format!(
                    "Keep both versions of \"{}\"?\n\nYour current local saves will be archived as a permanent snapshot under safety-backups, then the cloud version will be downloaded. You can restore the snapshot later if needed.",
                    game
                )
            }
            Self::ConfirmAccelaAction {
                install,
                action,
                remove_compatdata,
                remove_saves,
            } => {
                use crate::gui::accela::InstallAction;
                // confirm_message returns "title\n\ndescription". The
                // body() function splits on the first \n\n; everything
                // after that is the description, and any further \n\n
                // would render as a literal blank line. So we append
                // extras inline, separated by spaces — keeps the
                // description as a single paragraph like the other
                // destructive modals (ConfirmSyncRestore et al.).
                let mut msg = action.confirm_message(&install.game_name);
                if matches!(action, InstallAction::Uninstall) {
                    let mut extras: Vec<&str> = Vec::new();
                    if *remove_compatdata {
                        extras.push("Proton/Wine compatdata");
                    }
                    if *remove_saves {
                        extras.push("Steam cloud saves");
                    }
                    if !extras.is_empty() {
                        msg.push_str(" Also removing: ");
                        msg.push_str(&extras.join(", "));
                        msg.push('.');
                    }
                    msg.push_str(" This cannot be undone.");
                }
                msg
            }
        }
    }

    pub fn message(&self, histories: &TextHistories) -> Option<Message> {
        match self {
            Self::Error { .. }
            | Self::Errors { .. }
            | Self::NoMissingRoots
            | Self::ActiveScanGames => Some(Message::CloseModal),
            Self::ConfirmSyncBackup { game } => Some(Message::SyncBackupGame(game.clone())),
            Self::ConfirmRestoreSafetyBackup { game } => Some(Message::RestoreSafetyBackup(game.clone())),
            Self::ConfirmDeleteSafetyBackup { game } => Some(Message::DeleteSafetyBackup(game.clone())),
            Self::ConfirmResolveConflictKeepBoth { game } => Some(Message::ResolveConflictKeepBoth(game.clone())),
            Self::ConfirmSyncRestore { game } => Some(Message::SyncRestoreGame(game.clone())),
            Self::ConfirmSyncModeChange { game, previous_mode, .. } => {
                Some(Message::ConfirmSyncModeChange {
                    game: game.clone(),
                    previous_mode: previous_mode.clone(),
                })
            }
            Self::AddGame { .. } => Some(Message::AddGameConfirm),
            Self::ConfirmRemoveCustomGame { game } => Some(Message::RemoveCustomGameConfirm(game.clone())),
            Self::ConfirmAccelaAction {
                install,
                action,
                remove_compatdata,
                remove_saves,
            } => Some(Message::AccelaActionConfirm {
                install: install.clone(),
                action: *action,
                remove_compatdata: *remove_compatdata,
                remove_saves: *remove_saves,
            }),
            Self::Exiting => None,
            Self::ConfirmAddMissingRoots(missing) => Some(Message::ConfirmAddMissingRoots(missing.clone())),
            Self::UpdatingManifest => None,
            Self::ConfigureFtpRemote => {
                let host = histories.modal.host.current();
                let port = histories.modal.port.current();
                let username = histories.modal.username.current();
                let password = histories.modal.password.current();

                let Ok(port) = port.parse::<i32>() else { return None };
                if host.is_empty() || username.is_empty() {
                    None
                } else {
                    Some(Message::FinalizeRemote(Remote::Ftp {
                        id: Remote::generate_id(),
                        host,
                        port,
                        username,
                        password,
                    }))
                }
            }
            Self::ConfigureSmbRemote => {
                let host = histories.modal.host.current();
                let port = histories.modal.port.current();
                let username = histories.modal.username.current();
                let password = histories.modal.password.current();

                let Ok(port) = port.parse::<i32>() else { return None };
                if host.is_empty() || username.is_empty() {
                    None
                } else {
                    Some(Message::FinalizeRemote(Remote::Smb {
                        id: Remote::generate_id(),
                        host,
                        port,
                        username,
                        password,
                    }))
                }
            }
            Self::ConfigureWebDavRemote { provider } => {
                let url = histories.modal.url.current();
                let username = histories.modal.username.current();
                let password = histories.modal.password.current();

                if url.is_empty() || username.is_empty() {
                    None
                } else {
                    Some(Message::FinalizeRemote(Remote::WebDav {
                        id: Remote::generate_id(),
                        url,
                        username,
                        password,
                        provider: *provider,
                    }))
                }
            }
        }
    }

    fn extra_controls(&self) -> Vec<Element> {
        match self {
            Self::Errors { errors } => {
                let has_manifest_error = errors.iter().any(|e| {
                    matches!(e, crate::prelude::Error::ManifestCannotBeUpdated { .. })
                });
                if has_manifest_error {
                    vec![button::primary(
                        "Retry".to_string(),
                        Some(Message::UpdateManifest { force: true }),
                    )]
                } else {
                    vec![]
                }
            }
            Self::Error { .. }
            | Self::Exiting
            | Self::NoMissingRoots
            | Self::ConfirmAddMissingRoots(_)
            | Self::UpdatingManifest
            | Self::ConfigureFtpRemote { .. }
            | Self::ConfigureSmbRemote { .. }
            | Self::ConfigureWebDavRemote { .. }
            | Self::ActiveScanGames
            | Self::ConfirmSyncBackup { .. }
            | Self::ConfirmSyncRestore { .. }
            | Self::ConfirmSyncModeChange { .. }
            | Self::AddGame { .. }
            | Self::ConfirmRemoveCustomGame { .. }
            | Self::ConfirmRestoreSafetyBackup { .. }
            | Self::ConfirmDeleteSafetyBackup { .. }
            | Self::ConfirmResolveConflictKeepBoth { .. }
            | Self::ConfirmAccelaAction { .. } => vec![],
        }
    }

    pub fn body(&self, config: &Config, histories: &TextHistories, operation: &Operation) -> Column {
        // Para los modales de sync usamos layout propio con título + descripción separados
        let is_sync_modal = matches!(self,
            Self::ConfirmSyncBackup { .. }
            | Self::ConfirmSyncRestore { .. }
            | Self::ConfirmSyncModeChange { .. }
            | Self::ConfirmRemoveCustomGame { .. }
            | Self::ConfirmRestoreSafetyBackup { .. }
            | Self::ConfirmDeleteSafetyBackup { .. }
            | Self::ConfirmResolveConflictKeepBoth { .. }
            | Self::ConfirmAccelaAction { .. }
            | Self::ConfigureFtpRemote { .. }
            | Self::ConfigureSmbRemote { .. }
            | Self::ConfigureWebDavRemote { .. }
        );

        let mut col = if is_sync_modal {
            let full = self.text(config);
            let mut parts = full.splitn(2, "\n\n");
            let title = parts.next().unwrap_or("").to_string();
            let description = parts.next().unwrap_or("").to_string();
            Column::new()
                .width(Length::Fill)
                .spacing(10)
                .padding(padding::right(10))
                .align_x(Alignment::Center)
                .push(text(title).size(16))
                .push(text(description).size(13).class(style::Text::Muted))
        } else {
            Column::new()
                .width(Length::Fill)
                .spacing(15)
                .padding(padding::right(10))
                .align_x(Alignment::Center)
                .push(text(self.text(config)))
        };

        match self {
            Self::Error { .. }
            | Self::Errors { .. }
            | Self::Exiting
            | Self::NoMissingRoots
            | Self::ConfirmAddMissingRoots(_)
            | Self::UpdatingManifest
            | Self::ConfirmSyncBackup { .. }
            | Self::ConfirmSyncRestore { .. }
            | Self::ConfirmSyncModeChange { .. }
            | Self::ConfirmRemoveCustomGame { .. }
            | Self::ConfirmRestoreSafetyBackup { .. }
            | Self::ConfirmDeleteSafetyBackup { .. }
            | Self::ConfirmResolveConflictKeepBoth { .. }
            | Self::ConfirmAccelaAction { .. } => (),
            Self::AddGame { name, path, error } => {
                let mut form = Column::new()
                    .spacing(12)
                    .width(500)
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(8)
                            .push(text("Name").size(12).class(style::Text::Muted).width(110))
                            .push(
                                iced::widget::text_input("e.g. Hades", name)
                                    .on_input(Message::AddGameNameChanged)
                                    .padding([5, 5])
                                    .size(13)
                                    .width(Length::Fill)
                            )
                    )
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .spacing(8)
                            .push(text("Save location").size(12).class(style::Text::Muted).width(110))
                            .push(
                                iced::widget::text_input("e.g. C:\\Users\\...\\Hades", path)
                                    .on_input(Message::AddGamePathChanged)
                                    .padding([5, 5])
                                    .size(13)
                                    .width(Length::Fill)
                            )
                            .push(
                                crate::gui::widget::Button::new(text("Browse...").size(12))
                                    .padding([5, 10])
                                    .class(style::Button::Ghost)
                                    .on_press(Message::BrowseDir(crate::gui::common::BrowseSubject::AddGamePath))
                            )
                    );
                if let Some(err) = error {
                    form = form.push(text(err.clone()).size(12).class(style::Text::Muted));
                }
                col = col.push(form);
            }
            Self::ConfigureFtpRemote { .. } | Self::ConfigureSmbRemote { .. } => {
                let form = Column::new()
                    .spacing(12)
                    .width(500)
                    .push(ModalField::view(ModalInputKind::Host, histories))
                    .push(ModalField::view(ModalInputKind::Port, histories))
                    .push(ModalField::view(ModalInputKind::Username, histories))
                    .push(ModalField::view(ModalInputKind::Password, histories));
                col = col.push(form);
            }
            Self::ConfigureWebDavRemote { provider, .. } => {
                let form = Column::new()
                    .spacing(12)
                    .width(500)
                    .push(ModalField::view(ModalInputKind::Url, histories))
                    .push(ModalField::view(ModalInputKind::Username, histories))
                    .push(ModalField::view(ModalInputKind::Password, histories))
                    .push(ModalField::view_pick_list(
                        TRANSLATOR.provider_label(),
                        provider,
                        WebDavProvider::ALL,
                        ModalField::WebDavProvider,
                    ));
                col = col.push(form);
            }
            Self::ActiveScanGames => {
                if let Some(games) = operation.active_games() {
                    let now = chrono::Utc::now();
                    col = games
                        .iter()
                        .sorted_by_key(|(_, v)| *v)
                        .fold(col, |parent, (game, started)| {
                            let elapsed = now - started;
                            let readable = format!(
                                "{:0>2}:{:0>2}:{:0>2}.{:0>3}",
                                elapsed.num_hours(),
                                elapsed.num_minutes() % 60,
                                elapsed.num_seconds() % 60,
                                elapsed.num_milliseconds() % 1000,
                            );
                            parent.push(text(format!("{readable} - {game}")))
                        });
                    col = col.align_x(Alignment::Start).spacing(2);
                }
            }
        }

        col
    }

    fn content(&self, config: &Config, histories: &TextHistories, operation: &Operation) -> Container {
        let positive_button = button::primary(
            match self.variant() {
                ModalVariant::Loading => TRANSLATOR.okay_button(), // dummy
                ModalVariant::Info => TRANSLATOR.okay_button(),
                ModalVariant::Confirm => TRANSLATOR.continue_button(),
            },
            self.message(histories),
        );

        let negative_button = button::negative(TRANSLATOR.cancel_button(), Some(Message::CloseModal));

        Container::new(
            Column::new()
                .width(Length::Fill)
                .push(
                    Container::new(
                        ScrollSubject::Modal.into_widget(self.body(config, histories, operation).padding([0, 15])),
                    )
                    .padding(padding::top(15).right(5))
                    .width(Length::Fill)
                )
                .push(
                    Container::new(
                        match self.variant() {
                            ModalVariant::Loading => Row::new(),
                            ModalVariant::Info => Row::with_children(self.extra_controls()).push(positive_button),
                            ModalVariant::Confirm => Row::with_children(self.extra_controls())
                                .push(positive_button)
                                .push(negative_button),
                        }
                        .padding([20, 0])
                        .spacing(12)
                        .align_y(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .center_x(Length::Fill),
                ),
        )
        .class(style::Container::ModalForeground)
        .center_x(Length::Fill)
        .height(Length::Fill)
    }

    pub fn body_height_portion(&self) -> u16 {
        match self {
            Self::NoMissingRoots => 1,
            Self::Error { .. }
            | Self::Errors { .. }
            | Self::Exiting
            | Self::ConfirmAddMissingRoots(_)
            | Self::UpdatingManifest
            | Self::ConfigureFtpRemote { .. }
            | Self::ConfigureSmbRemote { .. }
            | Self::ConfigureWebDavRemote { .. }
            | Self::ActiveScanGames => 2,
            | Self::ConfirmSyncBackup { .. }
            | Self::ConfirmSyncRestore { .. }
            | Self::ConfirmSyncModeChange { .. }
            | Self::AddGame { .. }
            | Self::ConfirmRemoveCustomGame { .. }
            | Self::ConfirmRestoreSafetyBackup { .. }
            | Self::ConfirmDeleteSafetyBackup { .. }
            | Self::ConfirmResolveConflictKeepBoth { .. }
            | Self::ConfirmAccelaAction { .. } => 1,
        }
    }

    pub fn view(&self, config: &Config, histories: &TextHistories, operation: &Operation) -> Element {
        let horizontal = || {
            Container::new(Space::new().width(Length::FillPortion(1)).height(Length::Fill))
                .class(style::Container::ModalBackground)
        };

        let vertical = || {
            Container::new(Space::new())
                .width(Length::Fill)
                .height(Length::FillPortion(1))
                .class(style::Container::ModalBackground)
        };

        let modal = Container::new(
            Row::new()
                .push(horizontal())
                .push(
                    Column::new()
                        .width(Length::FillPortion(4))
                        .push(vertical())
                        .push(
                            Container::new(opaque(self.content(config, histories, operation)))
                                .class(style::Container::ModalBackground)
                                .width(Length::Fill)
                                .height(match self {
                                    Self::ConfirmSyncBackup { .. }
                                    | Self::ConfirmSyncRestore { .. }
                                    | Self::ConfirmSyncModeChange { .. }
                                    | Self::AddGame { .. }
                                    | Self::ConfirmRemoveCustomGame { .. }
                                    | Self::ConfirmRestoreSafetyBackup { .. }
                                    | Self::ConfirmDeleteSafetyBackup { .. }
                                    | Self::ConfirmResolveConflictKeepBoth { .. }
                                    | Self::ConfirmAccelaAction { .. }
                                    | Self::ConfigureFtpRemote { .. }
                                    | Self::ConfigureSmbRemote { .. }
                                    | Self::ConfigureWebDavRemote { .. }
                                    | Self::UpdatingManifest
                                    | Self::Error { .. }
                                    | Self::Errors { .. }
                                    | Self::NoMissingRoots => Length::Shrink,
                                    _ => Length::FillPortion(self.body_height_portion()),
                                }),
                        )
                        .push(vertical()),
                )
                .push(horizontal()),
        )
        .width(Length::Fill)
        .height(Length::Fill);

        opaque({
            let mut area = mouse_area(modal);

            match self.variant() {
                ModalVariant::Loading => {}
                ModalVariant::Info | ModalVariant::Confirm => {
                    area = area.on_press(Message::CloseModal);
                }
            }

            area
        })
    }
}
