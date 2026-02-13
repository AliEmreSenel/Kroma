//! Live Preview panel — daemon-rendered frame stream display.
//!
//! Displays a real-time JPEG frame stream from the daemon over IPC.
//! Shows daemon status, connection info, and preview controls.

use iced::widget::{column, container, row, text, Image};
use iced::{Element, Fill};

use crate::Message;
use crate::panels::dashboard::{btn_primary, btn_secondary, card, info_row};
use crate::panels::{AppContext, Panel};

pub struct LivePreviewPanel;

impl LivePreviewPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for LivePreviewPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let status_text = if ctx.daemon_connected {
            "\u{2705} Connected"
        } else {
            "\u{274C} Offline"
        };

        let status_card = card(
            "Daemon",
            column![
                text(status_text).size(11),
                info_row("FPS", &format!("{:.1}", ctx.fps)),
                info_row("Status", ctx.status_text),
                if let Some(loaded) = ctx.loaded_shade {
                    info_row("Shader", loaded)
                } else {
                    info_row("Shader", "None")
                },
            ]
            .spacing(2),
        );

        let live_label = if ctx.editor_live_preview { "Live ON" } else { "Live OFF" };
        let stream_label = if ctx.preview_streaming { "Stop Stream" } else { "Start Stream" };
        let controls = card(
            "Controls",
            column![
                btn_primary("Send to Daemon", Message::EditorLivePreview),
                btn_secondary(live_label, Message::EditorToggleLive),
                btn_primary("Compile", Message::EditorCompile),
                btn_secondary(stream_label, Message::TogglePreviewStream),
            ]
            .spacing(4),
        );

        // Build the preview area — show frame if available, else placeholder
        let preview_area: Element<'a, Message> = if let Some(frame_data) = ctx.preview_frame {
            let handle = iced::widget::image::Handle::from_rgba(
                frame_data.width,
                frame_data.height,
                frame_data.pixels.clone(),
            );
            container(
                column![
                    Image::new(handle)
                        .width(Fill)
                        .height(Fill)
                        .content_fit(iced::ContentFit::Contain),
                    row![
                        text(format!("{}x{}", frame_data.width, frame_data.height)).size(9),
                        text(if ctx.preview_streaming { " \u{25CF} Streaming" } else { " \u{25CB} Paused" }).size(9),
                    ]
                    .spacing(8),
                ]
                .spacing(4),
            )
            .width(Fill)
            .height(Fill)
            .padding(4)
            .style(|theme: &iced::Theme| {
                let p = theme.extended_palette();
                container::Style {
                    background: Some(iced::Color::BLACK.into()),
                    border: iced::Border::default()
                        .rounded(6)
                        .width(1)
                        .color(iced::Color { a: 0.12, ..p.background.base.text }),
                    ..Default::default()
                }
            })
            .into()
        } else {
            container(
                column![
                    text("No Preview").size(14),
                    text(if ctx.daemon_connected {
                        "Click 'Start Stream' to begin"
                    } else {
                        "Daemon not connected"
                    }).size(10),
                ]
                .spacing(4),
            )
            .width(Fill)
            .height(Fill)
            .padding(12)
            .center_x(Fill)
            .center_y(Fill)
            .style(|theme: &iced::Theme| {
                let p = theme.extended_palette();
                container::Style {
                    background: Some(p.background.strong.color.into()),
                    border: iced::Border::default()
                        .rounded(6)
                        .width(1)
                        .color(iced::Color { a: 0.12, ..p.background.base.text }),
                    ..Default::default()
                }
            })
            .into()
        };

        column![status_card, controls, preview_area]
            .spacing(8)
            .padding(8)
            .width(Fill)
            .height(Fill)
            .into()
    }
}