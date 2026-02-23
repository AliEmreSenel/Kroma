//! Asset Browser panel — file tree with drag-to-import.

use iced::widget::{Space, button, column, container, row, scrollable, text};
use iced::{Border, Element, Fill, Padding, Theme};

use crate::Message;
use crate::icons;
use crate::panels::dashboard::btn_secondary;
use crate::panels::{AppContext, Panel};
use crate::theme::ThemeTokens;

pub struct AssetBrowserPanel;

impl AssetBrowserPanel {
    pub fn new() -> Self {
        Self
    }
}

/// Returns an icon character for a file based on its extension.
fn shade_file_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" => icons::IMAGE,
        "mp4" | "webm" | "avi" | "mkv" => icons::VIDEO,
        "mp3" | "wav" | "ogg" | "flac" => icons::AUDIO,
        "ttf" | "otf" | "woff" | "woff2" => icons::FONT,
        "glsl" | "frag" | "vert" => icons::CODE,
        _ => icons::FILE,
    }
}

impl Panel for AssetBrowserPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let t = ctx.tokens;
        let mut items: Vec<Element<'_, Message>> = Vec::new();

        items.push(
            row![
                text(icons::PACKAGE).font(icons::ICON_FONT).size(14),
                text(format!(" {}", ctx.shade_config.meta.name)).size(14),
            ]
            .spacing(2)
            .align_y(iced::Alignment::Center)
            .into(),
        );
        items.push(Space::new().height(4).into());

        // Core shade files
        items.push(tree_btn("config.toml", ctx.shade_selected_file, t));
        items.push(tree_btn("shader.frag", ctx.shade_selected_file, t));

        items.push(Space::new().height(6).into());
        items.push(
            row![
                text(icons::FOLDER).font(icons::ICON_FONT).size(12),
                text(" assets/").size(12),
            ]
            .spacing(2)
            .align_y(iced::Alignment::Center)
            .into(),
        );

        // Asset entries
        if let Some(pkg) = ctx.shade_package {
            let mut sorted = pkg.asset_entries();
            sorted.sort_by(|a, b| a.name.cmp(&b.name));

            let sel_bg = t.tab_active;
            let sel_text = t.text_primary;
            let hover_bg = t.bg_secondary;
            let normal_text = t.text_primary;
            let remove_color = t.error;

            for entry in &sorted {
                let name = &entry.name;
                let short = name.rsplit('/').next().unwrap_or(name);
                let icon = shade_file_icon(short);
                let sz = if entry.size > 1_048_576 {
                    format!(" ({:.1}MB)", entry.size as f64 / 1_048_576.0)
                } else if entry.size > 1024 {
                    format!(" ({:.1}KB)", entry.size as f64 / 1024.0)
                } else {
                    format!(" ({}B)", entry.size)
                };
                let is_sel = ctx.shade_selected_file == Some(name.as_str());
                let name_c = name.clone();
                let remove_c = name.clone();

                items.push(
                    row![
                        button(
                            row![
                                text(icon).font(icons::ICON_FONT).size(11),
                                text(format!(" {}{}", short, sz)).size(11),
                            ]
                            .spacing(2)
                            .align_y(iced::Alignment::Center)
                        )
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
                                        text_color: normal_text,
                                        border: Border::default().rounded(4),
                                        ..Default::default()
                                    },
                                    _ => button::Style {
                                        background: None,
                                        text_color: normal_text,
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
                                text_color: remove_color,
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

        // Drop zone indicator when a file is being dragged over the window
        if ctx.drop_hover_active {
            let dz_color = t.drop_zone;
            let dz_border = t.border_focused;
            items.push(
                container(
                    text("Drop file here to import")
                        .size(11)
                        .color(t.text_accent),
                )
                .width(Fill)
                .padding(Padding::from([12, 8]))
                .style(move |_theme: &Theme| container::Style {
                    background: Some(dz_color.into()),
                    border: Border::default().rounded(6).width(2).color(dz_border),
                    ..Default::default()
                })
                .into(),
            );
            items.push(Space::new().height(4).into());
        }

        items.push(
            row![
                btn_secondary("+ Asset", Message::ShadeAddAsset, t),
                btn_secondary("+ GLSL", Message::ShadeAddGlslFile, t),
            ]
            .spacing(4)
            .into(),
        );

        let outer_bg = t.bg_tertiary;
        let outer_border = t.border_default;
        container(scrollable(column(items).spacing(2)).height(Fill))
            .width(Fill)
            .height(Fill)
            .padding(8)
            .style(move |_theme: &Theme| container::Style {
                background: Some(outer_bg.into()),
                border: Border {
                    color: outer_border,
                    width: 1.0,
                    radius: 0.into(),
                },
                ..Default::default()
            })
            .into()
    }
}

/// Styled button for core shade files (config.toml, shader.frag).
fn tree_btn<'a>(
    file_id: &'a str,
    selected: Option<&str>,
    tokens: &ThemeTokens,
) -> Element<'a, Message> {
    let is_sel = selected == Some(file_id);
    let id_owned = file_id.to_string();

    let sel_bg = tokens.tab_active;
    let sel_text = tokens.text_primary;
    let hover_bg = tokens.bg_secondary;
    let normal_text = tokens.text_primary;

    button(
        row![
            text(icons::FILE).font(icons::ICON_FONT).size(12),
            text(format!(" {}", file_id)).size(12),
        ]
        .spacing(2)
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
                    text_color: normal_text,
                    border: Border::default().rounded(4),
                    ..Default::default()
                },
                _ => button::Style {
                    background: None,
                    text_color: normal_text,
                    border: Border::default().rounded(4),
                    ..Default::default()
                },
            }
        }
    })
    .into()
}
