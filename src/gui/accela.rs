//! ACCELA tab — Phase 1.
//!
//! Renders a sidebar tab that talks to the headless ACCELA adapter
//! (see `accela_adapter/`) over a JSON-lines protocol on stdin/stdout.
//!
//! Phase 1 scope: configuration inputs + search + results list.
//! Future phases add fetch_manifest, depot picker, download, post-processing.

use std::path::PathBuf;
use std::process::Stdio;

use iced::{Alignment, Length};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::gui::{
    common::{Message, ScrollSubject},
    style,
    widget::{text, Button, Column, Container, Element, Row, TextInput},
};

#[derive(Debug, Clone)]
pub enum Event {
    AccelaPathChanged(String),
    PythonPathChanged(String),
    QueryChanged(String),
    SubmitSearch,
    SearchSucceeded(Vec<GameResult>),
    SearchFailed(String),
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
    pub manifest_available: bool,
    #[serde(default)]
    pub manifest_size: Option<u64>,
    #[serde(default)]
    pub uploaded_date: Option<String>,
}

#[derive(Default)]
pub struct AccelaScreen {
    pub accela_path: String,
    pub python_path: String,
    pub query: String,
    pub results: Vec<GameResult>,
    pub status: Status,
}

impl AccelaScreen {
    pub fn view(&self) -> Element<'_> {
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

        let header_row = Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(text("AppID").size(11).class(style::Text::Muted).width(80))
            .push(text("Name").size(11).class(style::Text::Muted).width(Length::Fill))
            .push(
                text("Manifest size")
                    .size(11)
                    .class(style::Text::Muted)
                    .width(110),
            )
            .push(
                text("Uploaded")
                    .size(11)
                    .class(style::Text::Muted)
                    .width(110),
            );

        let mut col = Column::new().spacing(4).push(header_row);
        for game in &self.results {
            let size_str = match game.manifest_size {
                Some(bytes) if game.manifest_available => format_size(bytes),
                _ if !game.manifest_available => "no manifest".to_string(),
                _ => "-".to_string(),
            };
            col = col.push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(
                        text(&game.game_id)
                            .size(12)
                            .class(style::Text::Muted)
                            .width(80),
                    )
                    .push(text(&game.game_name).size(12).width(Length::Fill))
                    .push(
                        text(size_str)
                            .size(11)
                            .class(style::Text::Muted)
                            .width(110),
                    )
                    .push(
                        text(game.uploaded_date.clone().unwrap_or_default())
                            .size(11)
                            .class(style::Text::Muted)
                            .width(110),
                    ),
            );
        }
        col.into()
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

/// Spawn the adapter, send a single search command, return parsed results.
///
/// The adapter exits cleanly when stdin is closed, so this is a one-shot
/// process per search. Future phases will keep a long-lived adapter for
/// streaming progress events.
pub async fn run_search(
    python_path: String,
    adapter_path: PathBuf,
    accela_path: String,
    query: String,
) -> Result<Vec<GameResult>, String> {
    let cmd_json = serde_json::json!({"cmd": "search", "query": query}).to_string();

    let mut child = Command::new(&python_path)
        .arg(&adapter_path)
        .arg("--accela-path")
        .arg(&accela_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;

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
    let event: serde_json::Value =
        serde_json::from_str(line).map_err(|e| format!("json parse: {e} (line: {line})"))?;

    match event.get("event").and_then(|v| v.as_str()) {
        Some("search_results") => {
            let games_value = event.get("games").cloned().unwrap_or(serde_json::Value::Null);
            let games: Vec<GameResult> =
                serde_json::from_value(games_value).map_err(|e| format!("results parse: {e}"))?;
            Ok(games)
        }
        Some("error") => Err(event
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error")
            .to_string()),
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
