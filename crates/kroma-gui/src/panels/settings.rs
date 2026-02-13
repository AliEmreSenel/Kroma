//! Settings panel — theme, directories, API keys.

use iced::widget::{button, column, text, text_input};
use iced::{Border, Element, Fill, Padding, Theme};

use crate::Message;
use crate::theme::KromaThemeId;
use crate::panels::{AppContext, Panel};
use crate::panels::dashboard::{card, info_row};

pub struct SettingsPanel;

impl SettingsPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for SettingsPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        // ── Theme selector ─────────────────────────────────────────────
        let mut theme_buttons: Vec<iced::Element<'_, Message>> = Vec::new();
        for &theme_id in KromaThemeId::all() {
            let btn = button(text(theme_id.name()).size(11))
                .width(Fill)
                .padding(Padding::from([4, 10]))
                .on_press(Message::ThemeChanged(theme_id))
                .style(move |theme: &Theme, _status| {
                    let p = theme.extended_palette();
                    button::Style {
                        background: Some(p.background.strong.color.into()),
                        text_color: p.background.base.text,
                        border: Border::default().rounded(4).width(1).color(
                            p.background.base.text,
                        ),
                        ..Default::default()
                    }
                });
            theme_buttons.push(btn.into());
        }
        let theme_card = card(
            "Theme",
            column(theme_buttons).spacing(3),
        );

        // ── API key ───────────────────────────────────────────────────
        let api_card = card(
            "Shadertoy API",
            column![
                text("API key (optional):").size(11),
                text_input("Shadertoy API key…", ctx.api_key)
                    .on_input(Message::ApiKeyChanged)
                    .size(11)
                    .padding(6)
                    .width(Fill),
            ]
            .spacing(4),
        );

        // ── Directories ───────────────────────────────────────────────
        let shader_dir = if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
            std::path::PathBuf::from(d).join("kroma/shaders")
        } else if let Some(h) = std::env::var_os("HOME") {
            std::path::PathBuf::from(h).join(".local/share/kroma/shaders")
        } else {
            std::path::PathBuf::from("./shaders")
        };
        let dirs_card = card(
            "Directories",
            column![
                info_row("Shaders", &format!("{}", shader_dir.display())),
                info_row("IPC", &kroma_shared::ipc::socket_path().display().to_string()),
            ]
            .spacing(3),
        );

        // ── About ─────────────────────────────────────────────────────
        let about_card = card(
            "About",
            column![
                text(format!(
                    "Kroma v{}",
                    env!("CARGO_PKG_VERSION")
                ))
                .size(12),
                text("Modular wallpaper engine for Linux").size(10),
                info_row("Renderer", "wgpu (Vulkan)"),
                info_row("Shaders", "GLSL 450 \u{2192} WGSL"),
                info_row("Audio", "cpal + rustfft"),
            ]
            .spacing(3),
        );

        iced::widget::scrollable(
            column![theme_card, api_card, dirs_card, about_card]
                .spacing(8)
                .padding(8)
                .width(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }
}