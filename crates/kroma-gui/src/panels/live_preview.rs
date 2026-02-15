//! Live Preview panel — daemon-rendered frame stream display.
//!
//! Displays a real-time JPEG frame stream from the daemon over IPC.
//! Shows daemon status, connection info, and preview controls.

use iced::widget::{column, container, row, text, Image};
use iced::{Element, Fill};

use crate::icons;
use crate::panels::dashboard::{btn_primary, btn_secondary, card, info_row};
use crate::panels::{AppContext, Panel};
use crate::Message;

pub struct LivePreviewPanel;

impl LivePreviewPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for LivePreviewPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let t = ctx.tokens;
        let (status_icon, status_label) = if ctx.daemon_connected {
            (icons::CHECK, "Connected")
        } else {
            (icons::CLOSE, "Offline")
        };
        let status_color = if ctx.daemon_connected { t.success } else { t.error };

        let status_card = card(
            "Daemon",
            column![
                row![
                    text(status_icon).font(icons::ICON_FONT).size(11).color(status_color),
                    text(status_label).size(11).color(status_color),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
                info_row("FPS", &format!("{:.1}", ctx.fps)),
                info_row("Status", ctx.status_text),
                if let Some(loaded) = ctx.loaded_shade {
                    info_row("Shader", loaded)
                } else {
                    info_row("Shader", "None")
                },
            ]
            .spacing(2),
            t,
        );

        let live_label = if ctx.editor_live_preview {
            "Live ON"
        } else {
            "Live OFF"
        };
        let stream_label = if ctx.preview_streaming {
            "Stop Stream"
        } else {
            "Start Stream"
        };
        let controls = card(
            "Controls",
            column![
                btn_primary("Send to Daemon", Message::EditorLivePreview, t),
                btn_secondary(live_label, Message::EditorToggleLive, t),
                btn_primary("Compile", Message::EditorCompile, t),
                btn_secondary(stream_label, Message::TogglePreviewStream, t),
            ]
            .spacing(4),
            t,
        );

        // Build the preview area — show frame if available, else placeholder
        let preview_border = t.border_default;
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
                        text(if ctx.preview_streaming {
                            " (*) Streaming"
                        } else {
                            " ( ) Paused"
                        })
                        .size(9),
                    ]
                    .spacing(8),
                ]
                .spacing(4),
            )
            .width(Fill)
            .height(Fill)
            .padding(4)
            .style(move |_theme: &iced::Theme| container::Style {
                background: Some(iced::Color::BLACK.into()),
                border: iced::Border::default()
                    .rounded(6)
                    .width(1)
                    .color(preview_border),
                ..Default::default()
            })
            .into()
        } else {
            let no_preview_bg = t.bg_tertiary;
            container(
                column![
                    text("No Preview").size(14),
                    text(if ctx.daemon_connected {
                        "Click 'Start Stream' to begin"
                    } else {
                        "Daemon not connected"
                    })
                    .size(10),
                ]
                .spacing(4),
            )
            .width(Fill)
            .height(Fill)
            .padding(12)
            .center_x(Fill)
            .center_y(Fill)
            .style(move |_theme: &iced::Theme| container::Style {
                background: Some(no_preview_bg.into()),
                border: iced::Border::default()
                    .rounded(6)
                    .width(1)
                    .color(preview_border),
                ..Default::default()
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
