//! Reusable UI widget helpers (extracted from main.rs).

use iced::widget::{button, column, container, row, text, Space};
use iced::{color, Border, Element, Fill, Padding, Theme};

use crate::Message;

use super::KromaApp;

impl KromaApp {
    /// A rounded card container with a title.
    pub(crate) fn card<'a>(
        &self,
        title: &'a str,
        content: impl Into<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        container(
            column![
                text(title).size(14),
                Space::with_height(6),
                content.into(),
            ],
        )
        .width(Fill)
        .padding(16)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.weak.color.into()),
                border: Border::default()
                    .rounded(10)
                    .width(1)
                    .color(iced::Color {
                        a: 0.1,
                        ..p.background.base.text
                    }),
                ..Default::default()
            }
        })
        .into()
    }

    /// A label: value row used in info panels.
    pub(crate) fn info_row<'a>(&self, label: &str, value: &str) -> Element<'a, Message> {
        row![
            text(format!("{}:", label)).size(13),
            text(value.to_string()).size(13),
        ]
        .spacing(6)
        .into()
    }

    pub(crate) fn btn_primary<'a>(&self, label: &'a str, msg: Message) -> Element<'a, Message> {
        button(text(label).size(13))
            .padding(Padding::from([8, 16]))
            .on_press(msg)
            .style(|theme: &Theme, status| {
                let p = theme.extended_palette();
                match status {
                    button::Status::Active => button::Style {
                        background: Some(p.primary.base.color.into()),
                        text_color: p.primary.base.text,
                        border: Border::default().rounded(6),
                        ..Default::default()
                    },
                    button::Status::Hovered => button::Style {
                        background: Some(p.primary.strong.color.into()),
                        text_color: p.primary.strong.text,
                        border: Border::default().rounded(6),
                        ..Default::default()
                    },
                    _ => button::primary(theme, status),
                }
            })
            .into()
    }

    pub(crate) fn btn_secondary<'a>(&self, label: &'a str, msg: Message) -> Element<'a, Message> {
        button(text(label).size(13))
            .padding(Padding::from([8, 16]))
            .on_press(msg)
            .style(|theme: &Theme, status| {
                let p = theme.extended_palette();
                match status {
                    button::Status::Active => button::Style {
                        background: Some(p.background.strong.color.into()),
                        text_color: p.background.base.text,
                        border: Border::default()
                            .rounded(6)
                            .width(1)
                            .color(iced::Color {
                                a: 0.2,
                                ..p.background.base.text
                            }),
                        ..Default::default()
                    },
                    button::Status::Hovered => button::Style {
                        background: Some(p.background.weak.color.into()),
                        text_color: p.background.base.text,
                        border: Border::default()
                            .rounded(6)
                            .width(1)
                            .color(iced::Color {
                                a: 0.3,
                                ..p.background.base.text
                            }),
                        ..Default::default()
                    },
                    _ => button::secondary(theme, status),
                }
            })
            .into()
    }

    pub(crate) fn btn_danger<'a>(&self, label: &'a str, msg: Message) -> Element<'a, Message> {
        button(text(label).size(13))
            .padding(Padding::from([8, 16]))
            .on_press(msg)
            .style(|_theme: &Theme, status| match status {
                button::Status::Active => button::Style {
                    background: Some(color!(0xdc2626).into()),
                    text_color: iced::Color::WHITE,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
                button::Status::Hovered => button::Style {
                    background: Some(color!(0xef4444).into()),
                    text_color: iced::Color::WHITE,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
                _ => button::Style {
                    background: Some(color!(0xdc2626).into()),
                    text_color: iced::Color::WHITE,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
            })
            .into()
    }
}
