//! Dashboard panel — daemon status, FPS, quick actions, event log.

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Border, Element, Fill, Padding, Theme};

use crate::Message;
use crate::panels::{AppContext, Panel};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Dashboard panel state.
pub struct DashboardPanel {
    // Dashboard is mostly read-only — state comes from AppContext.
}

impl DashboardPanel {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for DashboardPanel {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Panel implementation
// ---------------------------------------------------------------------------

impl Panel for DashboardPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        // FPS card — compact
        let fps_display = format!("{:.0}", ctx.fps);
        let fps_card = card(
            "Performance",
            column![
                text(format!("FPS: {}", fps_display)).size(16),
                iced::widget::progress_bar(0.0..=120.0, ctx.fps)
                    .width(Fill),
            ]
            .spacing(4),
        );

        // Status card — compact
        let status_card = card(
            "Status",
            column![
                info_row(
                    "Daemon",
                    if ctx.daemon_connected { "Connected" } else { "Disconnected" },
                ),
                info_row("State", if ctx.paused { "Paused" } else { "Running" }),
                info_row("Info", ctx.status_text),
            ]
            .spacing(3),
        );

        // Active shade — inline
        let shade_card = if let Some(shade) = ctx.loaded_shade {
            let short = shade.rsplit('/').next().unwrap_or(shade);
            card(
                "Active",
                column![
                    text(short).size(12),
                    text(shade).size(10),
                    btn_danger("Unload", Message::UnloadShadeClicked),
                ]
                .spacing(2),
            )
        } else {
            card(
                "Active",
                column![
                    text("No wallpaper loaded").size(12),
                ]
                .spacing(2),
            )
        };

        // Controls — stacked for any width
        let pause_resume = if ctx.paused {
            btn_primary("Resume", Message::ResumeClicked)
        } else {
            btn_secondary("Pause", Message::PauseClicked)
        };

        let controls = card(
            "Controls",
            column![
                btn_primary("Load Shade", Message::LoadShadeClicked),
                pause_resume,
                btn_danger("Shutdown", Message::ShutdownClicked),
            ]
            .spacing(4),
        );

        // Event log — fills remaining space
        let log_content: Element<Message> = if ctx.log_messages.is_empty() {
            text("No events yet.").size(11).into()
        } else {
            let items: Vec<Element<Message>> = ctx
                .log_messages
                .iter()
                .rev()
                .take(30)
                .map(|m| text(m.as_str()).size(10).into())
                .collect();
            column(items).spacing(2).into()
        };
        let log = card(
            "Log",
            scrollable(log_content).height(Fill),
        );

        scrollable(
            column![fps_card, status_card, shade_card, controls, log]
                .spacing(8)
                .padding(8)
                .width(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }
}

// ---------------------------------------------------------------------------
// Reusable UI helpers (shared across panels)
// ---------------------------------------------------------------------------

/// A rounded card container with a title.
pub fn card<'a>(title: &'a str, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(
        column![
            text(title).size(12),
            Space::with_height(4),
            content.into(),
        ],
    )
    .width(Fill)
    .padding(10)
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
pub fn info_row<'a>(label: &str, value: &str) -> Element<'a, Message> {
    row![
        text(format!("{}:", label)).size(11),
        text(value.to_string()).size(11),
    ]
    .spacing(4)
    .into()
}

/// Primary action button.
pub fn btn_primary<'a>(label: &'a str, msg: Message) -> Element<'a, Message> {
    button(text(label).size(11))
        .padding(Padding::from([4, 10]))
        .on_press(msg)
        .style(|theme: &Theme, status| {
            let p = theme.extended_palette();
            match status {
                button::Status::Active | button::Status::Pressed => button::Style {
                    background: Some(p.primary.base.color.into()),
                    text_color: p.primary.base.text,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
                _ => button::primary(theme, status),
            }
        })
        .into()
}

/// Secondary action button.
pub fn btn_secondary<'a>(label: &'a str, msg: Message) -> Element<'a, Message> {
    button(text(label).size(11))
        .padding(Padding::from([4, 10]))
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
                        .color(p.background.base.text),
                    ..Default::default()
                },
                _ => button::secondary(theme, status),
            }
        })
        .into()
}

/// Danger/destructive action button.
pub fn btn_danger<'a>(label: &'a str, msg: Message) -> Element<'a, Message> {
    button(text(label).size(11))
        .padding(Padding::from([4, 10]))
        .on_press(msg)
        .style(|theme: &Theme, status| {
            let p = theme.extended_palette();
            match status {
                button::Status::Active | button::Status::Pressed => button::Style {
                    background: Some(p.danger.base.color.into()),
                    text_color: p.danger.base.text,
                    border: Border::default().rounded(6),
                    ..Default::default()
                },
                _ => button::danger(theme, status),
            }
        })
        .into()
}
