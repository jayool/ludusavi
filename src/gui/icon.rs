use iced::{alignment, Length};

use crate::gui::{
    font,
    widget::{text, Text},
};

pub enum Icon {
    AddCircle,
    ArrowBack,
    ArrowDownward,
    ArrowForward,
    ArrowUpward,
    Error,
    FolderOpen,
    Info,
    KeyboardArrowDown,
    KeyboardArrowRight,
    RemoveCircle,
    Search,
    #[allow(unused)]
    Settings,
    SubdirectoryArrowRight,
}

impl Icon {
    pub const fn as_char(&self) -> char {
        match self {
            Self::AddCircle => '\u{E147}',
            Self::ArrowBack => '\u{e5c4}',
            Self::ArrowDownward => '\u{E5DB}',
            Self::ArrowForward => '\u{e5c8}',
            Self::ArrowUpward => '\u{E5D8}',
            Self::Error => '\u{e001}',
            Self::FolderOpen => '\u{E2C8}',
            Self::Info => '\u{e88f}',
            Self::KeyboardArrowDown => '\u{E313}',
            Self::KeyboardArrowRight => '\u{E315}',
            Self::RemoveCircle => '\u{E15C}',
            Self::Search => '\u{e8b6}',
            Self::Settings => '\u{E8B8}',
            Self::SubdirectoryArrowRight => '\u{E5DA}',
        }
    }

    pub fn text(self) -> Text<'static> {
        text(self.as_char().to_string())
            .font(font::ICONS)
            .size(20)
            .width(60)
            .height(20)
            .align_x(alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center)
            .line_height(1.0)
    }

    pub fn text_narrow(self) -> Text<'static> {
        self.text().width(Length::Shrink)
    }

    pub fn text_small(self) -> Text<'static> {
        text(self.as_char().to_string())
            .font(font::ICONS)
            .size(15)
            .width(15)
            .height(15)
            .align_x(alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center)
            .line_height(1.0)
    }
}
