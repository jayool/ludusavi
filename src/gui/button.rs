use iced::{alignment, keyboard};

use crate::{
    gui::{
        common::{BrowseFileSubject, BrowseSubject, Message, Screen},
        icon::Icon,
        style,
        widget::{text, Button, Element, Text},
    },
    lang::TRANSLATOR,
    prelude::EditAction,
};

const WIDTH: u32 = 125;

fn template_small(content: Text, action: Option<Message>, style: Option<style::Button>) -> Element {
    Button::new(content.align_x(alignment::Horizontal::Center))
        .on_press_maybe(action)
        .class(style.unwrap_or(style::Button::Primary))
        .padding(4)
        .height(28)
        .width(28)
        .into()
}

pub fn primary<'a>(content: String, action: Option<Message>) -> Element<'a> {
    Button::new(text(content).align_x(alignment::Horizontal::Center))
        .on_press_maybe(action)
        .class(style::Button::Primary)
        .padding(5)
        .width(WIDTH)
        .into()
}

pub fn negative<'a>(content: String, action: Option<Message>) -> Element<'a> {
    Button::new(text(content).align_x(alignment::Horizontal::Center))
        .on_press_maybe(action)
        .class(style::Button::Negative)
        .width(WIDTH)
        .padding(5)
        .into()
}

#[allow(dead_code)]
pub fn nav<'a>(screen: Screen, current_screen: Screen) -> Button<'a> {
    let label = match screen {
        Screen::Backup => TRANSLATOR.nav_backup_button(),
        Screen::CustomGames => TRANSLATOR.nav_custom_games_button(),
        Screen::Other => TRANSLATOR.nav_other_button(),
        Screen::Games | Screen::ThisDevice | Screen::AllDevices | Screen::GameDetail(_) | Screen::Accela => return Button::new(
            text("").size(14).align_x(alignment::Horizontal::Center)
        )
        .on_press(Message::SwitchScreen(Screen::Games))
        .padding([5, 20])
        .class(style::Button::NavButtonInactive),
    };

    Button::new(text(label).size(14).align_x(alignment::Horizontal::Center))
        .on_press(Message::SwitchScreen(screen.clone()))
        .padding([5, 20])
        .class(if current_screen == screen {
            style::Button::NavButtonActive
        } else {
            style::Button::NavButtonInactive
        })
}

pub fn expand<'a>(expanded: bool, on_press: Message) -> Element<'a> {
    Button::new(
        (if expanded {
            Icon::KeyboardArrowDown
        } else {
            Icon::KeyboardArrowRight
        })
        .text_small(),
    )
    .on_press(on_press)
    .class(style::Button::Primary)
    .padding(5)
    .height(25)
    .width(25)
    .into()
}
pub fn remove_small<'a>(action: impl Fn(EditAction) -> Message, index: usize) -> Element<'a> {
    template_small(
        Icon::RemoveCircle.text_small(),
        Some(action(EditAction::Remove(index))),
        Some(style::Button::Negative),
    )
}

pub fn move_up_small<'a>(action: impl Fn(EditAction) -> Message, index: usize) -> Element<'a> {
    template_small(
        Icon::ArrowUpward.text_small(),
        (index > 0).then(|| action(EditAction::move_up(index))),
        None,
    )
}

pub fn move_down_small<'a>(action: impl Fn(EditAction) -> Message, index: usize, max: usize) -> Element<'a> {
    template_small(
        Icon::ArrowDownward.text_small(),
        (index < max - 1).then(|| action(EditAction::move_down(index))),
        None,
    )
}

pub fn choose_folder_small<'a>(subject: BrowseSubject, modifiers: &keyboard::Modifiers) -> Element<'a> {
    if modifiers.shift() {
        template_small(Icon::OpenInNew.text_small(), Some(Message::OpenDirSubject(subject)), None)
    } else {
        template_small(Icon::FolderOpen.text_small(), Some(Message::BrowseDir(subject)), None)
    }
}

pub fn choose_file_small<'a>(subject: BrowseFileSubject, modifiers: &keyboard::Modifiers) -> Element<'a> {
    if modifiers.shift() {
        template_small(Icon::OpenInNew.text_small(), Some(Message::OpenFileSubject(subject)), None)
    } else {
        template_small(Icon::FolderOpen.text_small(), Some(Message::BrowseFile(subject)), None)
    }
}
pub fn add_small<'a>(action: impl Fn(EditAction) -> Message) -> Element<'a> {
    template_small(Icon::AddCircle.text_small(), Some(action(EditAction::Add)), None)
}

pub fn search_small<'a>(action: Message) -> Element<'a> {
    template_small(Icon::Search.text_small(), Some(action), None)
}
