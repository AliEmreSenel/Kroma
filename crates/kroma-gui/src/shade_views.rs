//! Shade Package Editor view methods (extracted from main.rs).

use iced::widget::{
    button, column, container, horizontal_space, row, scrollable, text, text_input, Space,
};
use iced::{color, Border, Element, Fill, Padding, Theme};

use crate::editor;
use crate::utils;
use crate::Message;

use super::KromaApp;

impl KromaApp {
    // -----------------------------------------------------------------------
    // Shade Package Editor tab
    // -----------------------------------------------------------------------

    pub(crate) fn view_shade_edit(&self) -> Element<'_, Message> {
        // Header toolbar
        let header = container(
            row![
                text("Shade Package Editor").size(18),
                horizontal_space(),
                self.btn_secondary("New", Message::ShadeNew),
                self.btn_secondary("Open", Message::ShadeOpen),
                self.btn_primary("Save .shade", Message::ShadeSave),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([8, 12]))
        .width(Fill)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                ..Default::default()
            }
        });

        let file_tree = self.view_shade_file_tree();
        let editor_panel = self.view_shade_editor_panel();

        column![
            header,
            row![file_tree, editor_panel].width(Fill).height(Fill),
        ]
        .width(Fill)
        .height(Fill)
        .into()
    }

    /// Left sidebar file tree for the shade package editor.
    fn view_shade_file_tree(&self) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = Vec::new();
        items.push(text(format!("\u{1F4E6} {}", self.shade_config.meta.name)).size(14).into());
        items.push(Space::with_height(4).into());
        items.push(self.shade_tree_btn("\u{1F4C4} config.toml", "config.toml"));
        items.push(self.shade_tree_btn("\u{1F4C4} shader.frag", "shader.frag"));
        items.push(Space::with_height(6).into());
        items.push(text("\u{1F4C1} assets/").size(12).into());
        if let Some(ref pkg) = self.shade_package {
            let mut sorted: Vec<&(String, Vec<u8>)> = pkg.assets.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, data) in sorted {
                let short = name.rsplit('/').next().unwrap_or(name);
                let icon = utils::shade_file_icon(short);
                let sz = if data.len() > 1_048_576 {
                    format!(" ({:.1}MB)", data.len() as f64 / 1_048_576.0)
                } else if data.len() > 1024 {
                    format!(" ({:.1}KB)", data.len() as f64 / 1024.0)
                } else {
                    format!(" ({}B)", data.len())
                };
                let is_sel = self.shade_selected_file.as_deref() == Some(name.as_str());
                let name_c = name.clone();
                let remove_c = name.clone();
                items.push(
                    row![
                        button(text(format!("  {} {}{}", icon, short, sz)).size(11))
                            .width(Fill)
                            .padding(Padding::from([3, 6]))
                            .on_press(Message::ShadeSelectFile(name_c))
                            .style(move |theme: &Theme, status| {
                                let p = theme.extended_palette();
                                if is_sel {
                                    button::Style {
                                        background: Some(p.primary.weak.color.into()),
                                        text_color: p.primary.weak.text,
                                        border: Border::default().rounded(4),
                                        ..Default::default()
                                    }
                                } else {
                                    match status {
                                        button::Status::Hovered => button::Style {
                                            background: Some(p.background.weak.color.into()),
                                            text_color: p.background.base.text,
                                            border: Border::default().rounded(4),
                                            ..Default::default()
                                        },
                                        _ => button::Style {
                                            background: None,
                                            text_color: p.background.base.text,
                                            border: Border::default().rounded(4),
                                            ..Default::default()
                                        },
                                    }
                                }
                            }),
                        button(text("\u{2716}").size(10))
                            .padding(Padding::from([2, 6]))
                            .on_press(Message::ShadeRemoveAsset(remove_c))
                            .style(|_theme: &Theme, _status| button::Style {
                                background: None,
                                text_color: iced::Color::from_rgb(0.8, 0.3, 0.3),
                                border: Border::default().rounded(4),
                                ..Default::default()
                            }),
                    ]
                    .spacing(2)
                    .align_y(iced::Alignment::Center)
                    .into(),
                );
            }
        }
        items.push(Space::with_height(8).into());
        items.push(
            row![
                self.btn_secondary("+ Asset", Message::ShadeAddAsset),
                self.btn_secondary("+ GLSL", Message::ShadeAddGlslFile),
            ]
            .spacing(4)
            .into(),
        );
        container(
            scrollable(column(items).spacing(2)).height(Fill),
        )
        .width(200)
        .height(Fill)
        .padding(8)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                border: Border {
                    color: iced::Color { a: 0.1, ..p.background.base.text },
                    width: 1.0,
                    radius: 0.into(),
                },
                ..Default::default()
            }
        })
        .into()
    }

    /// File tree button for core shade files.
    fn shade_tree_btn<'a>(&self, label: &'a str, file_id: &'a str) -> Element<'a, Message> {
        let is_sel = self.shade_selected_file.as_deref() == Some(file_id);
        let id_owned = file_id.to_string();
        button(text(label).size(12))
            .width(Fill)
            .padding(Padding::from([4, 8]))
            .on_press(Message::ShadeSelectFile(id_owned))
            .style(move |theme: &Theme, status| {
                let p = theme.extended_palette();
                if is_sel {
                    button::Style {
                        background: Some(p.primary.weak.color.into()),
                        text_color: p.primary.weak.text,
                        border: Border::default().rounded(4),
                        ..Default::default()
                    }
                } else {
                    match status {
                        button::Status::Hovered => button::Style {
                            background: Some(p.background.weak.color.into()),
                            text_color: p.background.base.text,
                            border: Border::default().rounded(4),
                            ..Default::default()
                        },
                        _ => button::Style {
                            background: None,
                            text_color: p.background.base.text,
                            border: Border::default().rounded(4),
                            ..Default::default()
                        },
                    }
                }
            })
            .into()
    }

    /// Right panel: context-sensitive editor for the selected file.
    fn view_shade_editor_panel(&self) -> Element<'_, Message> {
        let selected = self.shade_selected_file.as_deref().unwrap_or("config.toml");
        let mode_bar = self.view_shade_mode_bar(selected);
        let editor_content: Element<'_, Message> = match selected {
            "config.toml" => {
                if self.shade_edit_mode == "code" {
                    container(
                        scrollable(
                            iced::widget::text_editor(&self.shade_config_toml)
                                .on_action(Message::ShadeConfigToml)
                        )
                        .width(Fill)
                        .height(Fill),
                    )
                    .width(Fill)
                    .height(Fill)
                    .padding(4)
                    .into()
                } else {
                    self.view_shade_settings_form()
                }
            }
            "shader.frag" => {
                if self.shade_edit_mode == "preview" {
                    let src = self.shade_shader_content.text();
                    let display = if src.is_empty() { "// No shader source".to_string() } else { src };
                    container(
                        scrollable(text(display).size(12))
                            .width(Fill)
                            .height(Fill),
                    )
                    .width(Fill)
                    .height(Fill)
                    .padding(8)
                    .into()
                } else if self.shade_edit_mode == "nodes" {
                    // Node graph editor for shader.frag
                    let canvas = editor::canvas::graph_canvas(&self.shade_graph, &self.shade_graph_canvas)
                        .map(Message::ShadeGraphMsg);
                    let palette = self.view_shade_node_palette();
                    row![palette, canvas]
                        .width(Fill)
                        .height(Fill)
                        .into()
                } else {
                    container(
                        scrollable(
                            iced::widget::text_editor(&self.shade_shader_content)
                                .on_action(Message::ShadeShaderChanged)
                        )
                        .width(Fill)
                        .height(Fill),
                    )
                    .width(Fill)
                    .height(Fill)
                    .padding(4)
                    .into()
                }
            }
            other => self.view_shade_asset(other),
        };
        container(
            column![mode_bar, editor_content]
                .spacing(0)
                .width(Fill)
                .height(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }

    /// Mode toggle bar for shade editor.
    fn view_shade_mode_bar(&self, selected: &str) -> Element<'_, Message> {
        let mut items: Vec<Element<'_, Message>> = Vec::new();
        items.push(text(format!("Editing: {}", selected)).size(13).into());
        items.push(horizontal_space().into());
        match selected {
            "config.toml" => {
                if self.shade_edit_mode != "code" {
                    items.push(self.btn_primary("Settings", Message::ShadeEditMode("settings".into())));
                    items.push(self.btn_secondary("Code", Message::ShadeEditMode("code".into())));
                } else {
                    items.push(self.btn_secondary("Settings", Message::ShadeEditMode("settings".into())));
                    items.push(self.btn_primary("Code", Message::ShadeEditMode("code".into())));
                }
            }
            "shader.frag" => {
                let modes = vec![("Code", "code"), ("Nodes", "nodes"), ("Preview", "preview")];
                for (label, mode) in modes {
                    if self.shade_edit_mode == mode {
                        items.push(self.btn_primary(label, Message::ShadeEditMode(mode.into())));
                    } else {
                        items.push(self.btn_secondary(label, Message::ShadeEditMode(mode.into())));
                    }
                }
                items.push(Space::with_width(8).into());
                if self.shade_edit_mode == "nodes" {
                    items.push(self.btn_secondary("Compile \u{2192} GLSL", Message::ShadeCompileGraph));
                    items.push(self.btn_secondary("\u{2190} Parse GLSL", Message::ShadeParseToNodes));
                }
                items.push(self.btn_primary("Send to Daemon", Message::ShadeSendToDaemon));
            }
            _ => {
                items.push(text("Asset Viewer").size(12).into());
            }
        }
        container(
            row(items).spacing(6).align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([6, 12]))
        .width(Fill)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.weak.color.into()),
                ..Default::default()
            }
        })
        .into()
    }

    /// View for an asset file in the shade editor.
    fn view_shade_asset(&self, asset_name: &str) -> Element<'_, Message> {
        let short = asset_name.rsplit('/').next().unwrap_or(asset_name);
        let ext = short.rsplit('.').next().unwrap_or("").to_lowercase();
        if let Some(ref pkg) = self.shade_package {
            if let Some((_name, data)) = pkg.assets.iter().find(|(n, _)| n == asset_name) {
                let size = data.len();
                let size_str = if size > 1_048_576 {
                    format!("{:.1} MB", size as f64 / 1_048_576.0)
                } else if size > 1024 {
                    format!("{:.1} KB", size as f64 / 1024.0)
                } else {
                    format!("{} bytes", size)
                };
                let content: Element<'_, Message> = match ext.as_str() {
                    "glsl" | "frag" | "vert" => {
                        let source = String::from_utf8_lossy(data);
                        scrollable(text(source.to_string()).size(12))
                            .width(Fill)
                            .height(Fill)
                            .into()
                    }
                    "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" => {
                        column![
                            text(format!("\u{1F5BC} Image Asset: {}", short)).size(16),
                            text(format!("Size: {}", size_str)).size(13),
                            Space::with_height(8),
                            text("Image preview not available in editor").size(11),
                        ]
                        .spacing(6)
                        .into()
                    }
                    "mp4" | "webm" | "avi" | "mkv" => {
                        column![
                            text(format!("\u{1F3AC} Video Asset: {}", short)).size(16),
                            text(format!("Size: {}", size_str)).size(13),
                            Space::with_height(8),
                            text("Video preview not available in editor").size(11),
                        ]
                        .spacing(6)
                        .into()
                    }
                    "ttf" | "otf" | "woff" | "woff2" => {
                        column![
                            text(format!("\u{1F524} Font Asset: {}", short)).size(16),
                            text(format!("Size: {}", size_str)).size(13),
                            Space::with_height(8),
                            text("Font preview not available in editor").size(11),
                            text("Use this font in textures with type = \"font\"").size(11),
                        ]
                        .spacing(6)
                        .into()
                    }
                    "mp3" | "wav" | "ogg" | "flac" => {
                        column![
                            text(format!("\u{1F3B5} Audio Asset: {}", short)).size(16),
                            text(format!("Size: {}", size_str)).size(13),
                        ]
                        .spacing(6)
                        .into()
                    }
                    _ => {
                        column![
                            text(format!("\u{1F4C4} File: {}", short)).size(16),
                            text(format!("Size: {}", size_str)).size(13),
                        ]
                        .spacing(6)
                        .into()
                    }
                };
                return container(content)
                    .width(Fill)
                    .height(Fill)
                    .padding(16)
                    .into();
            }
        }
        container(
            text(format!("Asset not found: {}", asset_name)).size(13),
        )
        .width(Fill)
        .height(Fill)
        .padding(16)
        .into()
    }

    /// Settings form for config.toml (extracted from original shade editor).
    pub(crate) fn view_shade_settings_form(&self) -> Element<'_, Message> {
        // Meta section
        let meta = self.card(
            "Metadata",
            column![
                text_input("Shader name", &self.shade_config.meta.name)
                    .on_input(Message::ShadeMetaName),
                text_input("Author", &self.shade_config.meta.author)
                    .on_input(Message::ShadeMetaAuthor),
                row![
                    text_input("Version", &self.shade_config.meta.version)
                        .on_input(Message::ShadeMetaVersion)
                        .width(100),
                    text_input("Description", &self.shade_config.meta.description)
                        .on_input(Message::ShadeMetaDescription),
                ]
                .spacing(8),
            ]
            .spacing(8),
        );

        // Rendering section
        let render = self.card(
            "Rendering",
            column![
                row![
                    text("Target FPS:").size(13).width(120),
                    text_input("0 = monitor rate", &self.shade_config.rendering.target_fps.to_string())
                        .on_input(Message::ShadeTargetFps)
                        .width(120),
                    text("(0 = match monitor)").size(11),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
                row![
                    iced::widget::checkbox("Pause when offscreen", self.shade_config.rendering.pause_offscreen)
                        .on_toggle(Message::ShadePauseOffscreen),
                    iced::widget::checkbox("Pause on fullscreen app", self.shade_config.rendering.pause_fullscreen)
                        .on_toggle(Message::ShadePauseFullscreen),
                ]
                .spacing(16),
            ]
            .spacing(8),
        );

        // Audio section
        let audio = self.card(
            "Audio",
            column![
                iced::widget::checkbox("Enable audio capture", self.shade_config.audio.enabled)
                    .on_toggle(Message::ShadeAudioEnabled),
                row![
                    text("Source:").size(13).width(80),
                    text_input("desktop / microphone / device name", &self.shade_config.audio.source)
                        .on_input(Message::ShadeAudioSource),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
                text("Use 'desktop' for system audio, 'microphone' for mic, or a device name.").size(11),
            ]
            .spacing(8),
        );

        // Uniforms section
        let mut uniform_items: Vec<Element<'_, Message>> = Vec::new();
        let mut uniform_keys: Vec<String> = self.shade_config.uniforms.keys().cloned().collect();
        uniform_keys.sort();
        for name in uniform_keys {
            if let Some(u) = self.shade_config.uniforms.get(&name) {
                let range_str = match (u.min, u.max) {
                    (Some(lo), Some(hi)) => format!("[{} .. {}]", lo, hi),
                    _ => String::new(),
                };
                let ty = u.ty.clone();
                let label = name.clone();
                uniform_items.push(
                    row![
                        text(label).size(13).width(140),
                        text(ty).size(12).width(60),
                        text(range_str).size(11).width(Fill),
                        button(text("Remove").size(11))
                            .on_press(Message::ShadeRemoveUniform(name))
                            .padding(Padding::from([2, 8])),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center)
                    .into(),
                );
            }
        }
        uniform_items.push(
            row![
                text_input("name", &self.shade_new_uniform_name)
                    .on_input(Message::ShadeNewUniformName)
                    .width(140),
                text_input("type", &self.shade_new_uniform_type)
                    .on_input(Message::ShadeNewUniformType)
                    .width(80),
                self.btn_secondary("Add Uniform", Message::ShadeAddUniform),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into(),
        );
        let uniforms = self.card(
            "Uniforms",
            column(uniform_items).spacing(6),
        );

        // Textures section
        let mut texture_items: Vec<Element<'_, Message>> = Vec::new();
        let mut texture_keys: Vec<String> = self.shade_config.textures.keys().cloned().collect();
        texture_keys.sort();
        for name in texture_keys {
            if let Some(t) = self.shade_config.textures.get(&name) {
                let src = t.source.clone().unwrap_or_else(|| "none".into());
                let ty = t.ty.clone();
                let label = name.clone();
                texture_items.push(
                    row![
                        text(label).size(13).width(120),
                        text(ty).size(12).width(60),
                        text(src).size(11).width(Fill),
                        button(text("Remove").size(11))
                            .on_press(Message::ShadeRemoveTexture(name))
                            .padding(Padding::from([2, 8])),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center)
                    .into(),
                );
            }
        }
        texture_items.push(
            row![
                text_input("channel name", &self.shade_new_texture_name)
                    .on_input(Message::ShadeNewTextureName)
                    .width(120),
                text_input("type (image/video/glsl/font)", &self.shade_new_texture_type)
                    .on_input(Message::ShadeNewTextureType)
                    .width(160),
                self.btn_secondary("Add Texture", Message::ShadeAddTexture),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into(),
        );
        texture_items.push(
            text("Types: image, video, glsl (programmatic texture from shader), font (font file)").size(10).into(),
        );
        if self.shade_config.textures.values().any(|t| t.ty == "glsl") {
            texture_items.push(
                text("\u{2139} GLSL textures generate data from a shader in assets/. Set source to the .glsl filename.")
                    .size(10)
                    .color(color!(0x60a5fa))
                    .into(),
            );
        }
        if self.shade_config.textures.values().any(|t| t.ty == "font") {
            texture_items.push(
                text("\u{2139} Font textures render glyphs from .ttf/.otf files in assets/. Set source to the font file.")
                    .size(10)
                    .color(color!(0xa78bfa))
                    .into(),
            );
        }
        let textures = self.card(
            "Textures",
            column(texture_items).spacing(6),
        );

        // TOML preview
        let toml_preview = self.card(
            "config.toml Preview",
            container(
                scrollable(
                    iced::widget::text_editor(&self.shade_config_toml)
                        .on_action(Message::ShadeConfigToml)
                )
                .height(200),
            )
            .width(Fill),
        );

        scrollable(
            column![
                row![meta, render].spacing(16),
                audio,
                row![uniforms, textures].spacing(16),
                toml_preview,
            ]
            .spacing(12)
            .width(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }
}
