//! Properties panel — shade config.toml settings form & inspector.

use iced::widget::{button, checkbox, column, container, row, scrollable, text, text_input};
use iced::{Element, Fill, Length, Padding};

use crate::Message;
use crate::panels::dashboard::{btn_secondary, card};
use crate::panels::{AppContext, Panel};

pub struct PropertiesPanel;

impl PropertiesPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for PropertiesPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let t = ctx.tokens;
        let cfg = ctx.shade_config;

        // ── Metadata ───────────────────────────────────────────────────
        let meta = card(
            "Metadata",
            column![
                text_input("Shader name", &cfg.meta.name)
                    .on_input(Message::ShadeMetaName)
                    .size(t.font_size_sm),
                text_input("Author", &cfg.meta.author)
                    .on_input(Message::ShadeMetaAuthor)
                    .size(t.font_size_sm),
                text_input("Version", &cfg.meta.version)
                    .on_input(Message::ShadeMetaVersion)
                    .size(t.font_size_sm),
                text_input("Description", &cfg.meta.description)
                    .on_input(Message::ShadeMetaDescription)
                    .size(t.font_size_sm),
            ]
            .spacing(t.spacing_sm),
            t,
        );

        // ── Rendering ──────────────────────────────────────────────────
        let render = card(
            "Rendering",
            column![
                text_input(
                    "Target FPS (0=monitor)",
                    &cfg.rendering.target_fps.to_string(),
                )
                .on_input(Message::ShadeTargetFps)
                .size(t.font_size_sm),
                checkbox(cfg.rendering.pause_offscreen)
                    .label("Pause offscreen")
                    .on_toggle(Message::ShadePauseOffscreen)
                    .size(t.font_size_md)
                    .text_size(t.font_size_sm),
                checkbox(cfg.rendering.pause_fullscreen)
                    .label("Pause on fullscreen")
                    .on_toggle(Message::ShadePauseFullscreen)
                    .size(t.font_size_md)
                    .text_size(t.font_size_sm),
            ]
            .spacing(t.spacing_sm),
            t,
        );

        // ── Audio ──────────────────────────────────────────────────────
        let audio = card(
            "Audio",
            column![
                text("Audio is now configured as a texture.").size(t.font_size_sm),
                text("Add an 'audio_spectrum' texture in the Textures section.")
                    .size(t.font_size_sm),
            ]
            .spacing(t.spacing_sm),
            t,
        );

        // ── Uniforms ───────────────────────────────────────────────────
        let mut uniform_items: Vec<Element<'_, Message>> = Vec::new();
        let mut uniform_keys: Vec<String> = cfg.uniforms.keys().cloned().collect();
        uniform_keys.sort();
        for name in uniform_keys {
            if let Some(u) = cfg.uniforms.get(&name) {
                let ty = u.ty.clone();
                let label = name.clone();
                uniform_items.push(
                    row![
                        text(format!("{} ({})", label, ty)).size(11).width(Fill),
                        button(text("X").size(10))
                            .on_press(Message::ShadeRemoveUniform(name))
                            .padding(Padding::from([2, 6])),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center)
                    .into(),
                );
            }
        }
        uniform_items.push(
            column![
                text_input("name", ctx.shade_new_uniform_name)
                    .on_input(Message::ShadeNewUniformName)
                    .size(11),
                row![
                    text_input("type", ctx.shade_new_uniform_type)
                        .on_input(Message::ShadeNewUniformType)
                        .size(11),
                    btn_secondary("+ Add", Message::ShadeAddUniform, t),
                ]
                .spacing(4),
            ]
            .spacing(4)
            .into(),
        );
        let uniforms = card("Uniforms", column(uniform_items).spacing(4), t);

        // ── Textures ───────────────────────────────────────────────────
        let mut texture_items: Vec<Element<'_, Message>> = Vec::new();
        let mut texture_keys: Vec<String> = cfg.textures.keys().cloned().collect();
        texture_keys.sort();
        for name in texture_keys {
            if let Some(t) = cfg.textures.get(&name) {
                let src = t.source.clone().unwrap_or_else(|| "—".into());
                let label = name.clone();
                texture_items.push(
                    row![
                        text(format!("{} [{}]", label, t.ty.as_str()))
                            .size(11)
                            .width(Fill),
                        button(text("X").size(10))
                            .on_press(Message::ShadeRemoveTexture(name))
                            .padding(Padding::from([2, 6])),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center)
                    .into(),
                );
                if src != "—" {
                    texture_items.push(text(format!("  src: {}", src)).size(10).into());
                }
            }
        }
        texture_items.push(
            column![
                text_input("channel", ctx.shade_new_texture_name)
                    .on_input(Message::ShadeNewTextureName)
                    .size(11),
                row![
                    text_input("type", ctx.shade_new_texture_type)
                        .on_input(Message::ShadeNewTextureType)
                        .size(11),
                    btn_secondary("+ Add", Message::ShadeAddTexture, t),
                ]
                .spacing(4),
            ]
            .spacing(4)
            .into(),
        );
        let textures = card("Textures", column(texture_items).spacing(4), t);

        // ── TOML preview ───────────────────────────────────────────────
        let toml_preview = card(
            "config.toml",
            container(
                scrollable(
                    iced::widget::text_editor(ctx.shade_config_toml)
                        .on_action(Message::ShadeConfigToml),
                )
                .height(Length::Fill),
            )
            .width(Fill)
            .height(Length::FillPortion(1)),
            t,
        );

        scrollable(
            column![meta, render, audio, uniforms, textures, toml_preview]
                .spacing(t.spacing_md)
                .padding(t.spacing_md)
                .width(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }
}
