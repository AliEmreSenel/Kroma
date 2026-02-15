//! Code Editor panel — GLSL text editor with compile toolbar.
//!
//! This panel handles both the standalone editor's text mode and the
//! shade package editor's code view for shader.frag / config.toml.

use iced::widget::{column, container, horizontal_space, row, scrollable, text};
use iced::{Border, Element, Fill, Length, Padding, Theme};

use crate::icons;
use crate::panels::dashboard::{btn_danger, btn_primary, btn_secondary};
use crate::panels::{AppContext, Panel};
use crate::Message;

pub struct CodeEditorPanel;

impl CodeEditorPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for CodeEditorPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let t = ctx.tokens;
        let mode_label = if ctx.editor_text_mode {
            "GLSL"
        } else {
            "Nodes"
        };
        let live_label = if ctx.editor_live_preview {
            "Live ON"
        } else {
            "Live OFF"
        };

        let toolbar_bg = t.bg_tertiary;
        let toolbar = container(
            row![
                btn_secondary(
                    if ctx.editor_text_mode {
                        "Nodes"
                    } else {
                        "GLSL"
                    },
                    Message::EditorToggleTextMode,
                    t,
                ),
                btn_secondary("Open", Message::EditorLoadGlsl, t),
                btn_primary("Compile", Message::EditorCompile, t),
                btn_primary("Send", Message::EditorLivePreview, t),
                btn_primary("Export", Message::EditorExport, t),
                if ctx.editor_live_preview {
                    btn_danger(live_label, Message::EditorToggleLive, t)
                } else {
                    btn_secondary(live_label, Message::EditorToggleLive, t)
                },
                horizontal_space(),
                text(mode_label).size(10),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([4, 8]))
        .style(move |_theme: &Theme| container::Style {
            background: Some(toolbar_bg.into()),
            ..Default::default()
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
                scrollable(text(ctx.editor_glsl_preview).size(10)).height(Length::Fill),
            ]
            .spacing(2)
        };
        let preview_bg = t.bg_tertiary;
        let preview_panel = container(preview)
            .width(Length::FillPortion(1))
            .height(Fill)
            .padding(6)
            .style(move |_theme: &Theme| container::Style {
                background: Some(preview_bg.into()),
                ..Default::default()
            });

        // ── Compile error/warning overlay ──────────────────────────────
        let error_panel = build_error_panel(ctx.clone());

        // ── Text editor content ────────────────────────────────────────
        // Show shade_shader_content when a shade is loaded, otherwise editor_glsl_content
        let (content, action_msg): (
            &iced::widget::text_editor::Content,
            fn(iced::widget::text_editor::Action) -> Message,
        ) = if ctx.shade_loaded_path.is_some() {
            (ctx.shade_shader_content, Message::ShadeShaderChanged)
        } else {
            (ctx.editor_glsl_content, Message::EditorGlslSourceChanged)
        };

        let text_editor = container(
            scrollable(iced::widget::text_editor(content).on_action(action_msg))
                .width(Fill)
                .height(Fill),
        )
        .width(Length::FillPortion(2))
        .height(Fill)
        .padding(4);

        let mut layout = column![row![text_editor, preview_panel].width(Fill).height(Fill),];
        if let Some(ep) = error_panel {
            layout = layout.push(ep);
        }
        layout = layout.push(toolbar);
        layout.width(Fill).height(Fill).into()
    }
}

/// Build the compile error/warning bar (shared between panels).
pub fn build_error_panel<'a>(ctx: AppContext<'a>) -> Option<Element<'a, Message>> {
    let t = ctx.tokens;
    if !ctx.compile_errors.is_empty() {
        let mut error_col = column![].spacing(2);
        for err in ctx.compile_errors {
            let loc = match (err.line, err.column) {
                (Some(l), Some(c)) => format!("line {}:{}: ", l, c),
                (Some(l), None) => format!("line {}: ", l),
                _ => String::new(),
            };
            error_col = error_col.push(
                row![
                    text(icons::ERROR).font(icons::ICON_FONT).size(11).color(t.error),
                    text(format!(" {}{}", loc, err.message)).size(11).color(t.error),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
            );
        }
        for w in ctx.compile_warnings {
            error_col = error_col.push(
                row![
                    text(icons::WARNING).font(icons::ICON_FONT).size(11).color(t.warning),
                    text(format!(" {}", w)).size(11).color(t.warning),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
            );
        }
        Some(
            container(
                column![
                    text("Compilation Errors").size(12).color(t.error),
                    scrollable(error_col).height(Length::Shrink),
                ]
                .spacing(4),
            )
            .padding(Padding::from([6, 12]))
            .max_height(120)
            .width(Fill)
            .style({
                let err_bg = iced::Color { a: 0.25, ..t.error };
                let err_border = iced::Color { a: 0.5, ..t.error };
                move |_theme: &Theme| container::Style {
                    background: Some(iced::Background::Color(err_bg)),
                    border: Border {
                        color: err_border,
                        width: 1.0,
                        radius: 0.into(),
                    },
                    ..Default::default()
                }
            })
            .into(),
        )
    } else if ctx.compile_success == Some(true) && !ctx.compile_warnings.is_empty() {
        let mut warn_col = column![].spacing(2);
        for w in ctx.compile_warnings {
            warn_col = warn_col.push(
                row![
                    text(icons::WARNING).font(icons::ICON_FONT).size(11).color(t.warning),
                    text(format!(" {}", w)).size(11).color(t.warning),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
            );
        }
        Some(
            container(warn_col)
                .padding(Padding::from([4, 12]))
                .max_height(80)
                .width(Fill)
                .style({
                    let warn_bg = iced::Color {
                        a: 0.2,
                        ..t.warning
                    };
                    move |_theme: &Theme| container::Style {
                        background: Some(iced::Background::Color(warn_bg)),
                        ..Default::default()
                    }
                })
                .into(),
        )
    } else {
        None
    }
}
