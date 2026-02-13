//! Asset Preview panel — image/video/font/shader preview.

use iced::widget::{column, container, scrollable, text};
use iced::{Element, Fill};

use crate::Message;
use crate::panels::{AppContext, Panel};

pub struct AssetPreviewPanel;

impl AssetPreviewPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for AssetPreviewPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let selected = ctx.shade_selected_file;

        // If no file selected or it's a core file, show placeholder
        let asset_name = match selected {
            Some(name) if name != "config.toml" && name != "shader.frag" => name,
            _ => {
                return container(
                    column![
                        text("Asset Preview").size(12),
                        text("Select an asset to preview").size(10),
                    ]
                    .spacing(4),
                )
                .width(Fill)
                .height(Fill)
                .padding(8)
                .into();
            }
        };

        let short = asset_name.rsplit('/').next().unwrap_or(asset_name);
        let ext = short.rsplit('.').next().unwrap_or("").to_lowercase();

        if let Some(pkg) = ctx.shade_package {
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
                        column![
                            text(format!("\u{1F4DD} {}", short)).size(12),
                            text(size_str.clone()).size(10),
                            scrollable(text(source.to_string()).size(10))
                                .width(Fill)
                                .height(Fill),
                        ]
                        .spacing(4)
                        .into()
                    }
                    "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" => {
                        column![
                            text(format!("\u{1F5BC} {}", short)).size(12),
                            text(size_str.clone()).size(10),
                            text("Image preview — available with FFmpeg pipeline").size(10),
                        ]
                        .spacing(4)
                        .into()
                    }
                    "mp4" | "webm" | "avi" | "mkv" => {
                        column![
                            text(format!("\u{1F3AC} {}", short)).size(12),
                            text(size_str.clone()).size(10),
                            text("Video preview — available with FFmpeg pipeline").size(10),
                        ]
                        .spacing(4)
                        .into()
                    }
                    "ttf" | "otf" | "woff" | "woff2" => {
                        column![
                            text(format!("\u{1F524} {}", short)).size(12),
                            text(size_str.clone()).size(10),
                            text("Font preview — glyph atlas renders here").size(10),
                        ]
                        .spacing(4)
                        .into()
                    }
                    "mp3" | "wav" | "ogg" | "flac" => {
                        column![
                            text(format!("\u{1F3B5} {}", short)).size(12),
                            text(size_str.clone()).size(10),
                            text("Audio waveform preview coming soon").size(10),
                        ]
                        .spacing(4)
                        .into()
                    }
                    _ => {
                        column![
                            text(format!("\u{1F4C4} {}", short)).size(12),
                            text(size_str.clone()).size(10),
                        ]
                        .spacing(4)
                        .into()
                    }
                };

                return container(content)
                    .width(Fill)
                    .height(Fill)
                    .padding(8)
                    .into();
            }
        }

        container(
            text(format!("Asset not found: {}", asset_name)).size(11),
        )
        .width(Fill)
        .height(Fill)
        .padding(8)
        .into()
    }
}