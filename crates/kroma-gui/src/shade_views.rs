//! Shade Package Editor view methods (extracted from main.rs).

use iced::widget::space::horizontal;
use iced::widget::{Space, button, checkbox, column, container, row, scrollable, text, text_input};
use iced::{Border, Element, Fill, Padding, Theme};

use crate::Message;
use crate::editor;
use crate::icons;
use crate::utils;

use super::KromaApp;

impl KromaApp {
    // -----------------------------------------------------------------------
    // Shade Package Editor tab
    // -----------------------------------------------------------------------

    #[allow(dead_code)]
    pub(crate) fn view_shade_edit(&self) -> Element<'_, Message> {
        // Header toolbar
        let header_bg = self.theme_tokens.bg_tertiary;
        let header = container(
            row![
                text("Shade Package Editor").size(18),
                horizontal(),
                self.btn_secondary("New", Message::ShadeNew),
                self.btn_secondary("Open", Message::ShadeOpen),
                self.btn_primary("Save .shade", Message::ShadeSave),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([8, 12]))
        .width(Fill)
        .style(move |_theme: &Theme| container::Style {
            background: Some(header_bg.into()),
            ..Default::default()
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
        let sel_bg = self.theme_tokens.bg_accent;
        let sel_text = self.theme_tokens.text_primary;
        let hover_bg = self.theme_tokens.bg_secondary;
        let text_color = self.theme_tokens.text_primary;
        let error_color = self.theme_tokens.error;
        let tree_bg = self.theme_tokens.bg_tertiary;
        let tree_border = self.theme_tokens.border_default;
        let mut items: Vec<Element<'_, Message>> = vec![
            row![
                text(icons::PACKAGE).font(icons::ICON_FONT).size(14),
                text(&self.shade_config.meta.name).size(14),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center)
            .into(),
            Space::new().height(4).into(),
            self.shade_tree_btn(icons::FILE, "config.toml", "config.toml"),
            self.shade_tree_btn(icons::CODE, "shader.frag", "shader.frag"),
            Space::new().height(6).into(),
            row![
                text(icons::FOLDER).font(icons::ICON_FONT).size(12),
                text("assets/").size(12),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center)
            .into(),
        ];
        if let Some(ref pkg) = self.shade_package {
            let mut sorted = pkg.asset_entries();
            sorted.sort_by(|a, b| a.name.cmp(&b.name));
            for entry in &sorted {
                let name = &entry.name;
                let short = name.rsplit('/').next().unwrap_or(name);
                let icon = utils::shade_file_icon(short);
                let sz = if entry.size > 1_048_576 {
                    format!(" ({:.1}MB)", entry.size as f64 / 1_048_576.0)
                } else if entry.size > 1024 {
                    format!(" ({:.1}KB)", entry.size as f64 / 1024.0)
                } else {
                    format!(" ({}B)", entry.size)
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
                            .style(move |_theme: &Theme, status| {
                                if is_sel {
                                    button::Style {
                                        background: Some(sel_bg.into()),
                                        text_color: sel_text,
                                        border: Border::default().rounded(4),
                                        ..Default::default()
                                    }
                                } else {
                                    match status {
                                        button::Status::Hovered => button::Style {
                                            background: Some(hover_bg.into()),
                                            text_color,
                                            border: Border::default().rounded(4),
                                            ..Default::default()
                                        },
                                        _ => button::Style {
                                            background: None,
                                            text_color,
                                            border: Border::default().rounded(4),
                                            ..Default::default()
                                        },
                                    }
                                }
                            }),
                        button(text("X").size(10))
                            .padding(Padding::from([2, 6]))
                            .on_press(Message::ShadeRemoveAsset(remove_c))
                            .style(move |_theme: &Theme, _status| button::Style {
                                background: None,
                                text_color: error_color,
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
        items.push(Space::new().height(8).into());
        items.push(
            row![
                self.btn_secondary("+ Asset", Message::ShadeAddAsset),
                self.btn_secondary("+ GLSL", Message::ShadeAddGlslFile),
            ]
            .spacing(4)
            .into(),
        );
        container(scrollable(column(items).spacing(2)).height(Fill))
            .width(200)
            .height(Fill)
            .padding(8)
            .style(move |_theme: &Theme| container::Style {
                background: Some(tree_bg.into()),
                border: Border {
                    color: tree_border,
                    width: 1.0,
                    radius: 0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    /// File tree button for core shade files.
    fn shade_tree_btn<'a>(
        &self,
        icon_cp: &'a str,
        label: &'a str,
        file_id: &'a str,
    ) -> Element<'a, Message> {
        let is_sel = self.shade_selected_file.as_deref() == Some(file_id);
        let id_owned = file_id.to_string();
        let sel_bg = self.theme_tokens.bg_accent;
        let sel_text = self.theme_tokens.text_primary;
        let hover_bg = self.theme_tokens.bg_secondary;
        let text_color = self.theme_tokens.text_primary;
        button(
            row![
                text(icon_cp).font(icons::ICON_FONT).size(12),
                text(label).size(12),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center),
        )
        .width(Fill)
        .padding(Padding::from([4, 8]))
        .on_press(Message::ShadeSelectFile(id_owned))
        .style(move |_theme: &Theme, status| {
            if is_sel {
                button::Style {
                    background: Some(sel_bg.into()),
                    text_color: sel_text,
                    border: Border::default().rounded(4),
                    ..Default::default()
                }
            } else {
                match status {
                    button::Status::Hovered => button::Style {
                        background: Some(hover_bg.into()),
                        text_color,
                        border: Border::default().rounded(4),
                        ..Default::default()
                    },
                    _ => button::Style {
                        background: None,
                        text_color,
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
                                .on_action(Message::ShadeConfigToml),
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
                    let display = if src.is_empty() {
                        "// No shader source".to_string()
                    } else {
                        src
                    };
                    container(scrollable(text(display).size(12)).width(Fill).height(Fill))
                        .width(Fill)
                        .height(Fill)
                        .padding(8)
                        .into()
                } else if self.shade_edit_mode == "nodes" {
                    // Node graph editor for shader.frag
                    let canvas = editor::canvas::graph_canvas(
                        &self.shade_graph,
                        &self.shade_graph_canvas,
                        &self.theme_tokens,
                    )
                    .map(Message::ShadeGraphMsg);
                    let palette = crate::panels::node_editor::build_node_palette(
                        "",
                        &self.theme_tokens,
                        |kind| {
                            Message::ShadeGraphMsg(
                                crate::editor::canvas::GraphMessage::SetPendingNode(kind),
                            )
                        },
                    );
                    row![palette, canvas].width(Fill).height(Fill).into()
                } else {
                    container(
                        scrollable(
                            iced::widget::text_editor(&self.shade_shader_content)
                                .on_action(Message::ShadeShaderChanged),
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
        let bar_bg = self.theme_tokens.bg_secondary;
        let mut items: Vec<Element<'_, Message>> = Vec::new();
        items.push(text(format!("Editing: {}", selected)).size(13).into());
        items.push(horizontal().into());
        match selected {
            "config.toml" => {
                if self.shade_edit_mode != "code" {
                    items.push(
                        self.btn_primary("Settings", Message::ShadeEditMode("settings".into())),
                    );
                    items.push(self.btn_secondary("Code", Message::ShadeEditMode("code".into())));
                } else {
                    items.push(
                        self.btn_secondary("Settings", Message::ShadeEditMode("settings".into())),
                    );
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
                items.push(Space::new().width(8).into());
                if self.shade_edit_mode == "nodes" {
                    items.push(self.btn_secondary("Compile -> GLSL", Message::ShadeCompileGraph));
                    items.push(self.btn_secondary("<- Parse GLSL", Message::ShadeParseToNodes));
                }
                items.push(self.btn_primary("Send to Daemon", Message::ShadeSendToDaemon));
            }
            _ => {
                items.push(text("Asset Viewer").size(12).into());
            }
        }
        container(row(items).spacing(6).align_y(iced::Alignment::Center))
            .padding(Padding::from([6, 12]))
            .width(Fill)
            .style(move |_theme: &Theme| container::Style {
                background: Some(bar_bg.into()),
                ..Default::default()
            })
            .into()
    }

    /// View for an asset file in the shade editor.
    fn view_shade_asset(&self, asset_name: &str) -> Element<'_, Message> {
        let short = asset_name.rsplit('/').next().unwrap_or(asset_name);
        let ext = short.rsplit('.').next().unwrap_or("").to_lowercase();
        if let Some(ref pkg) = self.shade_package
            && let Some(data) = pkg.read_asset(asset_name)
        {
            let size = data.len();
            let data = data.as_slice();
            let size_str = if size > 1_048_576 {
                format!("{:.1} MB", size as f64 / 1_048_576.0)
            } else if size > 1024 {
                format!("{:.1} KB", size as f64 / 1024.0)
            } else {
                format!("{} bytes", size)
            };

            // Build "Bind to Channel" UI for bindable asset types
            let is_bindable = matches!(
                ext.as_str(),
                "jpg"
                    | "jpeg"
                    | "png"
                    | "bmp"
                    | "gif"
                    | "webp"
                    | "mp4"
                    | "webm"
                    | "avi"
                    | "mkv"
                    | "glsl"
                    | "frag"
            );
            let bind_section: Element<'_, Message> = if is_bindable {
                // Find which channel this asset is already bound to (if any)
                let current_binding: Option<(String, u32)> = self
                    .shade_config
                    .textures
                    .iter()
                    .find(|(_, t)| t.source.as_deref() == Some(asset_name))
                    .map(|(name, t)| (name.clone(), t.binding.unwrap_or(0)));

                let mut bind_items: Vec<Element<'_, Message>> = Vec::new();
                bind_items.push(Space::new().height(12).into());

                if let Some((ref ch_name, _idx)) = current_binding {
                    bind_items.push(
                        row![
                            text(icons::CHECK)
                                .font(icons::ICON_FONT)
                                .size(13)
                                .color(self.theme_tokens.success),
                            text(format!("Currently bound to: {}", ch_name))
                                .size(13)
                                .color(self.theme_tokens.success),
                        ]
                        .spacing(4)
                        .align_y(iced::Alignment::Center)
                        .into(),
                    );
                    bind_items.push(Space::new().height(4).into());
                }

                bind_items.push(text("Bind to texture channel:").size(12).into());
                let asset_owned = asset_name.to_string();
                let mut channel_btns: Vec<Element<'_, Message>> = Vec::new();
                for ch in 0u32..4 {
                    let ch_name = format!("iChannel{}", ch);
                    let is_current = current_binding
                        .as_ref()
                        .map(|(_n, idx)| *idx == ch)
                        .unwrap_or(false);
                    let is_occupied =
                        self.shade_config.textures.contains_key(&ch_name) && !is_current;
                    let label = if is_current {
                        format!("[v] Ch{}", ch)
                    } else if is_occupied {
                        let occ_src = self
                            .shade_config
                            .textures
                            .get(&ch_name)
                            .and_then(|t| t.source.as_ref())
                            .map(|s| s.rsplit('/').next().unwrap_or(s).to_string())
                            .unwrap_or_else(|| "used".into());
                        format!("Ch{} ({})", ch, occ_src)
                    } else {
                        format!("Ch{}", ch)
                    };
                    let a_name = asset_owned.clone();
                    let msg = Message::ShadeBindAssetToTexture(a_name, ch);
                    // Build button inline to avoid lifetime issue with owned label
                    if is_current {
                        let bg = self.theme_tokens.button_primary;
                        let txt = self.theme_tokens.text_primary;
                        let hover = self.theme_tokens.button_hover;
                        channel_btns.push(
                            button(text(label).size(12))
                                .padding(Padding::from([6, 12]))
                                .on_press(msg)
                                .style(move |_theme: &Theme, status| match status {
                                    button::Status::Hovered => button::Style {
                                        background: Some(hover.into()),
                                        text_color: txt,
                                        border: Border::default().rounded(6),
                                        ..Default::default()
                                    },
                                    _ => button::Style {
                                        background: Some(bg.into()),
                                        text_color: txt,
                                        border: Border::default().rounded(6),
                                        ..Default::default()
                                    },
                                })
                                .into(),
                        );
                    } else {
                        let bg = self.theme_tokens.bg_tertiary;
                        let txt = self.theme_tokens.text_primary;
                        let bdr = self.theme_tokens.border_default;
                        let hover = self.theme_tokens.bg_secondary;
                        channel_btns.push(
                            button(text(label).size(12))
                                .padding(Padding::from([6, 12]))
                                .on_press(msg)
                                .style(move |_theme: &Theme, status| match status {
                                    button::Status::Hovered => button::Style {
                                        background: Some(hover.into()),
                                        text_color: txt,
                                        border: Border::default().rounded(6).width(1).color(bdr),
                                        ..Default::default()
                                    },
                                    _ => button::Style {
                                        background: Some(bg.into()),
                                        text_color: txt,
                                        border: Border::default().rounded(6).width(1).color(bdr),
                                        ..Default::default()
                                    },
                                })
                                .into(),
                        );
                    }
                }
                bind_items.push(
                    row(channel_btns)
                        .spacing(6)
                        .align_y(iced::Alignment::Center)
                        .into(),
                );

                column(bind_items).spacing(4).into()
            } else {
                Space::new().height(0).into()
            };

            let content: Element<'_, Message> = match ext.as_str() {
                "glsl" | "frag" | "vert" => {
                    let source = String::from_utf8_lossy(data);
                    column![
                        scrollable(text(source.to_string()).size(12))
                            .width(Fill)
                            .height(Fill),
                        bind_section,
                    ]
                    .spacing(4)
                    .into()
                }
                "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" => column![
                    row![
                        text(icons::IMAGE).font(icons::ICON_FONT).size(16),
                        text(format!("Image Asset: {}", short)).size(16),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                    text(format!("Size: {}", size_str)).size(13),
                    Space::new().height(8),
                    text("Image preview not available in editor").size(11),
                    bind_section,
                ]
                .spacing(6)
                .into(),
                "mp4" | "webm" | "avi" | "mkv" => column![
                    row![
                        text(icons::VIDEO).font(icons::ICON_FONT).size(16),
                        text(format!("Video Asset: {}", short)).size(16),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                    text(format!("Size: {}", size_str)).size(13),
                    Space::new().height(8),
                    text("Video preview not available in editor").size(11),
                    bind_section,
                ]
                .spacing(6)
                .into(),
                "ttf" | "otf" | "woff" | "woff2" => column![
                    row![
                        text(icons::FONT).font(icons::ICON_FONT).size(16),
                        text(format!("Font Asset: {}", short)).size(16),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                    text(format!("Size: {}", size_str)).size(13),
                    Space::new().height(8),
                    text("Font preview not available in editor").size(11),
                    text("Use this font in textures with type = \"font\"").size(11),
                ]
                .spacing(6)
                .into(),
                "mp3" | "wav" | "ogg" | "flac" => column![
                    row![
                        text(icons::AUDIO).font(icons::ICON_FONT).size(16),
                        text(format!("Audio Asset: {}", short)).size(16),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                    text(format!("Size: {}", size_str)).size(13),
                ]
                .spacing(6)
                .into(),
                _ => column![
                    row![
                        text(icons::FILE).font(icons::ICON_FONT).size(16),
                        text(format!("File: {}", short)).size(16),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                    text(format!("Size: {}", size_str)).size(13),
                ]
                .spacing(6)
                .into(),
            };
            return container(content)
                .width(Fill)
                .height(Fill)
                .padding(16)
                .into();
        }
        container(text(format!("Asset not found: {}", asset_name)).size(13))
            .width(Fill)
            .height(Fill)
            .padding(16)
            .into()
    }

    /// Settings form for config.toml (extracted from original shade editor).
    #[allow(dead_code)]
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
                    text_input(
                        "0 = monitor rate",
                        &self.shade_config.rendering.target_fps.to_string()
                    )
                    .on_input(Message::ShadeTargetFps)
                    .width(120),
                    text("(0 = match monitor)").size(11),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
                row![
                    checkbox(self.shade_config.rendering.pause_offscreen)
                        .label("Pause when offscreen")
                        .on_toggle(Message::ShadePauseOffscreen),
                    checkbox(self.shade_config.rendering.pause_fullscreen)
                        .label("Pause on fullscreen app")
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
                text("Audio is now configured as a texture type.").size(13),
                text("Add an 'audio_spectrum' texture in the Textures section below.").size(11),
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
        let uniforms = self.card("Uniforms", column(uniform_items).spacing(6));

        // Textures section
        let mut texture_items: Vec<Element<'_, Message>> = Vec::new();
        let mut texture_keys: Vec<String> = self.shade_config.textures.keys().cloned().collect();
        texture_keys.sort();
        for name in texture_keys {
            if let Some(t) = self.shade_config.textures.get(&name) {
                let src = t.source.clone().unwrap_or_else(|| "[!] no source".into());
                let ty = t.ty.as_str();
                let binding_str = t
                    .binding
                    .map(|b| format!(" [binding {}]", b))
                    .unwrap_or_default();
                let label = name.clone();
                let has_source = t.source.is_some();
                let source_color = if has_source {
                    self.theme_tokens.success
                } else {
                    self.theme_tokens.warning
                };
                texture_items.push(
                    row![
                        text(label).size(13).width(120),
                        text(ty).size(12).width(60),
                        text(format!("{}{}", src, binding_str))
                            .size(11)
                            .width(Fill)
                            .color(source_color),
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
                text_input(
                    "type (image/video/font/slideshow/audio_spectrum/noise)",
                    &self.shade_new_texture_type
                )
                .on_input(Message::ShadeNewTextureType)
                .width(160),
                self.btn_secondary("Add Texture", Message::ShadeAddTexture),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .into(),
        );
        texture_items.push(
            text("Types: image, video, font, slideshow, audio_spectrum, noise")
                .size(10)
                .into(),
        );
        if self
            .shade_config
            .textures
            .values()
            .any(|t| t.ty == kroma_shared::types::TextureType::Image)
        {
            texture_items.push(
                row![
                    text(icons::INFO).font(icons::ICON_FONT).size(10).color(self.theme_tokens.info),
                    text("GLSL textures generate data from a shader in assets/. Set source to the .glsl filename.").size(10).color(self.theme_tokens.info),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .into(),
            );
        }
        if self
            .shade_config
            .textures
            .values()
            .any(|t| t.ty == kroma_shared::types::TextureType::Font)
        {
            texture_items.push(
                row![
                    text(icons::INFO).font(icons::ICON_FONT).size(10).color(self.theme_tokens.text_accent),
                    text("Font textures render glyphs from .ttf/.otf files in assets/. Set source to the font file.").size(10).color(self.theme_tokens.text_accent),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .into(),
            );
        }
        texture_items.push(
            text("Tip: Click an image/video asset in the file tree, then use 'Bind to Channel' to assign it.")
                .size(10)
                .color(self.theme_tokens.text_secondary)
                .into(),
        );
        let textures = self.card("Textures", column(texture_items).spacing(6));

        // Render Buffers section
        let mut buffer_items: Vec<Element<'_, Message>> = Vec::new();
        let mut sorted_buffers: Vec<_> = self.shade_config.buffers.iter().collect();
        sorted_buffers.sort_by_key(|(k, _)| (*k).clone());
        for (name, buf_def) in &sorted_buffers {
            let buf_name = name.to_string();
            let buf_name2 = buf_name.clone();
            let buf_name3 = buf_name.clone();
            let shader_val = buf_def.shader.clone();
            buffer_items.push(
                row![
                    text(format!("Buffer {}", buf_name)).size(13).width(80),
                    text_input("shader file", &shader_val)
                        .size(12)
                        .on_input(move |v| Message::ShadeBufferShaderChanged(buf_name.clone(), v))
                        .width(Fill),
                    checkbox(buf_def.feedback)
                        .label("Feedback")
                        .on_toggle(move |_| Message::ShadeBufferFeedbackToggled(buf_name2.clone()))
                        .size(14),
                    button(text("Remove").size(11))
                        .on_press(Message::ShadeRemoveBuffer(buf_name3))
                        .padding(Padding::from([2, 8])),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center)
                .into(),
            );
        }
        if self.shade_config.buffers.len() < 4 {
            buffer_items.push(self.btn_secondary("+ Add Buffer", Message::ShadeAddBuffer));
        }
        buffer_items.push(
            text("Multi-pass buffers (A–D) render to offscreen textures for feedback effects.")
                .size(10)
                .color(self.theme_tokens.text_secondary)
                .into(),
        );
        let buffers = self.card("Render Buffers", column(buffer_items).spacing(6));

        // TOML preview
        let toml_preview = self.card(
            "config.toml Preview",
            container(
                scrollable(
                    iced::widget::text_editor(&self.shade_config_toml)
                        .on_action(Message::ShadeConfigToml),
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
                buffers,
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

// ---------------------------------------------------------------------------
// Standalone settings form for use in the context-aware previewer
// ---------------------------------------------------------------------------

/// Render the shade config settings form using shared context.
/// Used by the AssetPreview panel when config.toml is selected.
pub fn view_shade_settings_inline<'a>(ctx: crate::panels::AppContext<'a>) -> Element<'a, Message> {
    use crate::panels::dashboard::card;

    let t = ctx.tokens;
    let cfg = ctx.shade_config;
    let _available_sources = ctx.available_audio_sources;

    // Meta section
    let meta = card(
        "Metadata",
        column![
            text_input("Shader name", &cfg.meta.name).on_input(Message::ShadeMetaName),
            text_input("Author", &cfg.meta.author).on_input(Message::ShadeMetaAuthor),
            row![
                text_input("Version", &cfg.meta.version)
                    .on_input(Message::ShadeMetaVersion)
                    .width(100),
                text_input("Description", &cfg.meta.description)
                    .on_input(Message::ShadeMetaDescription),
            ]
            .spacing(8),
        ]
        .spacing(8),
        t,
    );

    // Rendering section
    let render = card(
        "Rendering",
        column![
            row![
                text("Target FPS:").size(13).width(120),
                text_input("0 = monitor rate", &cfg.rendering.target_fps.to_string())
                    .on_input(Message::ShadeTargetFps)
                    .width(80),
            ]
            .spacing(8),
            checkbox(cfg.rendering.pause_offscreen)
                .label("Pause offscreen")
                .on_toggle(Message::ShadePauseOffscreen),
            checkbox(cfg.rendering.pause_fullscreen)
                .label("Pause fullscreen")
                .on_toggle(Message::ShadePauseFullscreen),
        ]
        .spacing(8),
        t,
    );

    // Audio section
    let audio = card(
        "Audio",
        column![
            text("Audio is now configured as a texture type.").size(13),
            text("Add an 'audio_spectrum' texture in the Textures section.").size(11),
        ]
        .spacing(8),
        t,
    );

    scrollable(
        column![row![meta, render].spacing(16), audio,]
            .spacing(12)
            .padding(t.spacing_md)
            .width(Fill),
    )
    .width(Fill)
    .height(Fill)
    .into()
}
