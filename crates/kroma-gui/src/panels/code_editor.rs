//! Code Editor panel — GLSL text editor with compile toolbar.
//!
//! This panel handles both the standalone editor's text mode and the
//! shade package editor's code view for shader.frag / config.toml.

use iced::widget::{column, container, row, scrollable, text, horizontal_space};
use iced::{Border, Element, Fill, Length, Padding, Theme};

use crate::Message;
use crate::panels::dashboard::{btn_primary, btn_secondary, btn_danger};
use crate::panels::{AppContext, Panel};

pub struct CodeEditorPanel;

impl CodeEditorPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for CodeEditorPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let mode_label = if ctx.editor_text_mode { "GLSL" } else { "Nodes" };
        let live_label = if ctx.editor_live_preview { "Live ON" } else { "Live OFF" };

        // ── Compact toolbar ────────────────────────────────────────────
        let toolbar = container(
            row![
                btn_secondary(
                    if ctx.editor_text_mode { "Nodes" } else { "GLSL" },
                    Message::EditorToggleTextMode,
                ),
                btn_secondary("Open", Message::EditorLoadGlsl),
                btn_primary("Compile", Message::EditorCompile),
                btn_primary("Send", Message::EditorLivePreview),
                btn_primary("Export", Message::EditorExport),
                if ctx.editor_live_preview {
                    btn_danger(live_label, Message::EditorToggleLive)
                } else {
                    btn_secondary(live_label, Message::EditorToggleLive)
                },
                horizontal_space(),
                text(mode_label).size(10),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([4, 8]))
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                ..Default::default()
            }
        });

        // ── GLSL preview side-panel ────────────────────────────────────
        let preview = if ctx.editor_glsl_preview.is_empty() {
            column![
                text("GLSL Output").size(11),
                text("Compile to see output").size(10),
            ]
            .spacing(2)
        } else {
            column![
                text("GLSL Output").size(11),
                scrollable(text(ctx.editor_glsl_preview).size(10))
                    .height(Length::Fill),
            ]
            .spacing(2)
        };
        let preview_panel = container(preview)
            .width(Length::FillPortion(1))
            .height(Fill)
            .padding(6)
            .style(|theme: &Theme| {
                let p = theme.extended_palette();
                container::Style {
                    background: Some(p.background.strong.color.into()),
                    ..Default::default()
                }
            });

        // ── Compile error/warning overlay ──────────────────────────────
        let error_panel = build_error_panel(ctx);

        // ── Text editor content ────────────────────────────────────────
        // Show shade_shader_content when a shade is loaded, otherwise editor_glsl_content
        let (content, action_msg): (&iced::widget::text_editor::Content, fn(iced::widget::text_editor::Action) -> Message) =
            if ctx.shade_loaded_path.is_some() {
                (ctx.shade_shader_content, Message::ShadeShaderChanged)
            } else {
                (ctx.editor_glsl_content, Message::EditorGlslSourceChanged)
            };

        let text_editor = container(
            scrollable(
                iced::widget::text_editor(content)
                    .on_action(action_msg),
            )
            .width(Fill)
            .height(Fill),
        )
        .width(Length::FillPortion(2))
        .height(Fill)
        .padding(4);

        let mut layout = column![
            row![text_editor, preview_panel].width(Fill).height(Fill),
        ];
        if let Some(ep) = error_panel {
            layout = layout.push(ep);
        }
        layout = layout.push(toolbar);
        layout.width(Fill).height(Fill).into()
    }
}

/// Build the compile error/warning bar (shared between panels).
pub fn build_error_panel<'a>(ctx: AppContext<'a>) -> Option<Element<'a, Message>> {
    if !ctx.compile_errors.is_empty() {
        let mut error_col = column![].spacing(2);
        for err in ctx.compile_errors {
            let loc = match (err.line, err.column) {
                (Some(l), Some(c)) => format!("line {}:{}: ", l, c),
                (Some(l), None) => format!("line {}: ", l),
                _ => String::new(),
            };
            error_col = error_col.push(
                text(format!("  \u{2716} {}{}", loc, err.message))
                    .size(11)
                    .color(iced::Color::from_rgb(1.0, 0.4, 0.4)),
            );
        }
        for w in ctx.compile_warnings {
            error_col = error_col.push(
                text(format!("  \u{26A0} {}", w))
                    .size(11)
                    .color(iced::Color::from_rgb(1.0, 0.8, 0.3)),
            );
        }
        Some(
            container(
                column![
                    text("Compilation Errors")
                        .size(12)
                        .color(iced::Color::from_rgb(1.0, 0.4, 0.4)),
                    scrollable(error_col).height(Length::Shrink),
                ]
                .spacing(4),
            )
            .padding(Padding::from([6, 12]))
            .max_height(120)
            .width(Fill)
            .style(|_theme: &Theme| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(
                    0.3, 0.05, 0.05, 0.8,
                ))),
                border: Border {
                    color: iced::Color::from_rgb(0.6, 0.15, 0.15),
                    width: 1.0,
                    radius: 0.into(),
                },
                ..Default::default()
            })
            .into(),
        )
    } else if ctx.compile_success == Some(true) && !ctx.compile_warnings.is_empty() {
        let mut warn_col = column![].spacing(2);
        for w in ctx.compile_warnings {
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
                    background: Some(iced::Background::Color(iced::Color::from_rgba(
                        0.3, 0.25, 0.0, 0.6,
                    ))),
                    ..Default::default()
                })
                .into(),
        )
    } else {
        None
    }
}