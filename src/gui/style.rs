// src/gui/style.rs
use iced::{
    widget::{button, checkbox, container, pick_list, scrollable, text_editor, text_input},
    Background, Border, Color, Shadow, Vector,
};

use crate::{resource::config, scan::ScanChange};

macro_rules! rgb8 {
    ($r:expr, $g:expr, $b:expr) => {
        Color::from_rgb($r as f32 / 255.0, $g as f32 / 255.0, $b as f32 / 255.0)
    };
}

trait ColorExt {
    fn alpha(self, alpha: f32) -> Color;
}

impl ColorExt for Color {
    fn alpha(mut self, alpha: f32) -> Self {
        self.a = alpha;
        self
    }
}

pub struct Theme {
    source: config::Theme,
    background: Color,
    // Surface layers
    surface: Color,
    surface2: Color,
    // Borders
    border: Color,
    // Text
    field: Color,
    text: Color,
    text_inverted: Color,
    text_button: Color,
    text_skipped: Color,
    text_selection: Color,
    text_muted: Color,
    text_dim: Color,
    // Semantic colors
    positive: Color,   // accent blue
    negative: Color,   // red
    disabled: Color,
    navigation: Color,
    success: Color,
    failure: Color,
    skipped: Color,
    added: Color,
    #[allow(dead_code)]
    yellow: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::from(config::Theme::Light)
    }
}

impl From<config::Theme> for Theme {
    fn from(source: config::Theme) -> Self {
        match source {
            config::Theme::Light => Self {
                source,
                background: Color::WHITE,
                surface: rgb8!(240, 240, 240),
                surface2: rgb8!(225, 225, 225),
                border: rgb8!(200, 200, 200),
                field: rgb8!(230, 230, 230),
                text: Color::BLACK,
                text_inverted: Color::WHITE,
                text_button: Color::WHITE,
                text_skipped: Color::BLACK,
                text_selection: Color::from_rgb(0.8, 0.8, 1.0),
                text_muted: rgb8!(100, 116, 139),
                text_dim: rgb8!(148, 163, 184),
                positive: rgb8!(28, 107, 223),
                negative: rgb8!(255, 0, 0),
                disabled: rgb8!(169, 169, 169),
                navigation: rgb8!(136, 0, 219),
                success: rgb8!(77, 127, 201),
                failure: rgb8!(201, 77, 77),
                skipped: rgb8!(230, 230, 230),
                added: rgb8!(28, 223, 86),
                yellow: rgb8!(245, 166, 35),
            },
            config::Theme::Dark => Self {
                source,
                // Main backgrounds — mockup palette
                background: rgb8!(15, 17, 23),    // --bg: #0f1117
                surface: rgb8!(23, 27, 38),        // --surface: #171b26
                surface2: rgb8!(30, 35, 51),       // --surface2: #1e2333
                border: rgb8!(42, 47, 66),         // --border: #2a2f42
                field: rgb8!(42, 47, 66),          // same as border for inputs
                text: rgb8!(226, 232, 240),        // --text: #e2e8f0
                text_inverted: rgb8!(15, 17, 23),
                text_button: Color::WHITE,
                text_skipped: rgb8!(226, 232, 240),
                text_selection: rgb8!(79, 142, 247).alpha(0.3),
                text_muted: rgb8!(100, 116, 139),  // --text-muted: #64748b
                text_dim: rgb8!(148, 163, 184),    // --text-dim: #94a3b8
                positive: rgb8!(79, 142, 247),     // --accent: #4f8ef7
                negative: rgb8!(242, 107, 107),    // --red: #f26b6b
                disabled: rgb8!(42, 47, 66),
                navigation: rgb8!(79, 142, 247),
                success: rgb8!(79, 142, 247),
                failure: rgb8!(242, 107, 107),
                skipped: rgb8!(30, 35, 51),
                added: rgb8!(62, 207, 142),        // --green: #3ecf8e
                yellow: rgb8!(245, 166, 35),       // --yellow: #f5a623
            },
        }
    }
}

impl iced::theme::Base for Theme {
    fn default(_preference: iced::theme::Mode) -> Self {
        <Theme as Default>::default()
    }

    fn mode(&self) -> iced::theme::Mode {
        match self.source {
            config::Theme::Light => iced::theme::Mode::Light,
            config::Theme::Dark => iced::theme::Mode::Dark,
        }
    }

    fn base(&self) -> iced::theme::Style {
        iced::theme::Style {
            background_color: self.background,
            text_color: self.text,
        }
    }

    fn palette(&self) -> Option<iced::theme::Palette> {
        None
    }

    fn name(&self) -> &str {
        match self.source {
            config::Theme::Light => "light",
            config::Theme::Dark => "dark",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum Text {
    #[default]
    Default,
    Failure,
    Muted,
    Dim,
    Accent,
    Green,
}
impl iced::widget::text::Catalog for Theme {
    type Class<'a> = Text;

    fn default<'a>() -> Self::Class<'a> {
        Default::default()
    }

    fn style(&self, item: &Self::Class<'_>) -> iced::widget::text::Style {
        match item {
            Text::Default => iced::widget::text::Style { color: None },
            Text::Failure => iced::widget::text::Style { color: Some(self.negative) },
            Text::Muted => iced::widget::text::Style { color: Some(self.text_muted) },
            Text::Dim => iced::widget::text::Style { color: Some(self.text_dim) },
            Text::Accent => iced::widget::text::Style { color: Some(self.positive) },
            Text::Green => iced::widget::text::Style { color: Some(self.added) },
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Menu;
impl iced::widget::overlay::menu::Catalog for Theme {
    type Class<'a> = Menu;

    fn default<'a>() -> <Self as iced::overlay::menu::Catalog>::Class<'a> {
        Default::default()
    }

    fn style(&self, _class: &<Self as iced::overlay::menu::Catalog>::Class<'_>) -> iced::overlay::menu::Style {
        iced::overlay::menu::Style {
            background: self.surface2.into(),
            border: Border {
                color: self.border,
                width: 1.0,
                radius: 7.0.into(),
            },
            text_color: self.text,
            selected_background: self.positive.into(),
            selected_text_color: Color::WHITE,
            shadow: Shadow::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum Button {
    #[default]
    Primary,
    Negative,
    // Sidebar navigation
    SidebarItem,
    SidebarItemActive,
    // Top bar / content actions
    Ghost,
    // Game list
    GameActionPrimary,
    GameListEntryTitle,
    GameListEntryTitleFailed,
    GameListEntryTitleDisabled,
    GameListEntryTitleUnscanned,
    // Legacy nav (kept for compatibility)
    NavButtonActive,
    NavButtonInactive,
    Badge,
    Bare,
}
impl button::Catalog for Theme {
    type Class<'a> = Button;

    fn default<'a>() -> Self::Class<'a> {
        Default::default()
    }

    fn style(&self, class: &Self::Class<'_>, status: button::Status) -> button::Style {
        let active = button::Style {
            background: match class {
                Button::Primary | Button::GameActionPrimary => Some(self.positive.into()),
                Button::Ghost => Some(self.surface2.into()),
                Button::SidebarItemActive => Some(self.positive.alpha(0.12).into()),
                Button::SidebarItem => None,
                Button::GameListEntryTitle => Some(self.success.into()),
                Button::GameListEntryTitleFailed => Some(self.failure.into()),
                Button::GameListEntryTitleDisabled => Some(self.skipped.into()),
                Button::GameListEntryTitleUnscanned => None,
                Button::Negative => Some(self.negative.into()),
                Button::NavButtonActive => Some(self.navigation.alpha(0.9).into()),
                Button::NavButtonInactive => None,
                Button::Badge | Button::Bare => None,
            },
            border: Border {
                color: match class {
                    Button::Ghost => self.border,
                    Button::SidebarItemActive | Button::SidebarItem => Color::TRANSPARENT,
                    Button::NavButtonActive | Button::NavButtonInactive => self.navigation,
                    _ => Color::TRANSPARENT,
                },
                width: match class {
                    Button::Ghost => 1.0,
                    Button::NavButtonActive | Button::NavButtonInactive => 1.0,
                    _ => 0.0,
                },
                radius: match class {
                    Button::SidebarItem | Button::SidebarItemActive => 7.0.into(),
                    Button::Ghost => 7.0.into(),
                    Button::Primary | Button::GameActionPrimary => 7.0.into(),
                    Button::GameListEntryTitle
                    | Button::GameListEntryTitleFailed
                    | Button::GameListEntryTitleDisabled
                    | Button::GameListEntryTitleUnscanned
                    | Button::NavButtonActive
                    | Button::NavButtonInactive => 10.0.into(),
                    _ => 4.0.into(),
                },
            },
            text_color: match class {
                Button::SidebarItemActive => self.positive,
                Button::SidebarItem => self.text_muted,
                Button::Ghost => self.text_dim,
                Button::GameListEntryTitleDisabled => self.text_skipped.alpha(0.8),
                Button::GameListEntryTitleUnscanned => self.text.alpha(0.8),
                Button::NavButtonInactive | Button::Bare => self.text,
                _ => self.text_button.alpha(0.9),
            },
            shadow: Shadow {
                offset: match class {
                    Button::SidebarItem
                    | Button::SidebarItemActive
                    | Button::Ghost
                    | Button::NavButtonActive
                    | Button::NavButtonInactive => Vector::new(0.0, 0.0),
                    _ => Vector::new(1.0, 1.0),
                },
                ..Default::default()
            },
            snap: true,
        };

        match status {
            button::Status::Active => active,
            button::Status::Hovered => button::Style {
                background: match class {
                    Button::SidebarItemActive => Some(self.positive.alpha(0.15).into()),
                    Button::SidebarItem => Some(self.surface2.into()),
                    Button::Ghost => Some(self.surface2.alpha(0.8).into()),
                    Button::NavButtonActive => Some(self.navigation.alpha(0.95).into()),
                    Button::NavButtonInactive => Some(self.navigation.alpha(0.2).into()),
                    _ => active.background,
                },
                border: Border {
                    color: match class {
                        Button::Ghost => self.border.alpha(0.8),
                        Button::NavButtonActive | Button::NavButtonInactive => self.navigation,
                        _ => active.border.color,
                    },
                    ..active.border
                },
                text_color: match class {
                    Button::SidebarItemActive => self.positive,
                    Button::SidebarItem => self.text_dim,
                    Button::Ghost => self.text,
                    Button::GameListEntryTitleDisabled => self.text_skipped,
                    Button::GameListEntryTitleUnscanned | Button::NavButtonInactive => self.text,
                    Button::Bare => self.text.alpha(0.9),
                    _ => self.text_button,
                },
                shadow: Shadow {
                    offset: match class {
                        Button::SidebarItem
                        | Button::SidebarItemActive
                        | Button::Ghost
                        | Button::NavButtonActive
                        | Button::NavButtonInactive => Vector::new(0.0, 0.0),
                        _ => Vector::new(1.0, 2.0),
                    },
                    ..Default::default()
                },
                snap: true,
            },
            button::Status::Pressed => button::Style {
                shadow: Shadow {
                    offset: Vector::default(),
                    ..active.shadow
                },
                ..active
            },
            button::Status::Disabled => button::Style {
                shadow: Shadow {
                    offset: Vector::default(),
                    ..active.shadow
                },
                background: active.background.map(|background| match background {
                    Background::Color(color) => Background::Color(Color {
                        a: color.a * 0.5,
                        ..color
                    }),
                    Background::Gradient(gradient) => Background::Gradient(gradient.scale_alpha(0.5)),
                }),
                text_color: Color {
                    a: active.text_color.a * 0.5,
                    ..active.text_color
                },
                ..active
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum Container {
    #[default]
    Wrapper,
    Primary,
    // Sidebar
    Sidebar,
    TopBar,
    // Content
    ContentArea,
    GamesTable,
    GamesTableRow,
    // Daemon status pill in sidebar
    DaemonStatus,
    // Modal
    ModalForeground,
    ModalBackground,
    // Game list (legacy)
    GameListEntry,
    Badge,
    BadgeActivated,
    BadgeFaded,
    ChangeBadge {
        change: ScanChange,
        faded: bool,
    },
    DisabledBackup,
    Notification,
    Tooltip,
    // Daemon dot
    DaemonDotActive,
    DaemonDotInactive,
}
impl container::Catalog for Theme {
    type Class<'a> = Container;

    fn default<'a>() -> Self::Class<'a> {
        Default::default()
    }

    fn style(&self, class: &Self::Class<'_>) -> container::Style {
        container::Style {
            background: Some(match class {
                Container::Wrapper => Color::TRANSPARENT.into(),
                Container::Primary => self.background.into(),
                Container::Sidebar => self.surface.into(),
                Container::TopBar => self.surface.into(),
                Container::ContentArea => self.background.into(),
                Container::GamesTable => self.surface.into(),
                Container::GamesTableRow => Color::TRANSPARENT.into(),
                Container::DaemonStatus => self.added.alpha(0.12).into(),
                Container::GameListEntry => self.field.alpha(0.15).into(),
                Container::ModalBackground => self.field.alpha(0.75).into(),
                Container::Notification => self.surface2.alpha(0.9).into(),
                Container::Tooltip => self.surface2.into(),
                Container::DisabledBackup => self.disabled.into(),
                Container::DaemonDotActive => self.added.into(),
                Container::DaemonDotInactive => self.text_muted.into(),
                Container::BadgeActivated => self.negative.into(),
                _ => self.background.into(),
            }),
            border: Border {
                color: match class {
                    Container::Wrapper | Container::Primary | Container::ContentArea => Color::TRANSPARENT,
                    Container::Sidebar => self.border,
                    Container::TopBar => self.border,
                    Container::GamesTable => self.border,
                    Container::GamesTableRow => self.border,
                    Container::DaemonStatus => self.added.alpha(0.19),
                    Container::GameListEntry | Container::Notification => self.field,
                    Container::ChangeBadge { change, faded } => {
                        if *faded {
                            self.disabled
                        } else {
                            match change {
                                ScanChange::New => self.added,
                                ScanChange::Different => self.positive,
                                ScanChange::Removed => self.negative,
                                ScanChange::Same | ScanChange::Unknown => self.disabled,
                            }
                        }
                    }
                    Container::BadgeActivated => self.negative,
                    Container::DaemonDotActive | Container::DaemonDotInactive => Color::TRANSPARENT,
                    Container::ModalForeground | Container::BadgeFaded => self.disabled,
                    _ => self.border,
                },
                width: match class {
                    Container::Sidebar => 1.0,
                    Container::TopBar => 1.0,
                    Container::GamesTable => 1.0,
                    Container::GamesTableRow => 0.0,
                    Container::DaemonStatus => 1.0,
                    Container::DaemonDotActive | Container::DaemonDotInactive => 0.0,
                    Container::GameListEntry
                    | Container::ModalForeground
                    | Container::Badge
                    | Container::BadgeActivated
                    | Container::BadgeFaded
                    | Container::ChangeBadge { .. }
                    | Container::Notification => 1.0,
                    _ => 0.0,
                },
                radius: match class {
                    Container::Sidebar | Container::TopBar | Container::ContentArea | Container::Primary => {
                        0.0.into()
                    }
                    Container::GamesTable => 10.0.into(),
                    Container::GamesTableRow => 0.0.into(),
                    Container::DaemonStatus => 8.0.into(),
                    Container::DaemonDotActive | Container::DaemonDotInactive => 4.0.into(),
                    Container::ModalForeground
                    | Container::GameListEntry
                    | Container::Badge
                    | Container::BadgeActivated
                    | Container::BadgeFaded
                    | Container::ChangeBadge { .. }
                    | Container::DisabledBackup => 10.0.into(),
                    Container::Notification | Container::Tooltip => 8.0.into(),
                    _ => 0.0.into(),
                },
            },
            text_color: match class {
                Container::Wrapper | Container::GamesTableRow => None,
                Container::DaemonDotActive | Container::DaemonDotInactive => None,
                Container::DaemonStatus => Some(self.added),
                Container::DisabledBackup => Some(self.text_inverted),
                Container::ChangeBadge { change, faded } => {
                    if *faded {
                        Some(self.disabled)
                    } else {
                        match change {
                            ScanChange::New => Some(self.added),
                            ScanChange::Different => Some(self.positive),
                            ScanChange::Removed => Some(self.negative),
                            ScanChange::Same | ScanChange::Unknown => Some(self.disabled),
                        }
                    }
                }
                Container::BadgeActivated => Some(self.text_button),
                Container::BadgeFaded => Some(self.disabled),
                _ => Some(self.text),
            },
            shadow: Shadow {
                color: Color::TRANSPARENT,
                offset: Vector::ZERO,
                blur_radius: 0.0,
            },
            snap: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Scrollable;
impl scrollable::Catalog for Theme {
    type Class<'a> = Scrollable;

    fn default<'a>() -> Self::Class<'a> {
        Default::default()
    }

    fn style(&self, _class: &Self::Class<'_>, status: scrollable::Status) -> scrollable::Style {
        let active = scrollable::Style {
            auto_scroll: scrollable::AutoScroll {
                background: self.background.into(),
                border: Border::default(),
                shadow: Shadow::default(),
                icon: self.text,
            },
            container: container::Style::default(),
            vertical_rail: scrollable::Rail {
                background: Some(Color::TRANSPARENT.into()),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 3.0.into(),
                },
                scroller: scrollable::Scroller {
                    background: self.border.alpha(0.8).into(),
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 3.0.into(),
                    },
                },
            },
            horizontal_rail: scrollable::Rail {
                background: Some(Color::TRANSPARENT.into()),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 3.0.into(),
                },
                scroller: scrollable::Scroller {
                    background: self.border.alpha(0.8).into(),
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: 3.0.into(),
                    },
                },
            },
            gap: None,
        };

        match status {
            scrollable::Status::Active { .. } => active,
            scrollable::Status::Hovered {
                is_horizontal_scrollbar_hovered,
                is_vertical_scrollbar_hovered,
                ..
            } => {
                if !is_horizontal_scrollbar_hovered && !is_vertical_scrollbar_hovered {
                    return active;
                }

                scrollable::Style {
                    vertical_rail: scrollable::Rail {
                        background: Some(self.surface2.into()),
                        border: Border {
                            color: self.border,
                            ..active.vertical_rail.border
                        },
                        ..active.vertical_rail
                    },
                    horizontal_rail: scrollable::Rail {
                        background: Some(self.surface2.into()),
                        border: Border {
                            color: self.border,
                            ..active.horizontal_rail.border
                        },
                        ..active.horizontal_rail
                    },
                    ..active
                }
            }
            scrollable::Status::Dragged { .. } => self.style(
                _class,
                scrollable::Status::Hovered {
                    is_horizontal_scrollbar_hovered: true,
                    is_vertical_scrollbar_hovered: true,
                    is_horizontal_scrollbar_disabled: false,
                    is_vertical_scrollbar_disabled: false,
                },
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum PickList {
    #[default]
    Primary,
    Backup,
    Popup,
}
impl pick_list::Catalog for Theme {
    type Class<'a> = PickList;

    fn default<'a>() -> <Self as pick_list::Catalog>::Class<'a> {
        Default::default()
    }

    fn style(&self, class: &<Self as pick_list::Catalog>::Class<'_>, status: pick_list::Status) -> pick_list::Style {
        let active = pick_list::Style {
            border: Border {
                color: self.border,
                width: 1.0,
                radius: match class {
                    PickList::Primary => 7.0.into(),
                    PickList::Backup | PickList::Popup => 10.0.into(),
                },
            },
            background: self.surface2.into(),
            text_color: self.text,
            placeholder_color: self.text_muted,
            handle_color: self.text_dim,
        };

        match status {
            pick_list::Status::Active => active,
            pick_list::Status::Hovered => pick_list::Style {
                background: self.surface2.alpha(0.8).into(),
                border: Border {
                    color: self.positive.alpha(0.5),
                    ..active.border
                },
                ..active
            },
            pick_list::Status::Opened { .. } => pick_list::Style {
                border: Border {
                    color: self.positive,
                    ..active.border
                },
                ..active
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Checkbox;
impl checkbox::Catalog for Theme {
    type Class<'a> = Checkbox;

    fn default<'a>() -> Self::Class<'a> {
        Default::default()
    }

    fn style(&self, _class: &Self::Class<'_>, status: checkbox::Status) -> checkbox::Style {
        let active = checkbox::Style {
            background: self.surface2.into(),
            icon_color: self.text,
            border: Border {
                color: self.border,
                width: 1.0,
                radius: 4.0.into(),
            },
            text_color: Some(self.text),
        };

        match status {
            checkbox::Status::Active { .. } => active,
            checkbox::Status::Hovered { .. } => checkbox::Style {
                border: Border {
                    color: self.positive.alpha(0.5),
                    ..active.border
                },
                ..active
            },
            checkbox::Status::Disabled { .. } => checkbox::Style {
                background: match active.background {
                    Background::Color(color) => Background::Color(Color {
                        a: color.a * 0.5,
                        ..color
                    }),
                    Background::Gradient(gradient) => Background::Gradient(gradient.scale_alpha(0.5)),
                },
                text_color: Some(self.text_muted),
                ..active
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TextInput;
impl text_input::Catalog for Theme {
    type Class<'a> = TextInput;

    fn default<'a>() -> Self::Class<'a> {
        Default::default()
    }

    fn style(&self, _class: &Self::Class<'_>, status: text_input::Status) -> text_input::Style {
        let active = text_input::Style {
            background: self.surface2.into(),
            border: Border {
                color: self.border,
                width: 1.0,
                radius: 6.0.into(),
            },
            icon: self.negative,
            placeholder: self.text_muted,
            value: self.text,
            selection: self.text_selection,
        };

        match status {
            text_input::Status::Active => active,
            text_input::Status::Hovered | text_input::Status::Focused { .. } => text_input::Style {
                border: Border {
                    color: self.positive,
                    ..active.border
                },
                ..active
            },
            text_input::Status::Disabled => text_input::Style {
                background: self.disabled.into(),
                value: self.text_muted,
                ..active
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProgressBar;
impl iced::widget::progress_bar::Catalog for Theme {
    type Class<'a> = ProgressBar;

    fn default<'a>() -> Self::Class<'a> {
        Default::default()
    }

    fn style(&self, _class: &Self::Class<'_>) -> iced::widget::progress_bar::Style {
        iced::widget::progress_bar::Style {
            background: self.surface2.into(),
            bar: self.positive.into(),
            border: Border {
                radius: 4.0.into(),
                ..Default::default()
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TextEditor;
impl text_editor::Catalog for Theme {
    type Class<'a> = TextEditor;

    fn default<'a>() -> Self::Class<'a> {
        Default::default()
    }

    fn style(&self, _class: &Self::Class<'_>, status: text_editor::Status) -> text_editor::Style {
        let active = text_editor::Style {
            background: self.surface2.alpha(0.5).into(),
            border: Border {
                radius: 6.0.into(),
                width: 1.0,
                color: self.border,
            },
            placeholder: self.text_muted,
            value: self.text,
            selection: self.text_selection,
        };

        match status {
            text_editor::Status::Active => active,
            text_editor::Status::Hovered => text_editor::Style {
                border: Border {
                    color: self.positive.alpha(0.5),
                    ..active.border
                },
                ..active
            },
            text_editor::Status::Focused { .. } => text_editor::Style {
                border: Border {
                    color: self.positive,
                    ..active.border
                },
                ..active
            },
            text_editor::Status::Disabled => text_editor::Style {
                background: Background::Color(self.disabled),
                value: self.text_muted,
                ..active
            },
        }
    }
}
