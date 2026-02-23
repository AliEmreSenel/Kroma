//! Settings panel — theme, directories, API keys.

use iced::widget::{button, column, text, text_input};
use iced::{Border, Element, Fill, Padding, Theme};

use crate::Message;
use crate::panels::dashboard::{card, info_row};
use crate::panels::{AppContext, Panel};
use crate::theme::KromaThemeId;

pub struct SettingsPanel;

impl SettingsPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for SettingsPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let t = ctx.tokens;
        // ── Theme selector ─────────────────────────────────────────────────────
        let btn_bg = t.bg_tertiary;
        let btn_text = t.text_primary;
        let btn_border = t.border_default;
        let mut theme_buttons: Vec<iced::Element<'_, Message>> = Vec::new();
        for &theme_id in KromaThemeId::all() {
            let btn = button(text(theme_id.name()).size(t.font_size_sm))
                .width(Fill)
                .padding(Padding::from([4, 10]))
                .on_press(Message::ThemeChanged(theme_id))
                .style(move |_theme: &Theme, _status| button::Style {
                    background: Some(btn_bg.into()),
                    text_color: btn_text,
                    border: Border::default().rounded(4).width(1).color(btn_border),
                    ..Default::default()
                });
            theme_buttons.push(btn.into());
        }
        let theme_card = card("Theme", column(theme_buttons).spacing(t.spacing_sm), t);

        // ── API key ───────────────────────────────────────────────────
        let api_card = card(
            "Shadertoy API",
            column![
                text("API key (optional):").size(t.font_size_sm),
                text_input("Shadertoy API key...", ctx.api_key)
                    .on_input(Message::ApiKeyChanged)
                    .size(t.font_size_sm)
                    .padding(6)
                    .width(Fill),
            ]
            .spacing(t.spacing_sm),
            t,
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
                info_row(
                    "IPC",
                    &kroma_shared::ipc::socket_path().display().to_string()
                ),
            ]
            .spacing(t.spacing_sm),
            t,
        );

        // ── About ─────────────────────────────────────────────────────
        let about_card = card(
            "About",
            column![
                text(format!("Kroma v{}", env!("CARGO_PKG_VERSION"))).size(t.font_size_sm),
                text("Modular wallpaper engine for Linux").size(10),
                info_row("Renderer", "wgpu (Vulkan)"),
                info_row("Shaders", "GLSL 450 -> WGSL"),
                info_row("Audio", "cpal + rustfft"),
            ]
            .spacing(t.spacing_sm),
            t,
        );

        iced::widget::scrollable(
            column![theme_card, api_card, dirs_card, about_card]
                .spacing(t.spacing_md)
                .padding(t.spacing_md)
                .width(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }
}
