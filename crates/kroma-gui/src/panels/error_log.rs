//! Error Log panel — compile errors, daemon messages, severity filtering.

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{color, Border, Element, Fill, Padding, Theme};

use crate::Message;
use crate::panels::{AppContext, Panel};

pub struct ErrorLogPanel;

impl ErrorLogPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for ErrorLogPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let mut items: Vec<Element<Message>> = Vec::new();

        // Compile errors
        for err in ctx.compile_errors {
            let line = err.line.map_or("?".to_string(), |l| l.to_string());
            let msg = format!("\u{2716} L{}: {}", line, err.message);
            items.push(text(msg).size(10).color(color!(0xf87171)).into());
        }

        // Compile warnings
        for warn in ctx.compile_warnings {
            items.push(text(format!("\u{26A0} {}", warn.as_str())).size(10).color(color!(0xfbbf24)).into());
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

        let header = row![
            text("Errors").size(12),
            iced::widget::horizontal_space(),
            button(text("Clear").size(10))
                .padding(Padding::from([2, 8]))
                .on_press(Message::ClearLog)
                .style(|theme: &Theme, _status| {
                    let p = theme.extended_palette();
                    button::Style {
                        background: Some(p.background.strong.color.into()),
                        text_color: p.background.base.text,
                        border: Border::default().rounded(4),
                        ..Default::default()
                    }
                }),
        ]
        .align_y(iced::Alignment::Center)
        .spacing(4);

        container(
            column![
                header,
                scrollable(content).height(Fill),
            ]
            .spacing(4),
        )
        .width(Fill)
        .height(Fill)
        .padding(8)
        .into()
    }
}
