//! Library panel — grid view of all .shade packages in the output directory.

use iced::widget::{button, column, row, scrollable, text, Space};
use iced::{Border, Element, Fill, Padding, Theme};

use crate::icons;
use crate::panels::dashboard::{btn_primary, btn_secondary, card};
use crate::panels::{AppContext, Panel};
use crate::Message;

pub struct LibraryPanel {
    /// Discovered .shade files from the shader directory.
    pub shade_files: Vec<ShadeEntry>,
}

/// A discovered .shade package in the library.
#[derive(Debug, Clone)]
pub struct ShadeEntry {
    pub path: String,
    pub name: String,
    pub size_str: String,
}

impl LibraryPanel {
    pub fn new() -> Self {
        let shade_files = scan_shade_directory();
        Self { shade_files }
    }
}

impl Panel for LibraryPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let t = ctx.tokens;
        // ── Active wallpaper indicator ─────────────────────────────────
        let active = if let Some(loaded) = ctx.loaded_shade {
            card(
                "Active",
                column![
                    text(loaded).size(11),
                    btn_secondary("Unload", Message::UnloadShadeClicked, t),
                ]
                .spacing(4),
                t,
            )
        } else {
            card("Active", column![text("No shade loaded").size(11)], t)
        };

        // ── Action buttons (stacked for narrow sidebar) ────────────────
        let actions = column![
            btn_primary("Load .shade…", Message::LoadShadeClicked, t),
            btn_secondary("Open Package…", Message::ShadeOpen, t),
            btn_secondary("New Package", Message::ShadeNew, t),
        ]
        .spacing(t.spacing_sm)
        .width(Fill);

        // ── Package list ───────────────────────────────────────────────
        let mut entries: Vec<Element<'_, Message>> = Vec::new();
        if self.shade_files.is_empty() {
            entries.push(
                text("No .shade packages found.\nImport or create one first!")
                    .size(11)
                    .into(),
            );
        } else {
            let edit_hover_bg = t.tab_active;
            let edit_default_bg = t.bg_tertiary;
            let edit_text_col = t.text_accent;
            let load_hover_bg = iced::Color {
                a: 0.3,
                ..t.success
            };
            let load_default_bg = t.bg_tertiary;
            let load_text_col = t.success;
            for entry in &self.shade_files {
                let edit_path = std::path::PathBuf::from(entry.path.clone());
                let load_path = std::path::PathBuf::from(entry.path.clone());
                entries.push(
                    column![
                        row![
                            text(icons::PACKAGE).font(icons::ICON_FONT).size(12),
                            text(&entry.name).size(12),
                        ]
                        .spacing(4)
                        .align_y(iced::Alignment::Center),
                        row![
                            text(&entry.size_str).size(10),
                            Space::with_width(Fill),
                            button(text("Edit").size(10))
                                .on_press(Message::ShadeOpenResult(Some(edit_path)))
                                .padding(Padding::from([2, 8]))
                                .style(move |_theme: &Theme, status| {
                                    let bg = match status {
                                        button::Status::Hovered => edit_hover_bg,
                                        _ => edit_default_bg,
                                    };
                                    button::Style {
                                        background: Some(bg.into()),
                                        text_color: edit_text_col,
                                        border: Border::default().rounded(4),
                                        ..Default::default()
                                    }
                                }),
                            button(text("Load").size(10))
                                .on_press(Message::ShadeFileSelected(Some(load_path)))
                                .padding(Padding::from([2, 8]))
                                .style(move |_theme: &Theme, status| {
                                    let bg = match status {
                                        button::Status::Hovered => load_hover_bg,
                                        _ => load_default_bg,
                                    };
                                    button::Style {
                                        background: Some(bg.into()),
                                        text_color: load_text_col,
                                        border: Border::default().rounded(4),
                                        ..Default::default()
                                    }
                                }),
                        ]
                        .spacing(4)
                        .align_y(iced::Alignment::Center),
                    ]
                    .spacing(2)
                    .padding(Padding::from([4, 8]))
                    .into(),
                );
            }
        }

        let pkg_count_text = text(format!("{} packages", self.shade_files.len())).size(10);
        let packages_content = column![pkg_count_text]
            .push(column(entries).spacing(4))
            .spacing(4);
        let packages = card("Packages", packages_content, t);

        scrollable(
            column![active, actions, packages]
                .spacing(t.spacing_md)
                .padding(t.spacing_md)
                .width(Fill),
        )
        .width(Fill)
        .height(Fill)
        .into()
    }
}

/// Scan the default shade output directory for .shade files.
fn scan_shade_directory() -> Vec<ShadeEntry> {
    let dir = shade_output_dir();
    let mut entries = Vec::new();

    if let Ok(read_dir) = std::fs::read_dir(&dir) {
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("shade") {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let size_str = if size > 1_048_576 {
                    format!("{:.1} MB", size as f64 / 1_048_576.0)
                } else if size > 1024 {
                    format!("{:.1} KB", size as f64 / 1024.0)
                } else {
                    format!("{} B", size)
                };
                entries.push(ShadeEntry {
                    path: path.to_string_lossy().to_string(),
                    name,
                    size_str,
                });
            }
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Get the shade output directory path.
fn shade_output_dir() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        std::path::PathBuf::from(d).join("kroma/shaders")
    } else if let Some(h) = std::env::var_os("HOME") {
        std::path::PathBuf::from(h).join(".local/share/kroma/shaders")
    } else {
        std::path::PathBuf::from("./shaders")
    }
}
