//! Dashboard panel — daemon status, FPS, quick actions, event log.

use iced::widget::{button, column, container, progress_bar, row, scrollable, text, Space};
use iced::{Border, Element, Fill, Padding, Theme};

use crate::panels::{AppContext, Panel};
use crate::theme::ThemeTokens;
use crate::Message;

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
        let t = ctx.tokens;
        // FPS card — compact
        let fps_display = format!("{:.0}", ctx.fps);
        let fps_card = card(
            "Performance",
            column![
                text(format!("FPS: {}", fps_display)).size(t.font_size_lg),
                progress_bar(0.0..=120.0, ctx.fps).length(Fill),
            ]
            .spacing(t.spacing_sm),
            t,
        );

        // Status card — compact
        let status_card = card(
            "Status",
            column![
                info_row(
                    "Daemon",
                    if ctx.daemon_connected {
                        "Connected"
                    } else {
                        "Disconnected"
                    },
                ),
                info_row("State", if ctx.paused { "Paused" } else { "Running" }),
                info_row("Info", ctx.status_text),
            ]
            .spacing(t.spacing_sm),
            t,
        );

        // Active shade — inline
        let shade_card = if let Some(shade) = ctx.loaded_shade {
            let short = shade.rsplit('/').next().unwrap_or(shade);
            card(
                "Active",
                column![
                    text(short).size(12),
                    text(shade).size(10),
                    btn_danger("Unload", Message::UnloadShadeClicked, t),
                ]
                .spacing(2),
                t,
            )
        } else {
            card(
                "Active",
                column![text("No wallpaper loaded").size(12),].spacing(2),
                t,
            )
        };

        // Controls — stacked for any width
        let pause_resume = if ctx.paused {
            btn_primary("Resume", Message::ResumeClicked, t)
        } else {
            btn_secondary("Pause", Message::PauseClicked, t)
        };

        let controls = card(
            "Controls",
            column![
                btn_primary("Load Shade", Message::LoadShadeClicked, t),
                pause_resume,
                btn_danger("Shutdown", Message::ShutdownClicked, t),
            ]
            .spacing(t.spacing_sm),
            t,
        );

        // Event log — fills remaining space
        let log_content: Element<Message> = if ctx.log_messages.is_empty() {
            text("No events yet.").size(t.font_size_sm).into()
        } else {
            let items: Vec<Element<Message>> = ctx
                .log_messages
                .iter()
                .rev()
                .take(30)
                .map(|m| text(m.as_str()).size(10).into())
                .collect();
            column(items).spacing(t.spacing_xs).into()
        };
        let log = card("Log", scrollable(log_content).height(Fill), t);

        scrollable(
            column![fps_card, status_card, shade_card, controls, log]
                .spacing(t.spacing_md)
                .padding(t.spacing_md)
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
pub fn card<'a>(
    title: &'a str,
    content: impl Into<Element<'a, Message>>,
    tokens: &ThemeTokens,
) -> Element<'a, Message> {
    let bg = tokens.bg_secondary;
    let border_color = tokens.border_default;
    let radius = tokens.border_radius;
    let title_size = tokens.font_size_sm;
    let pad = tokens.spacing_md;
    container(column![
        text(title).size(title_size),
        Space::new().height(tokens.spacing_sm),
        content.into(),
    ])
    .width(Fill)
    .padding(pad)
    .style(move |_theme: &Theme| container::Style {
        background: Some(bg.into()),
        border: Border::default()
            .rounded(radius)
            .width(1)
            .color(border_color),
        ..Default::default()
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
pub fn btn_primary<'a>(label: &'a str, msg: Message, tokens: &ThemeTokens) -> Element<'a, Message> {
    let bg = tokens.button_primary;
    let text_col = tokens.text_primary;
    let radius = tokens.border_radius;
    let pressed_bg = tokens.button_pressed;
    let hover_bg = tokens.button_hover;
    button(text(label).size(tokens.font_size_sm))
        .padding(Padding::from([4, 10]))
        .on_press(msg)
        .style(move |_theme: &Theme, status| match status {
            button::Status::Active => button::Style {
                background: Some(bg.into()),
                text_color: text_col,
                border: Border::default().rounded(radius),
                ..Default::default()
            },
            button::Status::Hovered => button::Style {
                background: Some(hover_bg.into()),
                text_color: text_col,
                border: Border::default().rounded(radius),
                ..Default::default()
            },
            button::Status::Pressed => button::Style {
                background: Some(pressed_bg.into()),
                text_color: text_col,
                border: Border::default().rounded(radius),
                ..Default::default()
            },
            _ => button::Style {
                background: Some(bg.into()),
                text_color: text_col,
                border: Border::default().rounded(radius),
                ..Default::default()
            },
        })
        .into()
}

/// Secondary action button.
pub fn btn_secondary<'a>(
    label: &'a str,
    msg: Message,
    tokens: &ThemeTokens,
) -> Element<'a, Message> {
    let bg = tokens.bg_tertiary;
    let text_col = tokens.text_primary;
    let border_col = tokens.border_default;
    let radius = tokens.border_radius;
    let hover_bg = tokens.bg_secondary;
    let pressed_bg = tokens.button_pressed;
    button(text(label).size(tokens.font_size_sm))
        .padding(Padding::from([4, 10]))
        .on_press(msg)
        .style(move |_theme: &Theme, status| match status {
            button::Status::Hovered => button::Style {
                background: Some(hover_bg.into()),
                text_color: text_col,
                border: Border::default().rounded(radius).width(1).color(border_col),
                ..Default::default()
            },
            button::Status::Pressed => button::Style {
                background: Some(pressed_bg.into()),
                text_color: text_col,
                border: Border::default().rounded(radius).width(1).color(border_col),
                ..Default::default()
            },
            _ => button::Style {
                background: Some(bg.into()),
                text_color: text_col,
                border: Border::default().rounded(radius).width(1).color(border_col),
                ..Default::default()
            },
        })
        .into()
}

/// Danger/destructive action button.
pub fn btn_danger<'a>(label: &'a str, msg: Message, tokens: &ThemeTokens) -> Element<'a, Message> {
    let bg = tokens.error;
    let text_col = tokens.text_primary;
    let radius = tokens.border_radius;
    let pressed_bg = tokens.button_pressed;
    button(text(label).size(tokens.font_size_sm))
        .padding(Padding::from([4, 10]))
        .on_press(msg)
        .style(move |_theme: &Theme, status| match status {
            button::Status::Pressed => button::Style {
                background: Some(pressed_bg.into()),
                text_color: text_col,
                border: Border::default().rounded(radius),
                ..Default::default()
            },
            _ => button::Style {
                background: Some(bg.into()),
                text_color: text_col,
                border: Border::default().rounded(radius),
                ..Default::default()
            },
        })
        .into()
}
