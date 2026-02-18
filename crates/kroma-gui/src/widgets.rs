//! Reusable UI widget helpers (extracted from main.rs).

use iced::widget::{button, column, container, row, text, Space};
use iced::{Border, Element, Fill, Padding, Theme};

use crate::Message;

use super::KromaApp;

impl KromaApp {
    /// A rounded card container with a title.
    #[allow(dead_code)]
    pub(crate) fn card<'a>(
        &self,
        title: &'a str,
        content: impl Into<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        let card_bg = self.theme_tokens.bg_secondary;
        let card_border = self.theme_tokens.border_default;
        container(column![
            text(title).size(self.theme_tokens.font_size_md),
            Space::new().height(6),
            content.into(),
        ])
        .width(Fill)
        .padding(self.theme_tokens.spacing_lg)
        .style(move |_theme: &Theme| container::Style {
            background: Some(card_bg.into()),
            border: Border::default().rounded(10).width(1).color(card_border),
            ..Default::default()
        })
        .into()
    }

    /// A label: value row used in info panels.
    #[allow(dead_code)]
    pub(crate) fn info_row<'a>(&self, label: &str, value: &str) -> Element<'a, Message> {
        row![
            text(format!("{}:", label)).size(13),
            text(value.to_string()).size(13),
        ]
        .spacing(6)
        .into()
    }

    #[allow(dead_code)]
    pub(crate) fn btn_primary<'a>(&self, label: &'a str, msg: Message) -> Element<'a, Message> {
        let active_bg = self.theme_tokens.button_primary;
        let active_text = self.theme_tokens.text_primary;
        let hover_bg = self.theme_tokens.button_hover;
        let pressed_bg = self.theme_tokens.button_pressed;
        button(text(label).size(13))
            .padding(Padding::from([8, 16]))
            .on_press(msg)
            .style(move |_theme: &Theme, status| match status {
                button::Status::Active => button::Style {
                    background: Some(active_bg.into()),
                    text_color: active_text,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
                button::Status::Hovered => button::Style {
                    background: Some(hover_bg.into()),
                    text_color: active_text,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
                button::Status::Pressed => button::Style {
                    background: Some(pressed_bg.into()),
                    text_color: active_text,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
                _ => button::Style {
                    background: Some(active_bg.into()),
                    text_color: active_text,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
            })
            .into()
    }

    #[allow(dead_code)]
    pub(crate) fn btn_secondary<'a>(&self, label: &'a str, msg: Message) -> Element<'a, Message> {
        let active_bg = self.theme_tokens.bg_tertiary;
        let text_col = self.theme_tokens.text_primary;
        let border_col = self.theme_tokens.border_default;
        let hover_bg = self.theme_tokens.bg_secondary;
        let border_hover = self.theme_tokens.border_focused;
        button(text(label).size(13))
            .padding(Padding::from([8, 16]))
            .on_press(msg)
            .style(move |_theme: &Theme, status| match status {
                button::Status::Active => button::Style {
                    background: Some(active_bg.into()),
                    text_color: text_col,
                    border: Border::default().rounded(6).width(1).color(border_col),
                    ..Default::default()
                },
                button::Status::Hovered => button::Style {
                    background: Some(hover_bg.into()),
                    text_color: text_col,
                    border: Border::default().rounded(6).width(1).color(border_hover),
                    ..Default::default()
                },
                _ => button::Style {
                    background: Some(active_bg.into()),
                    text_color: text_col,
                    border: Border::default().rounded(6).width(1).color(border_col),
                    ..Default::default()
                },
            })
            .into()
    }

    #[allow(dead_code)]
    pub(crate) fn btn_danger<'a>(&self, label: &'a str, msg: Message) -> Element<'a, Message> {
        let danger_bg = self.theme_tokens.error;
        let danger_text = self.theme_tokens.text_primary;
        button(text(label).size(13))
            .padding(Padding::from([8, 16]))
            .on_press(msg)
            .style(move |_theme: &Theme, status| match status {
                button::Status::Active | button::Status::Pressed => button::Style {
                    background: Some(danger_bg.into()),
                    text_color: danger_text,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
                button::Status::Hovered => button::Style {
                    background: Some(
                        iced::Color {
                            a: 0.85,
                            ..danger_bg
                        }
                        .into(),
                    ),
                    text_color: danger_text,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
                _ => button::Style {
                    background: Some(danger_bg.into()),
                    text_color: danger_text,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
            })
            .into()
    }
}
