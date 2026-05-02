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
}

impl AccelaScreen {
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
        }
    }

    fn search_view(&self) -> Element<'_> {
        let header = Container::new(
            Row::new()
                .padding([0, 24])
                .height(52)
                .align_y(Alignment::Center)
                .push(text("ACCELA").size(15).width(Length::Fill)),
        )
        .width(Length::Fill)
        .class(style::Container::TopBar);

        let paths_card = Container::new(
            Column::new()
                .spacing(10)
                .push(text("PATHS").size(13).class(style::Text::Muted))
                .push(
                    text("Set these once. Persistent settings come in a later phase.")
                        .size(12)
                        .class(style::Text::Muted),
                )
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
