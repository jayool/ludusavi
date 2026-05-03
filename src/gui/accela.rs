//! ACCELA tab — Phase 1.
//!
//! Renders a sidebar tab that talks to the headless ACCELA adapter
//! (see `accela_adapter/`) over a JSON-lines protocol on stdin/stdout.
//!
//! Phase 1 scope: configuration inputs + search + results list.
//! Future phases add fetch_manifest, depot picker, download, post-processing.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use iced::{Alignment, Length};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::gui::{
    common::{Message, ScrollSubject},
    icon::Icon,
    style,
    widget::{text, Button, Column, Container, Element, Row, TextInput},
};

const DOUBLE_CLICK_THRESHOLD: Duration = Duration::from_millis(400);

#[derive(Debug, Clone)]
pub enum Event {
    AccelaPathChanged(String),
    PythonPathChanged(String),
    QueryChanged(String),
    SubmitSearch,
    SearchSucceeded(Vec<GameResult>),
    SearchFailed(String),
    ImageLoaded(String, Result<Vec<u8>, String>),
    ResultClicked(String),
    ManifestFetched(Result<String, String>),
    ZipProcessed(Result<GameDetail, String>),
    BackToSearch,
    FileDropped(PathBuf),
    OpenAccelaPathPicker,
    OpenPythonPathPicker,
    OpenSettings,
    SettingsLoaded(Result<AccelaSettings, String>),
    SwitchSettingsTab(SettingsTab),
    SetSettingBool(String, bool),
    SetSettingString(String, String),
    SetSettingInt(String, i64),
    SetBlockSteamUpdates(bool),
    SaveSettings,
    SettingsBatchSaved(Result<usize, String>),
    ToggleApiKeyVisibility,
    ToggleSgdbKeyVisibility,
    RefreshMorrenusStats,
    StatsLoaded(Result<serde_json::Value, String>),
    RunTool(ToolKind),
    BrowseSteamlessExe,
    ToolFinished(Result<String, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Downloads,
    Integrations,
    Steam,
    Tools,
}

impl SettingsTab {
    fn label(&self) -> &'static str {
        match self {
            Self::Downloads => "Downloads",
            Self::Integrations => "Integrations",
            Self::Steam => "Steam",
            Self::Tools => "Tools",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ToolKind {
    RegisterProtocol,
    UnregisterProtocol,
    RunSlscheevo,
    RunSteamless(PathBuf),
}

impl ToolKind {
    fn key(&self) -> &'static str {
        match self {
            Self::RegisterProtocol => "register_protocol",
            Self::UnregisterProtocol => "unregister_protocol",
            Self::RunSlscheevo => "run_slscheevo",
            Self::RunSteamless(_) => "run_steamless",
        }
    }
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct AccelaSettings {
    pub library_mode: bool,
    pub auto_skip_single_choice: bool,
    pub max_downloads: u32,
    pub generate_achievements: bool,
    pub use_steamless: bool,
    pub auto_apply_goldberg: bool,
    pub create_application_shortcuts: bool,
    pub morrenus_api_key: String,
    pub sgdb_api_key: String,
    pub slssteam_mode: bool,
    pub sls_config_management: bool,
    pub prompt_steam_restart: bool,
    pub block_steam_updates: bool,
}

fn accela_settings_equal(a: &AccelaSettings, b: &AccelaSettings) -> bool {
    a.library_mode == b.library_mode
        && a.auto_skip_single_choice == b.auto_skip_single_choice
        && a.max_downloads == b.max_downloads
        && a.generate_achievements == b.generate_achievements
        && a.use_steamless == b.use_steamless
        && a.auto_apply_goldberg == b.auto_apply_goldberg
        && a.create_application_shortcuts == b.create_application_shortcuts
        && a.morrenus_api_key == b.morrenus_api_key
        && a.sgdb_api_key == b.sgdb_api_key
        && a.slssteam_mode == b.slssteam_mode
        && a.sls_config_management == b.sls_config_management
        && a.prompt_steam_restart == b.prompt_steam_restart
        && a.block_steam_updates == b.block_steam_updates
}

#[derive(Debug, Clone)]
pub enum ImageState {
    Loading,
    Loaded(iced::widget::image::Handle),
    Failed,
}

#[derive(Debug, Clone, Default)]
pub enum ViewState {
    #[default]
    Search,
    Loading(String),
    Depots(GameDetail),
    Settings,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct GameDetail {
    #[serde(default)]
    pub appid: Option<String>,
    #[serde(default)]
    pub game_name: Option<String>,
    #[serde(default)]
    pub depots: BTreeMap<String, DepotInfo>,
    #[serde(default)]
    pub dlcs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct DepotInfo {
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub size: Option<serde_json::Value>,
}

impl DepotInfo {
    pub fn size_display(&self) -> String {
        match &self.size {
            Some(serde_json::Value::String(s)) => s.parse::<u64>().map(format_size).unwrap_or_default(),
            Some(serde_json::Value::Number(n)) => n.as_u64().map(format_size).unwrap_or_default(),
            _ => String::new(),
        }
    }
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Searching,
    Error(String),
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct GameResult {
    pub game_id: String,
    pub game_name: String,
    #[serde(default)]
    pub uploaded_date: Option<String>,
    #[serde(default)]
    pub header_image: Option<String>,
}

#[derive(Default)]
pub struct AccelaScreen {
    pub accela_path: String,
    pub python_path: String,
    pub query: String,
    pub results: Vec<GameResult>,
    pub status: Status,
    pub image_cache: HashMap<String, ImageState>,
    pub view_state: ViewState,
    pub last_click: Option<(String, Instant)>,
    pub settings: Option<AccelaSettings>,
    pub settings_saved: Option<AccelaSettings>,
    pub settings_tab: SettingsTab,
    pub morrenus_stats: Option<serde_json::Value>,
    pub api_key_visible: bool,
    pub sgdb_key_visible: bool,
    pub tool_busy: Option<String>,
    pub tool_message: Option<String>,
}

pub enum SettingValue {
    Bool(bool),
    Int(i64),
    Str(String),
}

/// One pending change ready to ship to the adapter on Save.
#[derive(Debug, Clone)]
pub struct PendingChange {
    pub key: String,
    pub value: serde_json::Value,
    pub side_effect: Option<bool>,
}

impl AccelaScreen {
    pub fn update_setting(&mut self, key: &str, value: SettingValue) {
        let Some(s) = self.settings.as_mut() else {
            return;
        };
        match (key, value) {
            ("library_mode", SettingValue::Bool(b)) => s.library_mode = b,
            ("auto_skip_single_choice", SettingValue::Bool(b)) => s.auto_skip_single_choice = b,
            ("max_downloads", SettingValue::Int(n)) => {
                s.max_downloads = n.clamp(0, 255) as u32;
            }
            ("generate_achievements", SettingValue::Bool(b)) => s.generate_achievements = b,
            ("use_steamless", SettingValue::Bool(b)) => s.use_steamless = b,
            ("auto_apply_goldberg", SettingValue::Bool(b)) => s.auto_apply_goldberg = b,
            ("create_application_shortcuts", SettingValue::Bool(b)) => {
                s.create_application_shortcuts = b
            }
            ("morrenus_api_key", SettingValue::Str(v)) => s.morrenus_api_key = v,
            ("sgdb_api_key", SettingValue::Str(v)) => s.sgdb_api_key = v,
            ("slssteam_mode", SettingValue::Bool(b)) => s.slssteam_mode = b,
            ("sls_config_management", SettingValue::Bool(b)) => s.sls_config_management = b,
            ("prompt_steam_restart", SettingValue::Bool(b)) => s.prompt_steam_restart = b,
            ("block_steam_updates", SettingValue::Bool(b)) => s.block_steam_updates = b,
            _ => {}
        }
    }

    /// True when the working copy of settings differs from the last-saved snapshot.
    pub fn settings_dirty(&self) -> bool {
        match (&self.settings, &self.settings_saved) {
            (Some(cur), Some(saved)) => !accela_settings_equal(cur, saved),
            _ => false,
        }
    }

    /// Compute the list of changed fields to ship on Save.
    pub fn pending_changes(&self) -> Vec<PendingChange> {
        let (cur, saved) = match (&self.settings, &self.settings_saved) {
            (Some(c), Some(s)) => (c, s),
            _ => return Vec::new(),
        };
        let mut out = Vec::new();
        if cur.library_mode != saved.library_mode {
            out.push(PendingChange {
                key: "library_mode".into(),
                value: cur.library_mode.into(),
                side_effect: None,
            });
        }
        if cur.auto_skip_single_choice != saved.auto_skip_single_choice {
            out.push(PendingChange {
                key: "auto_skip_single_choice".into(),
                value: cur.auto_skip_single_choice.into(),
                side_effect: None,
            });
        }
        if cur.max_downloads != saved.max_downloads {
            out.push(PendingChange {
                key: "max_downloads".into(),
                value: serde_json::Value::Number(cur.max_downloads.into()),
                side_effect: None,
            });
        }
        if cur.generate_achievements != saved.generate_achievements {
            out.push(PendingChange {
                key: "generate_achievements".into(),
                value: cur.generate_achievements.into(),
                side_effect: None,
            });
        }
        if cur.use_steamless != saved.use_steamless {
            out.push(PendingChange {
                key: "use_steamless".into(),
                value: cur.use_steamless.into(),
                side_effect: None,
            });
        }
        if cur.auto_apply_goldberg != saved.auto_apply_goldberg {
            out.push(PendingChange {
                key: "auto_apply_goldberg".into(),
                value: cur.auto_apply_goldberg.into(),
                side_effect: None,
            });
        }
        if cur.create_application_shortcuts != saved.create_application_shortcuts {
            out.push(PendingChange {
                key: "create_application_shortcuts".into(),
                value: cur.create_application_shortcuts.into(),
                side_effect: None,
            });
        }
        if cur.morrenus_api_key != saved.morrenus_api_key {
            out.push(PendingChange {
                key: "morrenus_api_key".into(),
                value: serde_json::Value::String(cur.morrenus_api_key.clone()),
                side_effect: None,
            });
        }
        if cur.sgdb_api_key != saved.sgdb_api_key {
            out.push(PendingChange {
                key: "sgdb_api_key".into(),
                value: serde_json::Value::String(cur.sgdb_api_key.clone()),
                side_effect: None,
            });
        }
        if cur.slssteam_mode != saved.slssteam_mode {
            out.push(PendingChange {
                key: "slssteam_mode".into(),
                value: cur.slssteam_mode.into(),
                side_effect: None,
            });
        }
        if cur.sls_config_management != saved.sls_config_management {
            out.push(PendingChange {
                key: "sls_config_management".into(),
                value: cur.sls_config_management.into(),
                side_effect: None,
            });
        }
        if cur.prompt_steam_restart != saved.prompt_steam_restart {
            out.push(PendingChange {
                key: "prompt_steam_restart".into(),
                value: cur.prompt_steam_restart.into(),
                side_effect: None,
            });
        }
        if cur.block_steam_updates != saved.block_steam_updates {
            out.push(PendingChange {
                key: "block_steam_updates".into(),
                value: cur.block_steam_updates.into(),
                side_effect: Some(cur.block_steam_updates),
            });
        }
        out
    }

    pub fn register_click(&mut self, game_id: &str) -> bool {
        let now = Instant::now();
        let is_double = self
            .last_click
            .as_ref()
            .map(|(prev, t)| prev == game_id && now.duration_since(*t) < DOUBLE_CLICK_THRESHOLD)
            .unwrap_or(false);
        if is_double {
            self.last_click = None;
        } else {
            self.last_click = Some((game_id.to_string(), now));
        }
        is_double
    }
}

impl AccelaScreen {
    pub fn view(&self) -> Element<'_> {
        match &self.view_state {
            ViewState::Search => self.search_view(),
            ViewState::Loading(label) => self.loading_view(label),
            ViewState::Depots(detail) => self.depots_view(detail),
            ViewState::Settings => self.settings_view(),
        }
    }

    fn search_view(&self) -> Element<'_> {
        let header = Container::new(
            Row::new()
                .padding([0, 24])
                .height(52)
                .align_y(Alignment::Center)
                .push(text("ACCELA").size(15).width(Length::Fill))
                .push(
                    Button::new(text("⚙ Settings").size(12))
                        .padding([6, 12])
                        .class(style::Button::Ghost)
                        .on_press(Message::Accela(Event::OpenSettings)),
                ),
        )
        .width(Length::Fill)
        .class(style::Container::TopBar);

        let paths_card = Container::new(
            Column::new()
                .spacing(10)
                .push(text("PATHS").size(13).class(style::Text::Muted))
                .push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(
                            text("ACCELA bin")
                                .size(12)
                                .class(style::Text::Muted)
                                .width(140),
                        )
                        .push(
                            TextInput::new(
                                "C:\\path\\to\\ACCELA-...\\bin",
                                &self.accela_path,
                            )
                            .on_input(|s| Message::Accela(Event::AccelaPathChanged(s)))
                            .padding(6)
                            .size(12),
                        )
                        .push(
                            Button::new(Icon::FolderOpen.text_small())
                                .padding(5)
                                .height(25)
                                .width(25)
                                .on_press(Message::Accela(Event::OpenAccelaPathPicker)),
                        ),
                )
                .push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(
                            text("Python")
                                .size(12)
                                .class(style::Text::Muted)
                                .width(140),
                        )
                        .push(
                            TextInput::new(
                                "path to .venv\\Scripts\\python.exe (or your interpreter)",
                                &self.python_path,
                            )
                            .on_input(|s| Message::Accela(Event::PythonPathChanged(s)))
                            .padding(6)
                            .size(12),
                        )
                        .push(
                            Button::new(Icon::FolderOpen.text_small())
                                .padding(5)
                                .height(25)
                                .width(25)
                                .on_press(Message::Accela(Event::OpenPythonPathPicker)),
                        ),
                ),
        )
        .width(Length::Fill)
        .padding(16)
        .class(style::Container::GamesTable);

        let search_enabled = !self.accela_path.trim().is_empty()
            && !self.python_path.trim().is_empty()
            && !self.query.trim().is_empty()
            && self.status != Status::Searching;

        let search_card = Container::new(
            Column::new()
                .spacing(10)
                .push(text("SEARCH").size(13).class(style::Text::Muted))
                .push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(
                            TextInput::new("Game name or AppID", &self.query)
                                .on_input(|s| Message::Accela(Event::QueryChanged(s)))
                                .on_submit(Message::Accela(Event::SubmitSearch))
                                .padding(6)
                                .size(12),
                        )
                        .push(
                            Button::new(text("Search").size(13))
                                .padding([6, 14])
                                .class(if search_enabled {
                                    style::Button::Primary
                                } else {
                                    style::Button::Ghost
                                })
                                .on_press_maybe(
                                    search_enabled.then_some(Message::Accela(Event::SubmitSearch)),
                                ),
                        ),
                )
                .push(self.status_view()),
        )
        .width(Length::Fill)
        .padding(16)
        .class(style::Container::GamesTable);

        let results_card = Container::new(
            Column::new()
                .spacing(10)
                .push(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(text("RESULTS").size(13).class(style::Text::Muted))
                        .push(
                            text(format!("({})", self.results.len()))
                                .size(12)
                                .class(style::Text::Muted),
                        ),
                )
                .push(self.results_view()),
        )
        .width(Length::Fill)
        .padding(16)
        .class(style::Container::GamesTable);

        let content = Column::new().push(header).push(
            Container::new(
                ScrollSubject::Other.into_widget(
                    Column::new()
                        .spacing(16)
                        .padding([24, 24])
                        .push(paths_card)
                        .push(search_card)
                        .push(results_card),
                ),
            )
            .width(Length::Fill)
            .height(Length::Fill),
        );

        content.into()
    }

    fn status_view(&self) -> Element<'_> {
        match &self.status {
            Status::Idle => text("").size(12).into(),
            Status::Searching => text("Searching...")
                .size(12)
                .class(style::Text::Muted)
                .into(),
            Status::Error(msg) => text(format!("Error: {msg}"))
                .size(12)
                .class(style::Text::Failure)
                .into(),
        }
    }

    fn results_view(&self) -> Element<'_> {
        if self.results.is_empty() {
            return text("No results yet.")
                .size(12)
                .class(style::Text::Muted)
                .into();
        }

        const IMG_W: f32 = 130.0;
        const IMG_H: f32 = 60.0;
        const APPID_W: f32 = 80.0;
        const DATE_W: f32 = 110.0;

        let header_row = Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(Container::new(text("")).width(Length::Fixed(IMG_W)))
            .push(
                text("AppID")
                    .size(11)
                    .class(style::Text::Muted)
                    .width(Length::Fixed(APPID_W)),
            )
            .push(text("Name").size(11).class(style::Text::Muted).width(Length::Fill))
            .push(
                text("Uploaded")
                    .size(11)
                    .class(style::Text::Muted)
                    .width(Length::Fixed(DATE_W)),
            );

        let mut col = Column::new().spacing(8).push(header_row);
        for game in &self.results {
            let image_widget: Element = match self.image_cache.get(&game.game_id) {
                Some(ImageState::Loaded(handle)) => iced::widget::image(handle.clone())
                    .width(Length::Fixed(IMG_W))
                    .height(Length::Fixed(IMG_H))
                    .into(),
                _ => Container::new(text(""))
                    .width(Length::Fixed(IMG_W))
                    .height(Length::Fixed(IMG_H))
                    .class(style::Container::GameListEntry)
                    .into(),
            };

            let row = Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(image_widget)
                .push(
                    text(&game.game_id)
                        .size(12)
                        .class(style::Text::Muted)
                        .width(Length::Fixed(APPID_W)),
                )
                .push(text(&game.game_name).size(12).width(Length::Fill))
                .push(
                    text(game.uploaded_date.clone().unwrap_or_default())
                        .size(11)
                        .class(style::Text::Muted)
                        .width(Length::Fixed(DATE_W)),
                );

            let id = game.game_id.clone();
            let card = Container::new(row)
                .padding([8, 10])
                .width(Length::Fill)
                .class(style::Container::GameListEntry);

            let clickable = iced::widget::mouse_area(card)
                .on_press(Message::Accela(Event::ResultClicked(id)));

            col = col.push(clickable);
        }
        col.into()
    }

    fn loading_view<'a>(&'a self, label: &'a str) -> Element<'a> {
        let header = Container::new(
            Row::new()
                .padding([0, 24])
                .height(52)
                .align_y(Alignment::Center)
                .push(text("ACCELA").size(15).width(Length::Fill)),
        )
        .width(Length::Fill)
        .class(style::Container::TopBar);

        let body = Container::new(
            Column::new()
                .spacing(12)
                .padding([24, 24])
                .push(
                    Button::new(text("← Back to results").size(12))
                        .padding([6, 12])
                        .class(style::Button::Ghost)
                        .on_press(Message::Accela(Event::BackToSearch)),
                )
                .push(text(label).size(13).class(style::Text::Muted)),
        )
        .width(Length::Fill)
        .height(Length::Fill);

        Column::new().push(header).push(body).into()
    }

    fn depots_view<'a>(&'a self, detail: &'a GameDetail) -> Element<'a> {
        let header = Container::new(
            Row::new()
                .padding([0, 24])
                .height(52)
                .align_y(Alignment::Center)
                .push(text("ACCELA").size(15).width(Length::Fill)),
        )
        .width(Length::Fill)
        .class(style::Container::TopBar);

        let game_label = format!(
            "{} ({})",
            detail.game_name.as_deref().unwrap_or("Unknown"),
            detail.appid.as_deref().unwrap_or("?")
        );

        let toolbar = Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(
                Button::new(text("← Back to results").size(12))
                    .padding([6, 12])
                    .class(style::Button::Ghost)
                    .on_press(Message::Accela(Event::BackToSearch)),
            )
            .push(text(game_label).size(14).width(Length::Fill));

        let depots_card = if detail.depots.is_empty() {
            Container::new(
                Column::new()
                    .spacing(6)
                    .push(text("DEPOTS").size(13).class(style::Text::Muted))
                    .push(
                        text("No depots found in this manifest.")
                            .size(12)
                            .class(style::Text::Muted),
                    ),
            )
        } else {
            let mut col = Column::new()
                .spacing(6)
                .push(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(text("DEPOTS").size(13).class(style::Text::Muted))
                        .push(
                            text(format!("({})", detail.depots.len()))
                                .size(12)
                                .class(style::Text::Muted),
                        ),
                );

            for (depot_id, info) in &detail.depots {
                let size = info.size_display();
                col = col.push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(
                            text(depot_id)
                                .size(12)
                                .class(style::Text::Muted)
                                .width(Length::Fixed(80.0)),
                        )
                        .push(text(&info.desc).size(12).width(Length::Fill))
                        .push(
                            text(size)
                                .size(11)
                                .class(style::Text::Muted)
                                .width(Length::Fixed(110.0)),
                        ),
                );
            }
            Container::new(col)
        }
        .width(Length::Fill)
        .padding(16)
        .class(style::Container::GamesTable);

        let dlcs_card = if detail.dlcs.is_empty() {
            None
        } else {
            let mut col = Column::new().spacing(6).push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(text("DLCS").size(13).class(style::Text::Muted))
                    .push(
                        text(format!("({})", detail.dlcs.len()))
                            .size(12)
                            .class(style::Text::Muted),
                    ),
            );
            for (dlc_id, dlc_desc) in &detail.dlcs {
                col = col.push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(
                            text(dlc_id)
                                .size(12)
                                .class(style::Text::Muted)
                                .width(Length::Fixed(80.0)),
                        )
                        .push(text(dlc_desc).size(12).width(Length::Fill)),
                );
            }
            Some(
                Container::new(col)
                    .width(Length::Fill)
                    .padding(16)
                    .class(style::Container::GamesTable),
            )
        };

        let mut content_col = Column::new()
            .spacing(16)
            .padding([24, 24])
            .push(toolbar)
            .push(depots_card);
        if let Some(dlcs) = dlcs_card {
            content_col = content_col.push(dlcs);
        }

        Column::new()
            .push(header)
            .push(
                Container::new(ScrollSubject::Other.into_widget(content_col))
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .into()
    }

    fn settings_view(&self) -> Element<'_> {
        let header = Container::new(
            Row::new()
                .padding([0, 24])
                .height(52)
                .align_y(Alignment::Center)
                .push(text("ACCELA — Settings").size(15).width(Length::Fill)),
        )
        .width(Length::Fill)
        .class(style::Container::TopBar);

        let dirty = self.settings_dirty();
        let mut toolbar = Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(
                Button::new(text("← Back to results").size(12))
                    .padding([6, 12])
                    .class(style::Button::Ghost)
                    .on_press(Message::Accela(Event::BackToSearch)),
            )
            .push(iced::widget::Space::new().width(Length::Fill));
        if let Some(msg) = &self.tool_message {
            toolbar = toolbar.push(text(msg.clone()).size(11).class(style::Text::Muted));
        }
        toolbar = toolbar.push(
            Button::new(text("Save").size(12))
                .padding([6, 14])
                .class(if dirty {
                    style::Button::Primary
                } else {
                    style::Button::Ghost
                })
                .on_press_maybe(dirty.then_some(Message::Accela(Event::SaveSettings))),
        );

        let mut tabs_row = Row::new().spacing(4);
        for tab in [
            SettingsTab::Downloads,
            SettingsTab::Integrations,
            SettingsTab::Steam,
            SettingsTab::Tools,
        ] {
            let active = self.settings_tab == tab;
            tabs_row = tabs_row.push(
                Button::new(text(tab.label()).size(12))
                    .padding([6, 14])
                    .class(if active {
                        style::Button::Primary
                    } else {
                        style::Button::Ghost
                    })
                    .on_press(Message::Accela(Event::SwitchSettingsTab(tab))),
            );
        }

        let body: Element = match self.settings.as_ref() {
            None => text("Loading settings...")
                .size(12)
                .class(style::Text::Muted)
                .into(),
            Some(s) => match self.settings_tab {
                SettingsTab::Downloads => self.downloads_tab(s),
                SettingsTab::Integrations => self.integrations_tab(s),
                SettingsTab::Steam => self.steam_tab(s),
                SettingsTab::Tools => self.tools_tab(),
            },
        };

        let content = Column::new()
            .spacing(16)
            .padding([24, 24])
            .push(toolbar)
            .push(tabs_row)
            .push(body);

        Column::new()
            .push(header)
            .push(
                Container::new(ScrollSubject::Other.into_widget(content))
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .into()
    }

    fn downloads_tab<'a>(&'a self, s: &'a AccelaSettings) -> Element<'a> {
        let mut col = Column::new()
            .spacing(12)
            .push(text("DOWNLOAD SETTINGS").size(13).class(style::Text::Muted))
            .push(toggle_row(
                "Limit Downloads to Steam Libraries",
                "library_mode",
                s.library_mode,
                "Detect Steam libraries and let you choose where to install games.",
            ))
            .push(toggle_row(
                "Skip single-choice selection",
                "auto_skip_single_choice",
                s.auto_skip_single_choice,
                "Automatically skip selection when only one option exists.",
            ))
            .push(int_input_row(
                "Maximum concurrent downloads",
                "max_downloads",
                s.max_downloads,
                "Set maximum concurrent downloads (0-255).",
            ))
            .push(text("POST-PROCESSING").size(13).class(style::Text::Muted))
            .push(toggle_row(
                "Generate Steam Achievements",
                "generate_achievements",
                s.generate_achievements,
                "Generate achievement files for your games after downloads.",
            ))
            .push(toggle_row(
                "Remove Steam DRM with Steamless",
                "use_steamless",
                s.use_steamless,
                "Remove DRM from game executables after downloading.",
            ))
            .push(toggle_row(
                "Apply Goldberg Automatically",
                "auto_apply_goldberg",
                s.auto_apply_goldberg,
                "Automatically apply Goldberg after downloads.",
            ));

        if cfg!(target_os = "linux") {
            col = col.push(toggle_row(
                "Create Application Shortcuts (Linux only)",
                "create_application_shortcuts",
                s.create_application_shortcuts,
                "Create desktop shortcuts and install icons from SteamGridDB.",
            ));
        }

        Container::new(col)
            .width(Length::Fill)
            .padding(16)
            .class(style::Container::GamesTable)
            .into()
    }

    fn integrations_tab<'a>(&'a self, s: &'a AccelaSettings) -> Element<'a> {
        let api_row = api_key_row(
            "Morrenus API Key",
            "morrenus_api_key",
            &s.morrenus_api_key,
            self.api_key_visible,
            Message::Accela(Event::ToggleApiKeyVisibility),
        );

        let mut col = Column::new()
            .spacing(12)
            .push(text("API KEYS").size(13).class(style::Text::Muted))
            .push(api_row);

        if cfg!(target_os = "linux") {
            col = col.push(api_key_row(
                "SteamGridDB API Key (Linux only)",
                "sgdb_api_key",
                &s.sgdb_api_key,
                self.sgdb_key_visible,
                Message::Accela(Event::ToggleSgdbKeyVisibility),
            ));
        }

        col = col.push(text("MORRENUS STATS").size(13).class(style::Text::Muted));
        col = col.push(self.stats_view());
        col = col.push(
            Button::new(text("Refresh").size(12))
                .padding([6, 14])
                .class(style::Button::Ghost)
                .on_press(Message::Accela(Event::RefreshMorrenusStats)),
        );

        Container::new(col)
            .width(Length::Fill)
            .padding(16)
            .class(style::Container::GamesTable)
            .into()
    }

    fn stats_view(&self) -> Element<'_> {
        match self.morrenus_stats.as_ref() {
            None => text("Click Refresh to load stats.")
                .size(12)
                .class(style::Text::Muted)
                .into(),
            Some(stats) => {
                let user = stats
                    .get("username")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                let daily_used = stats.get("daily_usage").and_then(|v| v.as_u64()).unwrap_or(0);
                let daily_limit = stats
                    .get("daily_limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let total = stats
                    .get("api_key_usage_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let expires = stats
                    .get("api_key_expires_at")
                    .and_then(|v| v.as_str())
                    .map(|s| s.split('T').next().unwrap_or(s).to_string())
                    .unwrap_or_else(|| "Never".to_string());
                let active = stats
                    .get("can_make_requests")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Column::new()
                    .spacing(4)
                    .push(text(format!("User: {user}")).size(12))
                    .push(text(format!("Daily: {daily_used}/{daily_limit}")).size(12))
                    .push(text(format!("Total calls: {total}")).size(12))
                    .push(text(format!("Expires: {expires}")).size(12))
                    .push(
                        text(format!("Status: {}", if active { "Active" } else { "Blocked" }))
                            .size(12)
                            .class(if active {
                                style::Text::Green
                            } else {
                                style::Text::Failure
                            }),
                    )
                    .into()
            }
        }
    }

    fn steam_tab<'a>(&'a self, s: &'a AccelaSettings) -> Element<'a> {
        let mut col = Column::new()
            .spacing(10)
            .push(
                text("STEAM INTEGRATION")
                    .size(13)
                    .class(style::Text::Muted),
            );

        if cfg!(target_os = "windows") {
            col = col.push(toggle_row(
                "GreenLuma Wrapper Mode",
                "slssteam_mode",
                s.slssteam_mode,
                "Integrate games with Steam using GreenLuma. Games appear in your Steam library automatically.",
            ));
        } else {
            col = col.push(
                text("SLSsteam is enabled automatically for Steam library installs on Linux.")
                    .size(12)
                    .class(style::Text::Muted),
            );
        }

        col = col
            .push(toggle_row(
                "SLSsteam / GreenLuma Config Management",
                "sls_config_management",
                s.sls_config_management,
                "Allow ACCELA to manage SLSsteam/GreenLuma configuration files.",
            ))
            .push(text("STEAM SETTINGS").size(13).class(style::Text::Muted))
            .push(toggle_row(
                "Prompt Steam Restart",
                "prompt_steam_restart",
                s.prompt_steam_restart,
                "Show prompt to restart Steam after Steam-integrated downloads.",
            ))
            .push({
                Column::new()
                    .spacing(2)
                    .push(crate::gui::widget::checkbox(
                        "Block Steam Updates",
                        s.block_steam_updates,
                        |b| Message::Accela(Event::SetBlockSteamUpdates(b)),
                    ))
                    .push(
                        Row::new()
                            .push(iced::widget::Space::new().width(Length::Fixed(20.0)))
                            .push(
                                text(
                                    "Prevent Steam from automatically updating itself (writes steam.cfg in the Steam install folder).",
                                )
                                .size(11)
                                .class(style::Text::Muted),
                            ),
                    )
            });

        Container::new(col)
            .width(Length::Fill)
            .padding(16)
            .class(style::Container::GamesTable)
            .into()
    }

    fn tools_tab(&self) -> Element<'_> {
        let busy = self.tool_busy.is_some();
        let busy_label = self
            .tool_busy
            .clone()
            .map(|k| format!("Running: {k}..."))
            .unwrap_or_default();

        let mut col = Column::new()
            .spacing(12)
            .push(text("TOOLS").size(13).class(style::Text::Muted));

        if !busy_label.is_empty() {
            col = col.push(
                text(busy_label)
                    .size(12)
                    .class(style::Text::Muted),
            );
        }

        col = col.push(tool_button(
            "Configure Achievements",
            "Launch SLScheevo to setup achievement credentials.",
            (!busy).then(|| Message::Accela(Event::RunTool(ToolKind::RunSlscheevo))),
        ));

        col = col.push(tool_button(
            "Remove DRM (Steamless)",
            "Pick an .exe and run Steamless on it.",
            (!busy).then_some(Message::Accela(Event::BrowseSteamlessExe)),
        ));

        col = col.push(tool_button(
            "Open SLSsteam installer (web)",
            "Open the recommended SLSsteam installer page on GitHub.",
            Some(Message::OpenUrl(
                "https://github.com/Deadboy666/h3adcr-b?tab=readme-ov-file#headcrab".to_string(),
            )),
        ));

        if cfg!(target_os = "windows") {
            col = col
                .push(text("WINDOWS REGISTRY").size(13).class(style::Text::Muted))
                .push(tool_button(
                    "Register Registry Entries",
                    "Register accela:// URL protocol and .zip context menu entries.",
                    (!busy).then(|| Message::Accela(Event::RunTool(ToolKind::RegisterProtocol))),
                ))
                .push(tool_button(
                    "Remove Registry Entries",
                    "Remove accela:// URL protocol and .zip context menu entries.",
                    (!busy).then(|| Message::Accela(Event::RunTool(ToolKind::UnregisterProtocol))),
                ));
        }

        Container::new(col)
            .width(Length::Fill)
            .padding(16)
            .class(style::Container::GamesTable)
            .into()
    }
}

fn toggle_row<'a>(
    label: &'a str,
    key: &'static str,
    value: bool,
    tooltip: &'a str,
) -> Element<'a> {
    let cb = crate::gui::widget::checkbox(label, value, move |b| {
        Message::Accela(Event::SetSettingBool(key.to_string(), b))
    });
    Column::new()
        .spacing(2)
        .push(cb)
        .push(
            Row::new()
                .push(iced::widget::Space::new().width(Length::Fixed(20.0)))
                .push(text(tooltip).size(11).class(style::Text::Muted)),
        )
        .into()
}

fn int_input_row<'a>(
    label: &'a str,
    key: &'static str,
    value: u32,
    tooltip: &'a str,
) -> Element<'a> {
    Column::new()
        .spacing(2)
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(text(label).size(12).width(Length::Fill))
                .push(
                    TextInput::new("", &value.to_string())
                        .on_input(move |s| {
                            let parsed: i64 = s.parse().unwrap_or(0);
                            Message::Accela(Event::SetSettingInt(key.to_string(), parsed))
                        })
                        .padding(6)
                        .size(12)
                        .width(Length::Fixed(80.0)),
                ),
        )
        .push(text(tooltip).size(11).class(style::Text::Muted))
        .into()
}

fn api_key_row<'a>(
    label: &'a str,
    key: &'static str,
    value: &'a str,
    visible: bool,
    toggle_msg: Message,
) -> Element<'a> {
    let display: String = if visible {
        value.to_string()
    } else {
        "*".repeat(value.len().min(32))
    };
    Column::new()
        .spacing(4)
        .push(text(label).size(12).class(style::Text::Muted))
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(
                    TextInput::new("Paste your API key", &display)
                        .on_input(move |s| {
                            Message::Accela(Event::SetSettingString(key.to_string(), s))
                        })
                        .padding(6)
                        .size(12),
                )
                .push(
                    Button::new(text(if visible { "Hide" } else { "Show" }).size(11))
                        .padding([6, 10])
                        .class(style::Button::Ghost)
                        .on_press(toggle_msg),
                ),
        )
        .into()
}

fn tool_button<'a>(label: &'a str, tooltip: &'a str, action: Option<Message>) -> Element<'a> {
    Column::new()
        .spacing(2)
        .push(
            Button::new(text(label).size(12))
                .padding([7, 14])
                .class(if action.is_some() {
                    style::Button::Primary
                } else {
                    style::Button::Ghost
                })
                .on_press_maybe(action),
        )
        .push(text(tooltip).size(11).class(style::Text::Muted))
        .into()
}

/// Send one command to a freshly-spawned adapter and return the first event
/// it emits (parsed as JSON). The adapter exits cleanly when stdin is closed.
async fn send_command(
    python_path: &str,
    adapter_path: &PathBuf,
    accela_path: &str,
    cmd_json: String,
) -> Result<serde_json::Value, String> {
    let mut cmd = Command::new(python_path);
    cmd.arg(adapter_path)
        .arg("--accela-path")
        .arg(accela_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // CREATE_NO_WINDOW = 0x08000000. Hide the console window that Windows
    // would otherwise pop up for every spawned python process.
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000);

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "stdin pipe missing".to_string())?;
    stdin
        .write_all(format!("{cmd_json}\n").as_bytes())
        .await
        .map_err(|e| format!("stdin write: {e}"))?;
    drop(stdin);

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("wait_with_output: {e}"))?;

    if !output.status.success() && output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("adapter exited with {}: {stderr}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .next()
        .ok_or_else(|| "no output from adapter".to_string())?;

    serde_json::from_str(line).map_err(|e| format!("json parse: {e} (line: {line})"))
}

fn extract_error(event: &serde_json::Value) -> String {
    event
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown error")
        .to_string()
}

pub async fn run_search(
    python_path: String,
    adapter_path: PathBuf,
    accela_path: String,
    query: String,
) -> Result<Vec<GameResult>, String> {
    let cmd_json = serde_json::json!({"cmd": "search", "query": query}).to_string();
    let event = send_command(&python_path, &adapter_path, &accela_path, cmd_json).await?;

    match event.get("event").and_then(|v| v.as_str()) {
        Some("search_results") => {
            let games_value = event.get("games").cloned().unwrap_or(serde_json::Value::Null);
            serde_json::from_value(games_value).map_err(|e| format!("results parse: {e}"))
        }
        Some("error") => Err(extract_error(&event)),
        other => Err(format!("unexpected event: {other:?}")),
    }
}

pub async fn run_fetch_manifest(
    python_path: String,
    adapter_path: PathBuf,
    accela_path: String,
    appid: String,
) -> Result<String, String> {
    let cmd_json = serde_json::json!({"cmd": "fetch_manifest", "appid": appid}).to_string();
    let event = send_command(&python_path, &adapter_path, &accela_path, cmd_json).await?;

    match event.get("event").and_then(|v| v.as_str()) {
        Some("manifest_ready") => event
            .get("zip")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| "manifest_ready missing 'zip' field".to_string()),
        Some("error") => Err(extract_error(&event)),
        other => Err(format!("unexpected event: {other:?}")),
    }
}

pub async fn run_process_zip(
    python_path: String,
    adapter_path: PathBuf,
    accela_path: String,
    zip_path: String,
) -> Result<GameDetail, String> {
    let cmd_json = serde_json::json!({"cmd": "process_zip", "path": zip_path}).to_string();
    let event = send_command(&python_path, &adapter_path, &accela_path, cmd_json).await?;

    match event.get("event").and_then(|v| v.as_str()) {
        Some("depots_parsed") => {
            serde_json::from_value(event).map_err(|e| format!("depots_parsed parse: {e}"))
        }
        Some("error") => Err(extract_error(&event)),
        other => Err(format!("unexpected event: {other:?}")),
    }
}

/// Resolve the adapter script path relative to the running binary.
///
/// In dev (cargo run from repo root), it lives at `accela_adapter/adapter.py`.
/// In a real install, the user will configure this in settings (Phase 7).
pub fn default_adapter_path() -> PathBuf {
    PathBuf::from("accela_adapter").join("adapter.py")
}

/// Fetch a header image over HTTPS. Returns the raw bytes for use with
/// `iced::widget::image::Handle::from_bytes`.
pub async fn fetch_image(url: String) -> Result<Vec<u8>, String> {
    let response = reqwest::get(&url).await.map_err(|e| e.to_string())?;
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    Ok(bytes.to_vec())
}

pub async fn run_get_settings(
    python_path: String,
    adapter_path: PathBuf,
    accela_path: String,
) -> Result<AccelaSettings, String> {
    let cmd_json = serde_json::json!({"cmd": "get_settings"}).to_string();
    let event = send_command(&python_path, &adapter_path, &accela_path, cmd_json).await?;
    match event.get("event").and_then(|v| v.as_str()) {
        Some("settings") => {
            let values = event
                .get("values")
                .cloned()
                .ok_or_else(|| "settings event missing 'values'".to_string())?;
            serde_json::from_value(values).map_err(|e| format!("settings parse: {e}"))
        }
        Some("error") => Err(extract_error(&event)),
        other => Err(format!("unexpected event: {other:?}")),
    }
}

pub async fn run_set_setting(
    python_path: String,
    adapter_path: PathBuf,
    accela_path: String,
    key: String,
    value: serde_json::Value,
) -> Result<(), String> {
    let cmd_json = serde_json::json!({"cmd": "set_setting", "key": key, "value": value}).to_string();
    let event = send_command(&python_path, &adapter_path, &accela_path, cmd_json).await?;
    match event.get("event").and_then(|v| v.as_str()) {
        Some("setting_saved") => Ok(()),
        Some("error") => Err(extract_error(&event)),
        other => Err(format!("unexpected event: {other:?}")),
    }
}

pub async fn run_get_morrenus_stats(
    python_path: String,
    adapter_path: PathBuf,
    accela_path: String,
) -> Result<serde_json::Value, String> {
    let cmd_json = serde_json::json!({"cmd": "get_morrenus_stats"}).to_string();
    let event = send_command(&python_path, &adapter_path, &accela_path, cmd_json).await?;
    match event.get("event").and_then(|v| v.as_str()) {
        Some("morrenus_stats") => Ok(event.get("stats").cloned().unwrap_or(serde_json::Value::Null)),
        Some("error") => Err(extract_error(&event)),
        other => Err(format!("unexpected event: {other:?}")),
    }
}

pub async fn run_apply_steam_updates_block(
    python_path: String,
    adapter_path: PathBuf,
    accela_path: String,
    enabled: bool,
) -> Result<String, String> {
    let cmd_json =
        serde_json::json!({"cmd": "apply_steam_updates_block", "enabled": enabled}).to_string();
    let event = send_command(&python_path, &adapter_path, &accela_path, cmd_json).await?;
    match event.get("event").and_then(|v| v.as_str()) {
        Some("tool_done") => Ok(event
            .get("note")
            .and_then(|v| v.as_str())
            .unwrap_or("done")
            .to_string()),
        Some("error") => Err(extract_error(&event)),
        other => Err(format!("unexpected event: {other:?}")),
    }
}

pub async fn run_tool(
    python_path: String,
    adapter_path: PathBuf,
    accela_path: String,
    tool: ToolKind,
) -> Result<String, String> {
    let mut payload = serde_json::json!({"cmd": "run_tool", "kind": tool.key()});
    if let ToolKind::RunSteamless(path) = &tool {
        payload["exe_path"] = serde_json::Value::String(path.to_string_lossy().into_owned());
    }
    let event = send_command(&python_path, &adapter_path, &accela_path, payload.to_string()).await?;
    match event.get("event").and_then(|v| v.as_str()) {
        Some("tool_done") => Ok(event
            .get("note")
            .and_then(|v| v.as_str())
            .unwrap_or("done")
            .to_string()),
        Some("error") => Err(extract_error(&event)),
        other => Err(format!("unexpected event: {other:?}")),
    }
}
