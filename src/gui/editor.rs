use iced::{
    keyboard, padding,
    widget::Space,
    Alignment, Length,
};

use crate::{
    gui::{
        button,
        common::{BrowseFileSubject, BrowseSubject, Message, UndoSubject},
        shortcuts::TextHistories,
        style,
        widget::{checkbox, pick_list, text, Column, Container, IcedParentExt, Row},
    },
    lang::TRANSLATOR,
    resource::{
        cache::Cache,
        config::{self, Config, SecondaryManifestConfigKind},
        manifest::{Manifest, Store},
    },
};

pub fn root<'a>(config: &Config, histories: &TextHistories, modifiers: &keyboard::Modifiers) -> Container<'a> {
    let mut content = Column::new().width(Length::Fill).spacing(5);
    if config.roots.is_empty() {
        content = content.push(text(TRANSLATOR.no_roots_are_configured()));
    } else {
        content = config
            .roots
            .iter()
            .enumerate()
            .fold(content, |parent, (i, root)| match root.store() {
                Store::Lutris => parent
                    .push(
                        Row::new()
                            .spacing(10)
                            .push(button::move_up_small(Message::config(config::Event::Root), i))
                            .push(button::move_down_small(
                                Message::config(config::Event::Root),
                                i,
                                config.roots.len(),
                            ))
                            .push(histories.input_small(UndoSubject::RootPath(i)))
                            .push(
                                pick_list(
                                    Store::ALL,
                                    Some(root.store()),
                                    Message::config(move |v| config::Event::RootStore(i, v)),
                                )
                                .text_size(12)
                                .padding([5, 5])
                                .class(style::PickList::Primary),
                            )
                            .push(button::choose_folder_small(BrowseSubject::Root(i), modifiers))
                            .push(button::remove_small(Message::config(config::Event::Root), i)),
                    )
                    .push(
                        Row::new()
                            .spacing(10)
                            .align_y(Alignment::Center)
                            .push(space::horizontal().width(70))
                            .push(text(TRANSLATOR.field("pga.db")))
                            .push(histories.input_small(UndoSubject::RootLutrisDatabase(i)))
                            .push(button::choose_file_small(BrowseFileSubject::RootLutrisDatabase(i), modifiers)),
                    ),
                _ => parent.push(
                    Row::new()
                        .spacing(10)
                        .push(button::move_up_small(Message::config(config::Event::Root), i))
                        .push(button::move_down_small(
                            Message::config(config::Event::Root),
                            i,
                            config.roots.len(),
                        ))
                        .push(histories.input_small(UndoSubject::RootPath(i)))
                        .push(
                            pick_list(
                                Store::ALL,
                                Some(root.store()),
                                Message::config(move |v| config::Event::RootStore(i, v)),
                            )
                            .text_size(12)
                            .padding([5, 5])
                            .class(style::PickList::Primary),
                        )
                        .push(button::choose_folder_small(BrowseSubject::Root(i), modifiers))
                        .push(button::remove_small(Message::config(config::Event::Root), i)),
                ),
            });
    };

    content = content.push(
        Row::new()
            .spacing(10)
            .push(button::add_small(Message::config(config::Event::Root)))
            .push(button::search_small(Message::FindRoots)),
    );

    Container::new(content)
}

pub fn manifest<'a>(
    config: &Config,
    cache: &'a Cache,
    histories: &TextHistories,
    modifiers: &keyboard::Modifiers,
) -> Container<'a> {
    let label_width = Length::Fixed(160.0);
    let right_offset = Length::Fixed(70.0);

    let get_checked = |url: Option<&str>, cache: &'a Cache| {
        let url = url?;
        let cached = cache.manifests.get(url)?;
        let checked = match cached.checked {
            Some(x) => chrono::DateTime::<chrono::Local>::from(x)
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string(),
            None => "?".to_string(),
        };
        Some(Container::new(text(checked).size(12).class(style::Text::Muted)).width(label_width))
    };

    let get_updated = |url: Option<&str>, cache: &'a Cache| {
        let url = url?;
        let cached = cache.manifests.get(url)?;
        let updated = match cached.updated {
            Some(x) => chrono::DateTime::<chrono::Local>::from(x)
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string(),
            None => "?".to_string(),
        };
        Some(Container::new(text(updated).size(12).class(style::Text::Muted)).width(label_width))
    };

    let mut content = Column::new()
        .spacing(5)
        .push(
            Row::new()
                .spacing(20)
                .align_y(Alignment::Center)
                .push(text("PATH").size(11).class(style::Text::Muted).width(Length::Fill))
                .push(Container::new(text(TRANSLATOR.checked_label()).size(11).class(style::Text::Muted)).width(label_width))
                .push(Container::new(text(TRANSLATOR.updated_label()).size(11).class(style::Text::Muted)).width(label_width))
                .push_if(!config.manifest.secondary.is_empty(), || {
                    Space::new().width(right_offset)
                }),
        )
        .push(
            Row::new()
                .spacing(20)
                .align_y(Alignment::Center)
                .push(iced::widget::TextInput::new("", config.manifest.url()).width(Length::Fill).padding([5, 5]).size(12))
                .push(get_checked(Some(config.manifest.url()), cache))
                .push(get_updated(Some(config.manifest.url()), cache))
                .push_if(!config.manifest.secondary.is_empty(), || {
                    Space::new().width(right_offset)
                }),
        );

    content = config
        .manifest
        .secondary
        .iter()
        .enumerate()
        .fold(content, |column, (i, _)| {
            column.push(
                Row::new()
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push(
                        checkbox(
                            "",
                            config.manifest.secondary[i].enabled(),
                            Message::config(move |enabled| config::Event::SecondaryManifestEnabled {
                                index: i,
                                enabled,
                            }),
                        )
                        .spacing(0)
                        .class(style::Checkbox),
                    )
                    .push(button::move_up_small(Message::config(config::Event::SecondaryManifest), i))
                    .push(button::move_down_small(
                        Message::config(config::Event::SecondaryManifest),
                        i,
                        config.manifest.secondary.len(),
                    ))
                    .push(
                        pick_list(
                            SecondaryManifestConfigKind::ALL,
                            Some(config.manifest.secondary[i].kind()),
                            Message::config(move |v| config::Event::SecondaryManifestKind(i, v)),
                        )
                        .text_size(12)
                        .padding([5, 5])
                        .class(style::PickList::Primary)
                        .width(75),
                    )
                    .push(histories.input_small(UndoSubject::SecondaryManifest(i)))
                    .push(get_checked(config.manifest.secondary[i].url(), cache))
                    .push(get_updated(config.manifest.secondary[i].url(), cache))
                    .push(match config.manifest.secondary[i].kind() {
                        SecondaryManifestConfigKind::Local => {
                            Some(button::choose_file_small(BrowseFileSubject::SecondaryManifest(i), modifiers))
                        }
                        SecondaryManifestConfigKind::Remote => None,
                    })
                    .push(button::remove_small(Message::config(config::Event::SecondaryManifest), i)),
            )
        });

    content = content.push(button::add_small(Message::config(config::Event::SecondaryManifest)));

    Container::new(content)
}
