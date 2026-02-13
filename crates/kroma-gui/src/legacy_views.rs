//! Legacy sidebar-based views (extracted from main.rs).
//! Used when `use_dock_layout == false`.

use iced::widget::{
    button, column, container, horizontal_rule, horizontal_space, progress_bar, row, scrollable,
    text, text_input, Space,
};
use iced::{color, Border, Element, Fill, Length, Padding, Theme};

use crate::editor;
use crate::utils;
use crate::Message;

use super::{KromaApp, Tab};

impl KromaApp {
    // -----------------------------------------------------------------------
    // Sidebar
    // -----------------------------------------------------------------------

    pub(crate) fn view_sidebar(&self) -> Element<'_, Message> {
        let brand = container(
            column![
                text("KROMA").size(22),
                text("Wallpaper Engine").size(11),
            ]
            .spacing(2),
        )
        .padding(Padding { top: 20.0, right: 20.0, bottom: 12.0, left: 20.0 });

        let nav = column![
            self.nav_btn("Dashboard", Tab::Dashboard),
            self.nav_btn("Import", Tab::Import),
            self.nav_btn("Editor", Tab::Editor),
            self.nav_btn("Shade", Tab::ShadeEdit),
            self.nav_btn("Settings", Tab::Settings),
        ]
        .spacing(2)
        .padding(Padding::from([4, 8]));

        let dot_color = if self.daemon_connected {
            color!(0x4ade80)
        } else {
            color!(0xf87171)
        };
        let label = if self.daemon_connected {
            "Daemon Online"
        } else {
            "Daemon Offline"
        };

        let status_badge = container(
            row![
                text("\u{25CF}").size(14).color(dot_color),
                text(label).size(12),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([12, 20]));

        container(
            column![
                brand,
                horizontal_rule(1),
                nav,
                Space::with_height(Fill),
                horizontal_rule(1),
                status_badge,
            ]
            .height(Fill),
        )
        .width(200)
        .height(Fill)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                ..Default::default()
            }
        })
        .into()
    }

    fn nav_btn<'a>(&self, label: &'a str, tab: Tab) -> Element<'a, Message> {
        let active = self.active_tab == tab;
        let btn = button(text(label).size(14))
            .width(Fill)
            .padding(Padding::from([10, 16]))
            .on_press(Message::TabSelected(tab));

        if active {
            btn.style(|theme: &Theme, status| {
                let p = theme.extended_palette();
                match status {
                    button::Status::Active => button::Style {
                        background: Some(p.primary.base.color.into()),
                        text_color: p.primary.base.text,
                        border: Border::default().rounded(6),
                        ..Default::default()
                    },
                    _ => button::primary(theme, status),
                }
            })
            .into()
        } else {
            btn.style(|theme: &Theme, status| {
                let p = theme.extended_palette();
                match status {
                    button::Status::Active => button::Style {
                        background: None,
                        text_color: p.background.base.text,
                        border: Border::default().rounded(6),
                        ..Default::default()
                    },
                    button::Status::Hovered => button::Style {
                        background: Some(p.background.weak.color.into()),
                        text_color: p.background.base.text,
                        border: Border::default().rounded(6),
                        ..Default::default()
                    },
                    _ => button::secondary(theme, status),
                }
            })
            .into()
        }
    }

    // -----------------------------------------------------------------------
    // Dashboard tab
    // -----------------------------------------------------------------------

    pub(crate) fn view_dashboard(&self) -> Element<'_, Message> {
        let header = row![
            column![
                text("Dashboard").size(24),
                text("Monitor and control your wallpaper").size(13),
            ]
            .spacing(4),
            horizontal_space(),
            self.btn_secondary("Refresh", Message::RefreshStatus),
        ]
        .align_y(iced::Alignment::Center);

        // Performance card
        let target = if self.target_fps > 0 { self.target_fps as f32 } else { 60.0 };
        let fps_pct = (self.fps / target * 100.0).min(100.0);
        let fps_card = self.card(
            "Performance",
            column![
                row![
                    text(format!("{:.1}", self.fps)).size(36),
                    column![
                        text("FPS").size(14),
                        text(format!("/ {:.0} target", target)).size(10),
                    ]
                    .spacing(2),
                ]
                .spacing(6)
                .align_y(iced::Alignment::End),
                progress_bar(0.0..=target, self.fps).height(6),
                text(format!("{:.0}% of target", fps_pct)).size(10),
            ]
            .spacing(8),
        );

        // Status card
        let status_card = self.card(
            "Status",
            column![
                self.info_row(
                    "Daemon",
                    if self.daemon_connected {
                        "Connected"
                    } else {
                        "Disconnected"
                    },
                ),
                self.info_row(
                    "State",
                    if self.paused { "Paused" } else { "Running" },
                ),
                self.info_row("Info", &self.status_text),
            ]
            .spacing(6),
        );

        let stats = row![fps_card, status_card].spacing(16);

        // Active shade card
        let shade_card = if let Some(ref shade) = self.loaded_shade {
            let short = shade.rsplit('/').next().unwrap_or(shade);
            self.card(
                "Active Wallpaper",
                row![
                    column![text(short).size(16), text(shade).size(11)]
                        .spacing(2)
                        .width(Fill),
                    self.btn_danger("Unload", Message::UnloadShadeClicked),
                ]
                .align_y(iced::Alignment::Center),
            )
        } else {
            self.card(
                "Active Wallpaper",
                column![
                    text("No wallpaper loaded").size(14),
                    text("Load a .shade package to get started").size(12),
                ]
                .spacing(4),
            )
        };

        // Controls card
        let pr_btn = if self.paused {
            self.btn_primary("Resume", Message::ResumeClicked)
        } else {
            self.btn_secondary("Pause", Message::PauseClicked)
        };
        let controls = self.card(
            "Controls",
            row![
                self.btn_primary("Load Shade", Message::LoadShadeClicked),
                pr_btn,
                self.btn_danger("Shutdown Daemon", Message::ShutdownClicked),
            ]
            .spacing(8),
        );

        // Event log card
        let log_content: Element<Message> = if self.log_messages.is_empty() {
            text("No events yet.").size(12).into()
        } else {
            let items: Vec<Element<Message>> = self
                .log_messages
                .iter()
                .rev()
                .take(15)
                .map(|m| text(m).size(11).into())
                .collect();
            column(items).spacing(3).into()
        };
        let log = self.card(
            "Event Log",
            scrollable(log_content).height(Length::Fixed(120.0)),
        );

        scrollable(
            column![header, stats, shade_card, controls, log].spacing(16),
        )
        .into()
    }

    // -----------------------------------------------------------------------
    // Import tab
    // -----------------------------------------------------------------------

    pub(crate) fn view_import(&self) -> Element<'_, Message> {
        let header = column![
            text("Import Shaders").size(24),
            text("Import from local files or download from Shadertoy").size(13),
        ]
        .spacing(4);

        let local_card = self.card(
            "Import Local File",
            column![
                text("Select a Shadertoy-compatible GLSL file to convert into a .shade package.")
                    .size(12),
                Space::with_height(4),
                row![
                    text_input("Path to .glsl file...", &self.import_path)
                        .on_input(Message::ImportPathChanged)
                        .width(Fill)
                        .padding(10),
                    self.btn_secondary("Browse", Message::BrowseImportClicked),
                ]
                .spacing(8),
                row![
                    text_input("Shader name", &self.import_name)
                        .on_input(Message::ImportNameChanged)
                        .padding(10),
                    text_input("Author", &self.import_author)
                        .on_input(Message::ImportAuthorChanged)
                        .padding(10),
                    self.btn_primary("Import", Message::ImportClicked),
                ]
                .spacing(8),
                text(&self.import_result).size(12),
            ]
            .spacing(10),
        );

        let dl_btn = if self.downloading {
            self.btn_secondary("Downloading...", Message::DownloadShadertoyClicked)
        } else {
            self.btn_primary("Download", Message::DownloadShadertoyClicked)
        };

        let download_card = self.card(
            "Download from Shadertoy",
            column![
                text(
                    "Paste a Shadertoy URL or shader ID to download and convert automatically."
                )
                .size(12),
                Space::with_height(4),
                row![
                    text_input(
                        "https://www.shadertoy.com/view/XsXXDn  or  XsXXDn",
                        &self.shadertoy_url,
                    )
                    .on_input(Message::ShadertoyUrlChanged)
                    .width(Fill)
                    .padding(10),
                    dl_btn,
                ]
                .spacing(8),
                text(&self.download_result).size(12),
            ]
            .spacing(10),
        );

        scrollable(
            column![header, local_card, download_card].spacing(16),
        )
        .into()
    }

    // -----------------------------------------------------------------------
    // Editor tab
    // -----------------------------------------------------------------------

    pub(crate) fn view_editor(&self) -> Element<'_, Message> {
        // Mode toggle labels
        let mode_label = if self.editor_text_mode { "GLSL Text" } else { "Node Graph" };
        let live_label = if self.editor_live_preview { "Live: ON" } else { "Live: OFF" };

        // Bottom toolbar
        let toolbar = container(
            row![
                self.btn_secondary(
                    if self.editor_text_mode { "Switch to Nodes" } else { "Switch to GLSL" },
                    Message::EditorToggleTextMode,
                ),
                self.btn_secondary("Open GLSL File", Message::EditorLoadGlsl),
                self.btn_primary("Compile", Message::EditorCompile),
                self.btn_primary("Send to Daemon", Message::EditorLivePreview),
                self.btn_primary("Export .shade", Message::EditorExport),
                if self.editor_live_preview {
                    self.btn_danger(live_label, Message::EditorToggleLive)
                } else {
                    self.btn_secondary(live_label, Message::EditorToggleLive)
                },
                horizontal_space(),
                text(format!("Mode: {}", mode_label)).size(12),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([8, 12]))
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                ..Default::default()
            }
        });

        // Right panel: GLSL preview
        let preview = if self.editor_glsl_preview.is_empty() {
            column![
                text("GLSL Output").size(13),
                text("Click 'Compile' to see output").size(11),
            ]
            .spacing(4)
        } else {
            column![
                text("GLSL Output").size(13),
                scrollable(text(&self.editor_glsl_preview).size(10))
                    .height(Length::Fill),
            ]
            .spacing(4)
        };
        let preview_panel = container(preview)
            .width(220)
            .height(Fill)
            .padding(8)
            .style(|theme: &Theme| {
                let p = theme.extended_palette();
                container::Style {
                    background: Some(p.background.strong.color.into()),
                    ..Default::default()
                }
            });

        // Compile error/warning panel (shown between editor and toolbar)
        let error_panel: Option<Element<'_, Message>> = if !self.compile_errors.is_empty() {
            let mut error_col = column![].spacing(2);
            for err in &self.compile_errors {
                let loc = match (err.line, err.column) {
                    (Some(l), Some(c)) => format!("line {}:{}: ", l, c),
                    (Some(l), None) => format!("line {}: ", l),
                    _ => String::new(),
                };
                error_col = error_col.push(
                    text(format!("  {} {}{}", "\u{2716}", loc, err.message))
                        .size(11)
                        .color(iced::Color::from_rgb(1.0, 0.4, 0.4)),
                );
            }
            for w in &self.compile_warnings {
                error_col = error_col.push(
                    text(format!("  \u{26A0} {}", w))
                        .size(11)
                        .color(iced::Color::from_rgb(1.0, 0.8, 0.3)),
                );
            }
            Some(
                container(
                    column![
                        text("Compilation Errors").size(12).color(
                            iced::Color::from_rgb(1.0, 0.4, 0.4)
                        ),
                        scrollable(error_col).height(Length::Shrink),
                    ]
                    .spacing(4),
                )
                .padding(Padding::from([6, 12]))
                .max_height(120)
                .width(Fill)
                .style(|theme: &Theme| {
                    let _p = theme.extended_palette();
                    container::Style {
                        background: Some(iced::Background::Color(
                            iced::Color::from_rgba(0.3, 0.05, 0.05, 0.8),
                        )),
                        border: iced::Border {
                            color: iced::Color::from_rgb(0.6, 0.15, 0.15),
                            width: 1.0,
                            radius: 0.into(),
                        },
                        ..Default::default()
                    }
                })
                .into(),
            )
        } else if self.compile_success == Some(true) && !self.compile_warnings.is_empty() {
            let mut warn_col = column![].spacing(2);
            for w in &self.compile_warnings {
                warn_col = warn_col.push(
                    text(format!("  \u{26A0} {}", w))
                        .size(11)
                        .color(iced::Color::from_rgb(1.0, 0.8, 0.3)),
                );
            }
            Some(
                container(warn_col)
                    .padding(Padding::from([4, 12]))
                    .max_height(80)
                    .width(Fill)
                    .style(|_theme: &Theme| container::Style {
                        background: Some(iced::Background::Color(
                            iced::Color::from_rgba(0.3, 0.25, 0.0, 0.6),
                        )),
                        ..Default::default()
                    })
                    .into(),
            )
        } else {
            None
        };

        if self.editor_text_mode {
            // ---- TEXT EDITOR MODE ----
            let text_editor_widget = container(
                scrollable(
                    iced::widget::text_editor(&self.editor_glsl_content)
                        .on_action(Message::EditorGlslSourceChanged)
                )
                .width(Fill)
                .height(Fill),
            )
            .width(Fill)
            .height(Fill)
            .padding(4);

            let mut layout = column![
                row![text_editor_widget, preview_panel].width(Fill).height(Fill),
            ];
            if let Some(ep) = error_panel {
                layout = layout.push(ep);
            }
            layout = layout.push(toolbar);
            layout.width(Fill).height(Fill).into()
        } else {
            // ---- NODE GRAPH MODE ----
            let palette = self.view_node_palette();
            let canvas = editor::canvas::graph_canvas(&self.shader_graph, &self.graph_canvas)
                .map(Message::EditorGraph);

            let mut layout = column![
                row![palette, canvas, preview_panel].width(Fill).height(Fill),
            ];
            if let Some(ep) = error_panel {
                layout = layout.push(ep);
            }
            layout = layout.push(toolbar);
            layout.width(Fill).height(Fill).into()
        }
    }

    /// Node palette — categories of nodes user can add to the graph.
    pub(crate) fn view_node_palette_for(
        &self,
        msg_wrap: fn(editor::NodeKind) -> Message,
    ) -> Element<'_, Message> {
        let mut palette_col = column![text("Node Palette").size(14)].spacing(6);

        for (category, kinds) in editor::palette() {
            let mut cat_col = column![
                text(category).size(12),
            ]
            .spacing(2);

            for kind in kinds {
                let label = kind.label();
                let btn = button(text(label).size(11))
                    .width(Fill)
                    .padding(Padding::from([4, 8]))
                    .on_press(msg_wrap(kind))
                    .style(|theme: &Theme, status| {
                        let p = theme.extended_palette();
                        match status {
                            button::Status::Hovered => button::Style {
                                background: Some(p.primary.weak.color.into()),
                                text_color: p.primary.weak.text,
                                border: iced::Border::default().rounded(4),
                                ..Default::default()
                            },
                            _ => button::Style {
                                background: None,
                                text_color: p.background.base.text,
                                border: iced::Border::default().rounded(4),
                                ..Default::default()
                            },
                        }
                    });
                cat_col = cat_col.push(btn);
            }
            palette_col = palette_col.push(cat_col);
            palette_col = palette_col.push(Space::with_height(4));
        }

        container(
            scrollable(palette_col).height(Fill),
        )
        .width(170)
        .height(Fill)
        .padding(8)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                ..Default::default()
            }
        })
        .into()
    }

    pub(crate) fn view_node_palette(&self) -> Element<'_, Message> {
        self.view_node_palette_for(Message::EditorAddNode)
    }

    pub(crate) fn view_shade_node_palette(&self) -> Element<'_, Message> {
        self.view_node_palette_for(Message::ShadeAddNode)
    }

    // -----------------------------------------------------------------------
    // Settings tab
    // -----------------------------------------------------------------------

    pub(crate) fn view_settings(&self) -> Element<'_, Message> {
        let header = column![
            text("Settings").size(24),
            text("Configure Kroma preferences").size(13),
        ]
        .spacing(4);

        let api_card = self.card(
            "Shadertoy API Key",
            column![
                text("Optional: provide your Shadertoy API key for reliable downloads.").size(12),
                text("Get one at: https://www.shadertoy.com/profile").size(11),
                Space::with_height(4),
                text_input("Paste Shadertoy API key...", &self.api_key)
                    .on_input(Message::ApiKeyChanged)
                    .padding(10),
                text("Key is stored for this session only. Set SHADERTOY_API_KEY for persistence.")
                    .size(11),
            ]
            .spacing(6),
        );

        let dirs_card = self.card(
            "Directories",
            column![
                self.info_row("Shader output", &format!("{}", utils::dirs_output_dir().display())),
                self.info_row("IPC socket", "/tmp/kroma.sock"),
            ]
            .spacing(6),
        );

        let about_card = self.card(
            "About",
            column![
                text(format!(
                    "Kroma Wallpaper Engine v{}",
                    env!("CARGO_PKG_VERSION")
                ))
                .size(14),
                text("High-performance modular wallpaper engine for Linux (Wayland / Hyprland)")
                    .size(12),
                Space::with_height(4),
                self.info_row("Renderer", "wgpu (Vulkan)"),
                self.info_row("Shaders", "GLSL 450 \u{2192} WGSL (naga)"),
                self.info_row("Audio", "cpal + rustfft"),
                self.info_row("IPC", "Unix domain sockets"),
            ]
            .spacing(4),
        );

        scrollable(
            column![header, api_card, dirs_card, about_card].spacing(16),
        )
        .into()
    }
}
