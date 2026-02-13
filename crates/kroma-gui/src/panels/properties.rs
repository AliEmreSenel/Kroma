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
        let cfg = ctx.shade_config;

        // ── Metadata ───────────────────────────────────────────────────
        let meta = card(
            "Metadata",
            column![
                text_input("Shader name", &cfg.meta.name)
                    .on_input(Message::ShadeMetaName)
                    .size(12),
                text_input("Author", &cfg.meta.author)
                    .on_input(Message::ShadeMetaAuthor)
                    .size(12),
                text_input("Version", &cfg.meta.version)
                    .on_input(Message::ShadeMetaVersion)
                    .size(12),
                text_input("Description", &cfg.meta.description)
                    .on_input(Message::ShadeMetaDescription)
                    .size(12),
            ]
            .spacing(4),
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
                .size(12),
                checkbox("Pause offscreen", cfg.rendering.pause_offscreen)
                    .on_toggle(Message::ShadePauseOffscreen)
                    .size(14)
                    .text_size(12),
                checkbox("Pause on fullscreen", cfg.rendering.pause_fullscreen)
                    .on_toggle(Message::ShadePauseFullscreen)
                    .size(14)
                    .text_size(12),
            ]
            .spacing(4),
        );

        // ── Audio ──────────────────────────────────────────────────────
        let audio = card(
            "Audio",
            column![
                checkbox("Enable audio", cfg.audio.enabled)
                    .on_toggle(Message::ShadeAudioEnabled)
                    .size(14)
                    .text_size(12),
                text_input("Source: desktop / mic / device", &cfg.audio.source)
                    .on_input(Message::ShadeAudioSource)
                    .size(12),
            ]
            .spacing(4),
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
                        button(text("\u{2716}").size(10))
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
                    btn_secondary("+ Add", Message::ShadeAddUniform),
                ]
                .spacing(4),
            ]
            .spacing(4)
            .into(),
        );
        let uniforms = card("Uniforms", column(uniform_items).spacing(4));

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
                        text(format!("{} [{}]", label, t.ty)).size(11).width(Fill),
                        button(text("\u{2716}").size(10))
                            .on_press(Message::ShadeRemoveTexture(name))
                            .padding(Padding::from([2, 6])),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center)
                    .into(),
                );
                if src != "—" {
                    texture_items.push(
                        text(format!("  src: {}", src)).size(10).into(),
                    );
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
                    btn_secondary("+ Add", Message::ShadeAddTexture),
                ]
                .spacing(4),
            ]
            .spacing(4)
            .into(),
        );
        let textures = card("Textures", column(texture_items).spacing(4));

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
        );

        scrollable(
            column![meta, render, audio, uniforms, textures, toml_preview]
                .spacing(8)
                .padding(8)
                .width(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }
}