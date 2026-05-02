// Iced has built-in support for some keyboard shortcuts. This module provides
// support for implementing other shortcuts until Iced provides its own support.

use std::collections::VecDeque;

use iced::Length;

use crate::{
    cloud::Remote,
    gui::{
        common::{Message, UndoSubject},
        modal::{ModalField, ModalInputKind},
        style,
        widget::{Element, TextInput, Undoable},
    },
    prelude::{EditAction, StrictPath},
    resource::config::{self, Config, CustomGame},
};

pub enum Shortcut {
    Undo,
    Redo,
}

impl Shortcut {
    pub fn apply_to_strict_path_field(&self, config: &mut StrictPath, history: &mut TextHistory) {
        match self {
            Shortcut::Undo => {
                config.reset(history.undo());
            }
            Shortcut::Redo => {
                config.reset(history.redo());
            }
        }
    }

    pub fn apply_to_option_strict_path_field(&self, config: &mut Option<StrictPath>, history: &mut TextHistory) {
        let value = match self {
            Shortcut::Undo => history.undo(),
            Shortcut::Redo => history.redo(),
        };

        if value.is_empty() {
            *config = None;
        } else {
            match config {
                Some(config) => config.reset(value),
                None => *config = Some(value.into()),
            }
        }
    }

    pub fn apply_to_string_field(&self, config: &mut String, history: &mut TextHistory) {
        match self {
            Shortcut::Undo => {
                *config = history.undo();
            }
            Shortcut::Redo => {
                *config = history.redo();
            }
        }
    }
}

impl From<crate::gui::undoable::Action> for Shortcut {
    fn from(source: crate::gui::undoable::Action) -> Self {
        match source {
            crate::gui::undoable::Action::Undo => Self::Undo,
            crate::gui::undoable::Action::Redo => Self::Redo,
        }
    }
}

pub struct TextHistory {
    history: VecDeque<String>,
    limit: usize,
    position: usize,
}

impl Default for TextHistory {
    fn default() -> Self {
        Self::new("", 100)
    }
}

impl TextHistory {
    pub fn new(initial: &str, limit: usize) -> Self {
        let mut history = VecDeque::<String>::new();
        history.push_back(initial.to_string());
        Self {
            history,
            limit,
            position: 0,
        }
    }

    pub fn raw(initial: &str) -> Self {
        let mut history = VecDeque::<String>::new();
        history.push_back(initial.to_string());
        Self {
            history,
            limit: 100,
            position: 0,
        }
    }

    pub fn path(initial: &StrictPath) -> Self {
        let mut history = VecDeque::<String>::new();
        history.push_back(initial.raw().into());
        Self {
            history,
            limit: 100,
            position: 0,
        }
    }

    pub fn push(&mut self, text: &str) {
        if self.current() == text {
            return;
        }
        if self.position + 1 < self.history.len() {
            self.history.truncate(self.position + 1);
        }
        if self.position + 1 >= self.limit {
            self.history.pop_front();
        }
        self.history.push_back(text.to_string());
        self.position = self.history.len() - 1;
    }

    pub fn current(&self) -> String {
        match self.history.get(self.position) {
            Some(x) => x.to_string(),
            None => "".to_string(),
        }
    }

    pub fn clear(&mut self) {
        self.initialize("".to_string());
    }

    pub fn initialize(&mut self, value: String) {
        self.history.clear();
        self.history.push_back(value);
        self.position = 0;
    }

    pub fn undo(&mut self) -> String {
        self.position = if self.position == 0 { 0 } else { self.position - 1 };
        self.current()
    }

    pub fn redo(&mut self) -> String {
        self.position = std::cmp::min(self.position + 1, self.history.len() - 1);
        self.current()
    }

    pub fn apply(&mut self, shortcut: Shortcut) {
        match shortcut {
            Shortcut::Undo => {
                self.undo();
            }
            Shortcut::Redo => {
                self.redo();
            }
        }
    }
}

#[derive(Default)]
pub struct RootHistory {
    pub path: TextHistory,
    pub lutris_database: TextHistory,
}

impl RootHistory {
    pub fn clear_secondary(&mut self) {
        self.lutris_database.clear();
    }
}

#[derive(Default)]
pub struct CustomGameHistory {
    pub name: TextHistory,
    pub alias: TextHistory,
    pub files: Vec<TextHistory>,
    pub registry: Vec<TextHistory>,
    pub install_dir: Vec<TextHistory>,
    pub wine_prefix: Vec<TextHistory>,
}

#[derive(Default)]
pub struct ModalHistory {
    pub url: TextHistory,
    pub host: TextHistory,
    pub port: TextHistory,
    pub username: TextHistory,
    pub password: TextHistory,
}

#[derive(Default)]
pub struct TextHistories {
    pub backup_target: TextHistory,
    pub restore_source: TextHistory,
    pub backup_search_game_name: TextHistory,
    pub custom_games_search_game_name: TextHistory,
    pub roots: Vec<RootHistory>,
    pub secondary_manifests: Vec<TextHistory>,
    pub custom_games: Vec<CustomGameHistory>,
    pub backup_filter_ignored_paths: Vec<TextHistory>,
    pub backup_filter_ignored_registry: Vec<TextHistory>,
    pub rclone_executable: TextHistory,
    pub rclone_arguments: TextHistory,
    pub cloud_remote_id: TextHistory,
    pub cloud_path: TextHistory,
    pub modal: ModalHistory,
}

impl TextHistories {
    pub fn new(config: &Config) -> Self {
        let mut histories = Self {
            backup_target: TextHistory::path(&config.backup.path),
            restore_source: TextHistory::path(&config.restore.path),
            backup_search_game_name: TextHistory::raw(""),
            rclone_executable: TextHistory::path(&config.apps.rclone.path),
            rclone_arguments: TextHistory::raw(&config.apps.rclone.arguments),
            cloud_path: TextHistory::raw(&config.cloud.path),
            ..Default::default()
        };

        for x in &config.roots {
            histories.roots.push(RootHistory {
                path: TextHistory::path(x.path()),
                lutris_database: x.lutris_database().map(TextHistory::path).unwrap_or_default(),
            });
        }

        for x in &config.manifest.secondary {
            histories.secondary_manifests.push(TextHistory::raw(&x.value()));
        }

        for x in &config.custom_games {
            histories.add_custom_game(x);
        }

        for x in &config.backup.filter.ignored_paths {
            histories.backup_filter_ignored_paths.push(TextHistory::path(x));
        }
        for x in &config.backup.filter.ignored_registry {
            histories
                .backup_filter_ignored_registry
                .push(TextHistory::raw(&x.raw()));
        }

        if let Some(Remote::Custom { id }) = &config.cloud.remote {
            histories.cloud_remote_id.push(id);
        }

        histories
    }

    pub fn add_custom_game(&mut self, game: &CustomGame) {
        let history = CustomGameHistory {
            name: TextHistory::raw(&game.name),
            alias: TextHistory::raw(&game.alias.clone().unwrap_or_default()),
            files: game.files.iter().map(|x| TextHistory::raw(x)).collect(),
            registry: game.registry.iter().map(|x| TextHistory::raw(x)).collect(),
            install_dir: game.install_dir.iter().map(|x| TextHistory::raw(x)).collect(),
            wine_prefix: game.wine_prefix.iter().map(|x| TextHistory::raw(x)).collect(),
        };
        self.custom_games.push(history);
    }

    pub fn clear_modal_fields(&mut self) {
        self.modal.url.clear();
        self.modal.host.clear();
        self.modal.port.clear();
        self.modal.username.clear();
        self.modal.password.clear();
    }

pub fn input_small<'a>(&self, subject: UndoSubject) -> Element<'a> {
        let current = match &subject {
            UndoSubject::BackupTarget => self.backup_target.current(),
            UndoSubject::RcloneExecutable => self.rclone_executable.current(),
            UndoSubject::RcloneArguments => self.rclone_arguments.current(),
            UndoSubject::CloudPath => self.cloud_path.current(),
            UndoSubject::CloudRemoteId => self.cloud_remote_id.current(),
            UndoSubject::RootPath(i) => self.roots.get(*i).map(|x| x.path.current()).unwrap_or_default(),
            UndoSubject::RootLutrisDatabase(i) => self.roots.get(*i).map(|x| x.lutris_database.current()).unwrap_or_default(),
            UndoSubject::SecondaryManifest(i) => self.secondary_manifests.get(*i).map(|x| x.current()).unwrap_or_default(),
            UndoSubject::ModalField(field) => match field {
                ModalInputKind::Url => self.modal.url.current(),
                ModalInputKind::Host => self.modal.host.current(),
                ModalInputKind::Port => self.modal.port.current(),
                ModalInputKind::Username => self.modal.username.current(),
                ModalInputKind::Password => self.modal.password.current(),
            },
        };

        let event: Box<dyn Fn(String) -> Message> = match subject.clone() {
            UndoSubject::BackupTarget => Box::new(Message::config(config::Event::BackupTarget)),
            UndoSubject::RcloneExecutable => Box::new(Message::config(config::Event::RcloneExecutable)),
            UndoSubject::RcloneArguments => Box::new(Message::config(config::Event::RcloneArguments)),
            UndoSubject::CloudPath => Box::new(Message::config(config::Event::CloudPath)),
            UndoSubject::CloudRemoteId => Box::new(Message::config(config::Event::CloudRemoteId)),
            UndoSubject::RootPath(i) => Box::new(Message::config(move |value| {
                config::Event::Root(EditAction::Change(i, value))
            })),
            UndoSubject::RootLutrisDatabase(i) => Box::new(Message::config(move |value| {
                config::Event::RootLutrisDatabase(i, value)
            })),
            UndoSubject::SecondaryManifest(i) => Box::new(Message::config(move |value| {
                config::Event::SecondaryManifest(EditAction::Change(i, value))
            })),
            UndoSubject::ModalField(field) => Box::new(move |value| {
                Message::EditedModalField(match field {
                    ModalInputKind::Url => ModalField::Url(value),
                    ModalInputKind::Host => ModalField::Host(value),
                    ModalInputKind::Port => ModalField::Port(value),
                    ModalInputKind::Username => ModalField::Username(value),
                    ModalInputKind::Password => ModalField::Password(value),
                })
            }),
        };

        let is_password = matches!(
            subject,
            UndoSubject::ModalField(ModalInputKind::Password)
        );

        Undoable::new(
            {
                let mut input = TextInput::new("", &current)
                    .on_input(event)
                    .class(style::TextInput)
                    .width(Length::Fill)
                    .padding([5, 5])
                    .size(12);

                if is_password {
                    input = input.secure(true);
                }

                input
            },
            move |action| Message::UndoRedo(action, subject.clone()),
        )
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_history() {
        let mut ht = TextHistory::new("initial", 3);

        assert_eq!(ht.current(), "initial");
        assert_eq!(ht.undo(), "initial");
        assert_eq!(ht.redo(), "initial");

        ht.push("a");
        assert_eq!(ht.current(), "a");
        assert_eq!(ht.undo(), "initial");
        assert_eq!(ht.undo(), "initial");
        assert_eq!(ht.redo(), "a");
        assert_eq!(ht.redo(), "a");

        // Duplicates are ignored:
        ht.push("a");
        ht.push("a");
        ht.push("a");
        assert_eq!(ht.undo(), "initial");

        // History is clipped at the limit:
        ht.push("b");
        ht.push("c");
        ht.push("d");
        assert_eq!(ht.undo(), "c");
        assert_eq!(ht.undo(), "b");
        assert_eq!(ht.undo(), "b");

        // Redos are lost on push:
        ht.push("e");
        assert_eq!(ht.current(), "e");
        assert_eq!(ht.redo(), "e");
        assert_eq!(ht.undo(), "b");
        assert_eq!(ht.undo(), "b");
    }
}
