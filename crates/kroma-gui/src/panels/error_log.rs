//! Error Log panel — compile errors, daemon messages, severity filtering.

use iced::widget::space::horizontal;
use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Border, Element, Fill, Padding, Theme};

use crate::icons;
use crate::panels::{AppContext, Panel};
use crate::Message;

pub struct ErrorLogPanel;

impl ErrorLogPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for ErrorLogPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let t = ctx.tokens;
        let mut items: Vec<Element<Message>> = Vec::new();

        // Compile errors
        for err in ctx.compile_errors {
            let line = err.line.map_or("?".to_string(), |l| l.to_string());
            let msg = format!("L{}: {}", line, err.message);
            items.push(
                row![
                    text(icons::ERROR)
                        .font(icons::ICON_FONT)
                        .size(10)
                        .color(t.error),
                    text(msg).size(10).color(t.error),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .into(),
            );
        }

        // Compile warnings
        for warn in ctx.compile_warnings {
            items.push(
                row![
                    text(icons::WARNING)
                        .font(icons::ICON_FONT)
                        .size(10)
                        .color(t.warning),
                    text(warn.as_str()).size(10).color(t.warning),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .into(),
            );
        }

        // General log messages (most recent first)
        for msg in ctx.log_messages.iter().rev().take(50) {
            items.push(text(msg.as_str()).size(10).into());
        }

        let content: Element<Message> = if items.is_empty() {
            text("No errors or messages.").size(11).into()
        } else {
            column(items).spacing(1).into()
        };

        let clear_bg = t.bg_tertiary;
        let clear_text = t.text_primary;
        let header = row![
            text("Errors").size(t.font_size_sm),
            horizontal(),
            button(text("Clear").size(10))
                .padding(Padding::from([2, 8]))
                .on_press(Message::ClearLog)
                .style(move |_theme: &Theme, _status| {
                    button::Style {
                        background: Some(clear_bg.into()),
                        text_color: clear_text,
                        border: Border::default().rounded(4),
                        ..Default::default()
                    }
                }),
        ]
        .align_y(iced::Alignment::Center)
        .spacing(4);

        container(column![header, scrollable(content).height(Fill),].spacing(t.spacing_sm))
            .width(Fill)
            .height(Fill)
            .padding(t.spacing_md)
            .into()
    }
}
