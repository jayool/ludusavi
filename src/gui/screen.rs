use std::collections::HashSet;

use iced::{keyboard, padding, Alignment, Length};

use crate::{
    cloud::{Remote, RemoteChoice},
    gui::{
        badge::Badge,
        button,
        common::{BrowseFileSubject, BrowseSubject, Message, Operation, ScrollSubject, UndoSubject},
        editor,
        game_list::GameList,
        icon::Icon,
        search::CustomGamesFilter,
        shortcuts::TextHistories,
        style,
        widget::{
            checkbox, number_input, pick_list, text, Button, Column, Container, Element, IcedParentExt, Row, Space,
        },
    },
    lang::{Language, TRANSLATOR},
    prelude::{AVAILABLE_PARALELLISM, STEAM_DECK},
    resource::{
        cache::Cache,
        config::{self, BackupFormat, CloudFilter, Config, SortKey, Theme, ZipCompression},
        manifest::{Manifest, Store},
    },
    scan::{DuplicateDetector, Duplication, OperationStatus, ScanKind},
};

const RCLONE_URL: &str = "https://rclone.org/downloads";
const RELEASE_URL: &str = "https://github.com/mtkennerly/ludusavi/releases";

fn template(content: Column) -> Element {
    Container::new(content.spacing(15).align_x(Alignment::Center))
        .height(Length::Fill)
        .center_x(Length::Fill)
        .padding(padding::all(5))
        .into()
}

fn make_status_row<'a>(status: &OperationStatus, duplication: Duplication) -> Row<'a> {
    let size = 25;

    Row::new()
        .padding([0, 20])
        .align_y(Alignment::Center)
        .spacing(15)
        .push(text(TRANSLATOR.processed_games(status)).size(size))
        .push_if(status.changed_games.new > 0, || {
            Badge::new_entry_with_count(status.changed_games.new).view()
        })
        .push_if(status.changed_games.different > 0, || {
            Badge::changed_entry_with_count(status.changed_games.different).view()
        })
        .push(text("|").size(size))
        .push(text(TRANSLATOR.processed_bytes(status)).size(size))
        .push_if(!duplication.resolved(), || {
            Badge::new(&TRANSLATOR.badge_duplicates()).view()
        })
}

#[derive(Default)]
pub struct Backup {
    pub log: GameList,
    pub previewed_games: HashSet<String>,
    pub duplicate_detector: DuplicateDetector,
}

impl Backup {
    const SCAN_KIND: ScanKind = ScanKind::Backup;

    pub fn new(config: &Config, cache: &Cache) -> Self {
        Self {
            log: GameList::with_recent_games(Self::SCAN_KIND, config, cache),
            ..Default::default()
        }
    }

    pub fn view(
        &self,
        config: &Config,
        manifest: &Manifest,
        operation: &Operation,
        histories: &TextHistories,
        modifiers: &keyboard::Modifiers,
        daemon_running: bool,
        sync_status: &std::collections::HashMap<String, String>,
    ) -> Element {
        let sort = &config.backup.sort;

        let duplicatees = self.log.duplicatees(&self.duplicate_detector);

        let content = Column::new()
            .push(
                Row::new()
                    .padding([0, 20])
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push(button::backup_preview(operation, self.log.is_filtered()))
                    .push(button::backup(operation, self.log.is_filtered()))
                    .push(button::toggle_all_scanned_games(
                        self.log.all_visible_entries_selected(
                            config,
                            Self::SCAN_KIND,
                            manifest,
                            &self.duplicate_detector,
                            duplicatees.as_ref(),
                        ),
                        self.log.is_filtered(),
                    ))
                    .push(button::filter(self.log.search.show)),
            )
            .push(
                Row::new()
                    .padding([0, 20])
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .push(
                        Container::new(Space::new().width(8).height(8)).class(if daemon_running {
                            style::Container::DaemonDotActive
                        } else {
                            style::Container::DaemonDotInactive
                        }),
                    )
                    .push(text(if daemon_running {
                        "Sync daemon running"
                    } else {
                        "Sync daemon stopped"
                    })),
            )
            .push(make_status_row(
                &self.log.compute_operation_status(
                    config,
                    Self::SCAN_KIND,
                    manifest,
                    &self.duplicate_detector,
                    duplicatees.as_ref(),
                ),
                self.duplicate_detector.overall(),
            ))
            .push(
                Row::new()
                    .padding([0, 20])
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push(text(TRANSLATOR.backup_target_label()))
                    .push(histories.input_small(UndoSubject::BackupTarget))
                    .push(button::choose_folder(BrowseSubject::BackupTarget, modifiers))
                    .push("|")
                    .push(text(TRANSLATOR.sort_label()))
                    .push(
                        pick_list(SortKey::ALL, Some(sort.key), Message::config(config::Event::SortKey))
                            .class(style::PickList::Primary),
                    )
                    .push(button::sort_order(sort.reversed)),
            )
            .push(self.log.view(
                Self::SCAN_KIND,
                config,
                manifest,
                &self.duplicate_detector,
                duplicatees.as_ref(),
                operation,
                histories,
                modifiers,
                sync_status,
            ));

        template(content)
    }
}

#[derive(Default)]
pub struct Restore {
    pub log: GameList,
    pub duplicate_detector: DuplicateDetector,
}

impl Restore {
    const SCAN_KIND: ScanKind = ScanKind::Restore;

    pub fn new(config: &Config, cache: &Cache) -> Self {
        Self {
            log: GameList::with_recent_games(Self::SCAN_KIND, config, cache),
            ..Default::default()
        }
    }

    pub fn view(
        &self,
        config: &Config,
        manifest: &Manifest,
        operation: &Operation,
        histories: &TextHistories,
        modifiers: &keyboard::Modifiers,
        sync_status: &std::collections::HashMap<String, String>,
        daemon_running: bool,
    ) -> Element {
        let sort = &config.restore.sort;

        let duplicatees = self.log.duplicatees(&self.duplicate_detector);

        let content = Column::new()
            .push(
                Row::new()
                    .padding([0, 20])
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push(button::restore_preview(operation, self.log.is_filtered()))
                    .push(button::restore(operation, self.log.is_filtered()))
                    .push(button::toggle_all_scanned_games(
                        self.log.all_visible_entries_selected(
                            config,
                            Self::SCAN_KIND,
                            manifest,
                            &self.duplicate_detector,
                            duplicatees.as_ref(),
                        ),
                        self.log.is_filtered(),
                    ))
                    .push(button::validate_backups(operation))
                    .push(button::filter(self.log.search.show)),
            )
            .push(
                Row::new()
                    .padding([0, 20])
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .push(
                        Container::new(Space::new().width(8).height(8)).class(if daemon_running {
                            style::Container::DaemonDotActive
                        } else {
                            style::Container::DaemonDotInactive
                        }),
                    )
                    .push(text(if daemon_running {
                        "Sync daemon running"
                    } else {
                        "Sync daemon stopped"
                    })),
            )
            .push(make_status_row(
                &self.log.compute_operation_status(
                    config,
                    Self::SCAN_KIND,
                    manifest,
                    &self.duplicate_detector,
                    duplicatees.as_ref(),
                ),
                self.duplicate_detector.overall(),
            ))
            .push(
                Row::new()
                    .padding([0, 20])
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push(text(TRANSLATOR.restore_source_label()))
                    .push(histories.input(UndoSubject::RestoreSource))
                    .push(button::choose_folder(BrowseSubject::RestoreSource, modifiers))
                    .push("|")
                    .push(text(TRANSLATOR.sort_label()))
                    .push(
                        pick_list(SortKey::ALL, Some(sort.key), Message::config(config::Event::SortKey))
                            .class(style::PickList::Primary),
                    )
                    .push(button::sort_order(sort.reversed)),
            )
            .push(self.log.view(
                Self::SCAN_KIND,
                config,
                manifest,
                &self.duplicate_detector,
                duplicatees.as_ref(),
                operation,
                histories,
                modifiers,
                sync_status,
            ));

        template(content)
    }
}

#[derive(Default)]
pub struct CustomGames {
    pub filter: CustomGamesFilter,
}

impl CustomGames {
    pub fn view<'a>(
        &'a self,
        config: &Config,
        manifest: &Manifest,
        operating: bool,
        histories: &'a TextHistories,
        modifiers: &keyboard::Modifiers,
    ) -> Element<'a> {
        let content = Column::new()
            .push(
                Row::new()
                    .padding([0, 20])
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push(button::add_game())
                    .push(button::toggle_all_custom_games(
                        self.all_visible_game_selected(config),
                        self.is_filtered(),
                    ))
                    .push(button::sort(config::Event::SortCustomGames))
                    .push(button::filter(self.filter.enabled)),
            )
            .push(self.filter.view(histories))
            .push(editor::custom_games(
                config,
                manifest,
                operating,
                histories,
                modifiers,
                &self.filter,
            ));

        template(content)
    }

    fn is_filtered(&self) -> bool {
        self.filter.enabled
    }

    pub fn visible_games(&self, config: &Config) -> Vec<usize> {
        config
            .custom_games
            .iter()
            .enumerate()
            .filter_map(|(i, game)| self.filter.qualifies(game).then_some(i))
            .collect()
    }

    fn all_visible_game_selected(&self, config: &Config) -> bool {
        config
            .custom_games
            .iter()
            .filter(|game| self.filter.qualifies(game))
            .all(|x| !x.ignore)
    }
}

pub fn other<'a>(
    updating_manifest: bool,
    config: &'a Config,
    cache: &'a Cache,
    operation: &Operation,
    histories: &'a TextHistories,
    modifiers: &keyboard::Modifiers,
) -> Element<'a> {
    let is_rclone_valid = config.apps.rclone.is_valid();
    let is_cloud_configured = config.cloud.remote.is_some();
    let is_cloud_path_valid = crate::cloud::validate_cloud_path(&config.cloud.path).is_ok();

    let header = Container::new(
        Row::new()
            .padding([0, 24])
            .height(52)
            .align_y(Alignment::Center)
            .push(text("Settings").size(15).width(Length::Fill)),
    )
    .width(Length::Fill)
    .class(style::Container::TopBar);

    // --- SECCIÓN ROOTS ---
    let roots_card = Container::new(
        Column::new()
            .spacing(10)
            .push(text("ROOTS").size(11).class(style::Text::Muted))
            .push(text("Game roots are required to detect save file locations automatically.").size(12).class(style::Text::Muted))
            .push(editor::root(config, histories, modifiers)),
    )
    .width(Length::Fill)
    .padding(16)
    .class(style::Container::GamesTable);

    // --- SECCIÓN MANIFEST ---
    let manifest_card = Container::new(
        Column::new()
            .spacing(10)
            .push(text("MANIFEST").size(11).class(style::Text::Muted))
            .push(text("The manifest contains the list of known games and their save locations.").size(12).class(style::Text::Muted))
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(
                        Button::new(text("Refresh manifest").size(13))
                            .padding([7, 14])
                            .class(style::Button::Ghost)
                            .on_press_maybe((!updating_manifest).then_some(Message::UpdateManifest { force: true }))
                    )
                    .push_if(updating_manifest, || {
                        text("Updating...").size(12).class(style::Text::Muted)
                    }),
            )
            .push(
                editor::manifest(config, cache, histories, modifiers)
                    .padding(padding::top(5))
                    .class(style::Container::Wrapper),
            ),
    )
    .width(Length::Fill)
    .padding(16)
    .class(style::Container::GamesTable);

    // --- SECCIÓN CLOUD / RCLONE ---
    let cloud_card = Container::new(
        Column::new()
            .spacing(10)
            .push(text("CLOUD / RCLONE").size(11).class(style::Text::Muted))
            .push(
                Container::new({
                    let mut column = Column::new().spacing(8).push(
                        Row::new()
                            .spacing(10)
                            .align_y(Alignment::Center)
                            .push(text("rclone executable").size(12).class(style::Text::Muted).width(140))
                            .push(histories.input_small(UndoSubject::RcloneExecutable))
                            .push_if(!is_rclone_valid, || {
                                Icon::Error.text().width(Length::Shrink).class(style::Text::Failure)
                            })
                            .push(button::choose_file(BrowseFileSubject::RcloneExecutable, modifiers))
                            .push(histories.input_small(UndoSubject::RcloneArguments)),
                    );

                    if is_rclone_valid {
                        let choice: RemoteChoice = config.cloud.remote.as_ref().into();
                        column = column
                            .push({
                                let mut row = Row::new()
                                    .spacing(10)
                                    .align_y(Alignment::Center)
                                    .push(text("Remote").size(12).class(style::Text::Muted).width(140))
                                    .push_if(!operation.idle(), || {
                                        text(choice.to_string())
                                            .height(30)
                                            .align_y(iced::alignment::Vertical::Center)
                                    })
                                    .push_if(operation.idle(), || {
                                        pick_list(
                                            RemoteChoice::ALL,
                                            Some(choice),
                                            Message::EditedCloudRemote,
                                        )
                                    });

                                if let Some(Remote::Custom { .. }) = &config.cloud.remote {
                                    row = row
                                        .push(text(TRANSLATOR.remote_name_label()).size(12))
                                        .push(histories.input_small(UndoSubject::CloudRemoteId));
                                }

                                if let Some(description) = config.cloud.remote.as_ref().and_then(|x| x.description()) {
                                    row = row.push(text(description).size(12).class(style::Text::Muted));
                                }

                                row
                            })
                            .push_if(choice != RemoteChoice::None, || {
                                Row::new()
                                    .spacing(10)
                                    .align_y(Alignment::Center)
                                    .push(text("Cloud path").size(12).class(style::Text::Muted).width(140))
                                    .push(histories.input_small(UndoSubject::CloudPath))
                                    .push_if(!is_cloud_path_valid, || {
                                        Icon::Error.text().width(Length::Shrink).class(style::Text::Failure)
                                    })
                            });

                        if !is_cloud_configured {
                            column = column.push(
                                text(TRANSLATOR.cloud_not_configured()).size(12).class(style::Text::Muted)
                            );
                        }
                        if !is_cloud_path_valid {
                            column = column.push(
                                text(TRANSLATOR.prefix_warning(&TRANSLATOR.cloud_path_invalid()))
                                    .size(12)
                                    .class(style::Text::Failure)
                            );
                        }
                    } else {
                        column = column
                            .push(
                                text(TRANSLATOR.prefix_warning(&TRANSLATOR.rclone_unavailable()))
                                    .size(12)
                                    .class(style::Text::Failure),
                            )
                            .push(
                                Button::new(text(TRANSLATOR.get_rclone_button()).size(13))
                                    .padding([7, 14])
                                    .class(style::Button::Ghost)
                                    .on_press(Message::OpenUrl(RCLONE_URL.to_string()))
                            );
                    }

                    column
                })
                .padding(10)
                .class(style::Container::GamesTableRow),
            ),
    )
    .width(Length::Fill)
    .padding(16)
    .class(style::Container::GamesTable);

    // --- SECCIÓN SYNC ---
    let sync_card = Container::new(
        Column::new()
            .spacing(10)
            .push(text("LOCAL").size(11).class(style::Text::Muted))
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(text("Backup path").size(12).class(style::Text::Muted).width(140))
                    .push(histories.input_small(UndoSubject::BackupTarget))
                    .push(button::choose_folder(BrowseSubject::BackupTarget, modifiers)),
            )
            .push(text("Local ZIP backups are stored in this directory.").size(11).class(style::Text::Muted)),
    )
    .width(Length::Fill)
    .padding(16)
    .class(style::Container::GamesTable);

    // --- SECCIÓN DAEMON ---
    #[cfg(target_os = "windows")]
    let install_script = "install-service-windows.ps1";
    #[cfg(not(target_os = "windows"))]
    let install_script = "install-service-linux.sh";

    #[cfg(target_os = "windows")]
    let uninstall_script = "uninstall-service-windows.ps1";
    #[cfg(not(target_os = "windows"))]
    let uninstall_script = "uninstall-service-linux.sh";

    let daemon_card = Container::new(
        Column::new()
            .spacing(10)
            .push(text("DAEMON").size(11).class(style::Text::Muted))
            .push(
                Row::new()
                    .spacing(8)
                    .push(
                        Button::new(text("Install service").size(13))
                            .padding([7, 14])
                            .class(style::Button::Primary)
                            .on_press(Message::OpenDir {
                                path: crate::prelude::StrictPath::new(install_script.to_string()),
                            })
                    )
                    .push(
                        Button::new(text("Uninstall service").size(13))
                            .padding([7, 14])
                            .class(style::Button::Ghost)
                            .on_press(Message::OpenDir {
                                path: crate::prelude::StrictPath::new(uninstall_script.to_string()),
                            })
                    ),
            )
            .push(text("Install or uninstall the sync daemon as a system service.").size(11).class(style::Text::Muted)),
    )
    .width(Length::Fill)
    .padding(16)
    .class(style::Container::GamesTable);

    let content = Column::new()
        .push(header)
        .push(
            Container::new(
                ScrollSubject::Other.into_widget(
                    Column::new()
                        .spacing(16)
                        .padding([24, 24])
                        .push(sync_card)
                        .push(cloud_card)
                        .push(daemon_card)
                        .push(roots_card)
                        .push(manifest_card),
                )
            )
            .width(Length::Fill)
            .height(Length::Fill),
        );

    content.into()
}
