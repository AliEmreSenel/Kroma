//! Asset Browser panel — file tree with drag-to-import.

use iced::widget::{button, column, container, row, scrollable, text, Space};
use iced::{Border, Element, Fill, Padding, Theme};

use crate::Message;
use crate::panels::dashboard::btn_secondary;
use crate::panels::{AppContext, Panel};

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
        "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" => "\u{1F5BC}",
        "mp4" | "webm" | "avi" | "mkv" => "\u{1F3AC}",
        "mp3" | "wav" | "ogg" | "flac" => "\u{1F3B5}",
        "ttf" | "otf" | "woff" | "woff2" => "\u{1F524}",
        "glsl" | "frag" | "vert" => "\u{1F4DD}",
        _ => "\u{1F4C4}",
    }
}

impl Panel for AssetBrowserPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let mut items: Vec<Element<'_, Message>> = Vec::new();

        items.push(
            text(format!("\u{1F4E6} {}", ctx.shade_config.meta.name))
                .size(14)
                .into(),
        );
        items.push(Space::with_height(4).into());

        // Core shade files
        items.push(tree_btn("config.toml", ctx.shade_selected_file));
        items.push(tree_btn("shader.frag", ctx.shade_selected_file));

        items.push(Space::with_height(6).into());
        items.push(text("\u{1F4C1} assets/").size(12).into());

        // Asset entries
        if let Some(pkg) = ctx.shade_package {
            let mut sorted: Vec<&(String, Vec<u8>)> = pkg.assets.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));

            for (name, data) in sorted {
                let short = name.rsplit('/').next().unwrap_or(name);
                let icon = shade_file_icon(short);
                let sz = if data.len() > 1_048_576 {
                    format!(" ({:.1}MB)", data.len() as f64 / 1_048_576.0)
                } else if data.len() > 1024 {
                    format!(" ({:.1}KB)", data.len() as f64 / 1024.0)
                } else {
                    format!(" ({}B)", data.len())
                };
                let is_sel = ctx.shade_selected_file == Some(name.as_str());
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
                btn_secondary("+ Asset", Message::ShadeAddAsset),
                btn_secondary("+ GLSL", Message::ShadeAddGlslFile),
            ]
            .spacing(4)
            .into(),
        );

        container(
            scrollable(column(items).spacing(2)).height(Fill),
        )
        .width(Fill)
        .height(Fill)
        .padding(8)
        .style(|theme: &Theme| {
            let p = theme.extended_palette();
            container::Style {
                background: Some(p.background.strong.color.into()),
                border: Border {
                    color: iced::Color {
                        a: 0.1,
                        ..p.background.base.text
                    },
                    width: 1.0,
                    radius: 0.into(),
                },
                ..Default::default()
            }
        })
        .into()
    }
}

/// Styled button for core shade files (config.toml, shader.frag).
fn tree_btn<'a>(file_id: &'a str, selected: Option<&str>) -> Element<'a, Message> {
    let is_sel = selected == Some(file_id);
    let label = format!("\u{1F4C4} {}", file_id);
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
