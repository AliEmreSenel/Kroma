//! Asset Preview panel — image/video/font/shader preview.

use iced::widget::{column, container, scrollable, slider, text};
use iced::{Element, Fill};

use crate::icons;
use crate::panels::{AppContext, Panel};
use crate::Message;

pub struct AssetPreviewPanel;

impl AssetPreviewPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Panel for AssetPreviewPanel {
    fn view<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let t = ctx.tokens;
        let selected = ctx.shade_selected_file;

        match selected {
            None => {
                return container(
                    column![
                        text("Kroma Workspace").size(14).color(t.text_primary),
                        text("Select a file to preview")
                            .size(11)
                            .color(t.text_secondary),
                        text("Or use New/Open from the menu bar")
                            .size(10)
                            .color(t.text_secondary),
                    ]
                    .spacing(6),
                )
                .center(Fill)
                .width(Fill)
                .height(Fill)
                .padding(8)
                .into();
            }
            Some("config.toml") => {
                return self.view_config_editor(ctx);
            }
            Some(name)
                if name == "shader.frag"
                    || name.ends_with(".glsl")
                    || name.ends_with(".frag")
                    || name.ends_with(".vert") =>
            {
                return self.view_shader_editor(ctx, name);
            }
            Some(name) => {
                return self.view_asset(ctx, name);
            }
        }
    }
}

impl AssetPreviewPanel {
    /// View for config.toml — project settings form with mode toggle.
    fn view_config_editor<'a>(&'a self, ctx: AppContext<'a>) -> Element<'a, Message> {
        let t = ctx.tokens;
        let edit_mode = ctx.shade_edit_mode;

        // Mode toggle bar
        let mode_bar =
            self.build_mode_bar(&["settings", "toml"], &["Settings", "TOML"], edit_mode, t);

        let content: Element<'_, Message> = if edit_mode == "toml" {
            // Raw TOML editor
            use iced::widget::text_editor;
            column![text_editor(ctx.shade_config_toml)
                .on_action(Message::ShadeConfigToml)
                .height(Fill),]
            .width(Fill)
            .height(Fill)
            .into()
        } else {
            // Settings form (reuse shade settings form view)
            crate::shade_views::view_shade_settings_inline(ctx)
        };

        column![mode_bar, content]
            .spacing(2)
            .width(Fill)
            .height(Fill)
            .into()
    }

    /// View for shader files — node editor / code editor with mode toggle.
    fn view_shader_editor<'a>(&'a self, ctx: AppContext<'a>, name: &str) -> Element<'a, Message> {
        let t = ctx.tokens;
        let edit_mode = ctx.shade_edit_mode;
        let short = name.rsplit('/').next().unwrap_or(name);

        // Mode toggle bar
        let mode_bar = self.build_mode_bar(
            &["nodes", "code", "preview"],
            &["Nodes", "Code", "Preview"],
            edit_mode,
            t,
        );

        let content: Element<'_, Message> = match edit_mode {
            "nodes" => {
                // Resolve active shade graph + canvas based on nav_path
                use iced::widget::{button, container, row};

                let (active_graph, active_canvas) =
                    if let Some(&last_id) = ctx.shade_nav_path.last() {
                        if let Some((sg, sc)) = ctx.shade_subgraphs.get(&last_id) {
                            (sg, sc)
                        } else {
                            (ctx.shade_graph, ctx.shade_graph_canvas)
                        }
                    } else {
                        (ctx.shade_graph, ctx.shade_graph_canvas)
                    };

                let canvas = crate::editor::canvas::graph_canvas(active_graph, active_canvas, t)
                    .map(Message::ShadeGraphMsg);

                let palette = crate::panels::node_editor::build_node_palette(
                    ctx.editor_palette_filter,
                    t,
                    |kind| {
                        Message::ShadeGraphMsg(crate::editor::canvas::GraphMessage::SetPendingNode(
                            kind,
                        ))
                    },
                );

                let toolbar = iced::widget::row![
                    iced::widget::button(text("Compile").size(11).color(t.text_primary))
                        .on_press(Message::ShadeCompileGraph)
                        .padding(iced::Padding::from([3, 8]))
                        .style({
                            let bg = t.bg_tertiary;
                            let txt = t.text_primary;
                            move |_: &iced::Theme, _| iced::widget::button::Style {
                                background: Some(iced::Background::Color(bg)),
                                text_color: txt,
                                ..Default::default()
                            }
                        }),
                    iced::widget::button(text("Parse GLSL").size(11).color(t.text_primary))
                        .on_press(Message::ShadeParseToNodes)
                        .padding(iced::Padding::from([3, 8]))
                        .style({
                            let bg = t.bg_tertiary;
                            let txt = t.text_primary;
                            move |_: &iced::Theme, _| iced::widget::button::Style {
                                background: Some(iced::Background::Color(bg)),
                                text_color: txt,
                                ..Default::default()
                            }
                        }),
                ]
                .spacing(4)
                .padding(iced::Padding::from([2, 4]));

                // Build breadcrumb bar when navigated into sub-graphs
                let main_content: Element<'_, Message> = column![
                    toolbar,
                    row![iced::widget::container(palette).width(180), canvas,]
                        .width(Fill)
                        .height(Fill),
                ]
                .width(Fill)
                .height(Fill)
                .into();

                if ctx.shade_nav_path.is_empty() {
                    main_content
                } else {
                    let crumb_bg = t.bg_secondary;
                    let crumb_text = t.text_accent;
                    let bar_bg = t.bg_tertiary;
                    let text_primary = t.text_primary;

                    let mut crumbs: Vec<Element<'_, Message>> = Vec::new();
                    // Root breadcrumb (clickable — exits all sub-graphs)
                    crumbs.push(
                        button(text("Root").size(11))
                            .padding(iced::Padding::from([2, 6]))
                            .on_press(Message::ShadeGraphMsg(
                                crate::editor::canvas::GraphMessage::ExitSubGraph,
                            ))
                            .style(move |_: &iced::Theme, _| iced::widget::button::Style {
                                background: Some(iced::Background::Color(crumb_bg)),
                                text_color: crumb_text,
                                border: iced::Border::default().rounded(3),
                                ..Default::default()
                            })
                            .into(),
                    );
                    // Nav path labels
                    for label in &ctx.shade_nav_labels {
                        crumbs.push(text(" > ").size(11).color(t.text_secondary).into());
                        crumbs.push(text(label.clone()).size(11).color(text_primary).into());
                    }

                    let breadcrumb_bar =
                        container(row(crumbs).spacing(2).align_y(iced::Alignment::Center))
                            .padding(iced::Padding::from([4, 8]))
                            .width(Fill)
                            .style(move |_: &iced::Theme| container::Style {
                                background: Some(iced::Background::Color(bar_bg)),
                                ..Default::default()
                            });

                    column![breadcrumb_bar, main_content]
                        .width(Fill)
                        .height(Fill)
                        .into()
                }
            }
            "code" => {
                use iced::widget::text_editor;
                column![
                    text(format!("Editing: {}", short))
                        .size(11)
                        .color(t.text_secondary),
                    text_editor(ctx.shade_shader_content)
                        .on_action(Message::ShadeShaderChanged)
                        .height(Fill),
                ]
                .spacing(4)
                .width(Fill)
                .height(Fill)
                .into()
            }
            _ => {
                // Preview — compile and show result
                column![
                    text(format!("Preview: {}", short))
                        .size(11)
                        .color(t.text_secondary),
                    text("Compile and preview will appear here")
                        .size(10)
                        .color(t.text_secondary),
                ]
                .spacing(4)
                .width(Fill)
                .height(Fill)
                .into()
            }
        };

        column![mode_bar, content]
            .spacing(2)
            .width(Fill)
            .height(Fill)
            .into()
    }
    // TODO: Use mime-type to deicde not extensions
    /// View for asset files — image/video/font/audio/text preview.
    fn view_asset<'a>(&'a self, ctx: AppContext<'a>, asset_name: &str) -> Element<'a, Message> {
        let t = ctx.tokens;

        let short = asset_name.rsplit('/').next().unwrap_or(asset_name);
        let _ext = short.rsplit('.').next().unwrap_or("").to_lowercase();

        // Header with file name
        let _header: Element<'_, Message> =
            iced::widget::row![text(short).size(12).color(t.text_primary),]
                .spacing(4)
                .padding(iced::Padding::from([4, 8]))
                .into();

        let short = asset_name.rsplit('/').next().unwrap_or(asset_name);
        let ext = short.rsplit('.').next().unwrap_or("").to_lowercase();

        if let Some(pkg) = ctx.shade_package {
            if let Some(data) = pkg.read_asset(asset_name) {
                let size = data.len();
                let size_str = if size > 1_048_576 {
                    format!("{:.1} MB", size as f64 / 1_048_576.0)
                } else if size > 1024 {
                    format!("{:.1} KB", size as f64 / 1024.0)
                } else {
                    format!("{} bytes", size)
                };
                let data = data.as_slice();

                let content: Element<'_, Message> = match ext.as_str() {
                    "glsl" | "frag" | "vert" => {
                        let source = String::from_utf8_lossy(data);
                        column![
                            iced::widget::row![
                                text(icons::CODE).font(icons::ICON_FONT).size(12),
                                text(format!(" {}", short)).size(12),
                            ]
                            .spacing(2)
                            .align_y(iced::Alignment::Center),
                            text(size_str.clone()).size(10).color(t.text_secondary),
                            scrollable(text(source.to_string()).size(10))
                                .width(Fill)
                                .height(Fill),
                        ]
                        .spacing(4)
                        .into()
                    }
                    "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" => {
                        build_image_preview(short, &size_str, data, t)
                    }
                    "mp4" | "webm" | "avi" | "mkv" => {
                        build_video_preview(short, &size_str, data, t, asset_name, ctx.video_player)
                    }
                    "ttf" | "otf" | "woff" | "woff2" => {
                        build_font_preview(short, &size_str, data, t)
                    }
                    "mp3" | "wav" | "ogg" | "flac" => column![
                        iced::widget::row![
                            text(icons::AUDIO).font(icons::ICON_FONT).size(12),
                            text(format!(" {}", short)).size(12),
                        ]
                        .spacing(2)
                        .align_y(iced::Alignment::Center),
                        text(size_str.clone()).size(10).color(t.text_secondary),
                        build_audio_info(data, t),
                    ]
                    .spacing(4)
                    .into(),
                    "toml" | "json" | "yaml" | "yml" | "txt" | "md" => {
                        let source = String::from_utf8_lossy(data);
                        column![
                            iced::widget::row![
                                text(icons::FILE).font(icons::ICON_FONT).size(12),
                                text(format!(" {}", short)).size(12),
                            ]
                            .spacing(2)
                            .align_y(iced::Alignment::Center),
                            text(size_str.clone()).size(10).color(t.text_secondary),
                            scrollable(text(source.to_string()).size(10))
                                .width(Fill)
                                .height(Fill),
                        ]
                        .spacing(4)
                        .into()
                    }
                    _ => column![
                        iced::widget::row![
                            text(icons::FILE).font(icons::ICON_FONT).size(12),
                            text(format!(" {}", short)).size(12),
                        ]
                        .spacing(2)
                        .align_y(iced::Alignment::Center),
                        text(size_str.clone()).size(10).color(t.text_secondary),
                        text(format!("Binary file ({} extension)", ext))
                            .size(10)
                            .color(t.text_secondary),
                    ]
                    .spacing(4)
                    .into(),
                };

                return container(content)
                    .width(Fill)
                    .height(Fill)
                    .padding(8)
                    .into();
            }
        }

        container(
            text(format!("Asset not found: {}", asset_name))
                .size(11)
                .color(t.warning),
        )
        .width(Fill)
        .height(Fill)
        .padding(8)
        .into()
    }

    /// Build a mode toggle bar (e.g., Settings/TOML or Nodes/Code/Preview).
    fn build_mode_bar<'a>(
        &self,
        modes: &[&str],
        labels: &[&str],
        active_mode: &str,
        t: &crate::theme::ThemeTokens,
    ) -> Element<'a, Message> {
        use iced::widget::{button, row};
        use iced::{Border, Padding, Theme};

        let active_bg = t.tab_active;
        let inactive_bg = t.tab_inactive;
        let txt = t.text_primary;
        let radius = t.border_radius;

        let mut bar_items: Vec<Element<'_, Message>> = Vec::new();
        for (mode, label) in modes.iter().zip(labels.iter()) {
            let is_active = active_mode == *mode;
            let bg = if is_active { active_bg } else { inactive_bg };
            let mode_str = mode.to_string();
            let label_owned = label.to_string();
            bar_items.push(
                button(text(label_owned).size(11).color(txt))
                    .on_press(Message::ShadeEditMode(mode_str))
                    .padding(Padding::from([4, 10]))
                    .style(move |_: &Theme, _status| button::Style {
                        background: Some(iced::Background::Color(bg)),
                        text_color: txt,
                        border: Border::default().rounded(radius * 0.5),
                        ..Default::default()
                    })
                    .into(),
            );
        }

        row(bar_items)
            .spacing(2)
            .padding(Padding::from([2, 4]))
            .into()
    }
}

// ---------------------------------------------------------------------------
// Image preview — decode PNG/JPEG/etc. and display via iced image widget
// ---------------------------------------------------------------------------

fn build_image_preview<'a>(
    name: &str,
    size_str: &str,
    data: &[u8],
    tokens: &crate::theme::ThemeTokens,
) -> Element<'a, Message> {
    use iced::widget::image as img;

    // Try to decode image dimensions using the image crate
    match image::load_from_memory(data) {
        Ok(decoded) => {
            let (w, h) = (decoded.width(), decoded.height());
            let rgba = decoded.to_rgba8();
            let handle = img::Handle::from_rgba(w, h, rgba.into_raw());

            column![
                iced::widget::row![
                    text(icons::IMAGE).font(icons::ICON_FONT).size(12),
                    text(format!(" {}", name)).size(12),
                ]
                .spacing(2)
                .align_y(iced::Alignment::Center),
                text(format!("{} | {}x{}", size_str, w, h))
                    .size(10)
                    .color(tokens.text_secondary),
                img::Image::new(handle)
                    .width(Fill)
                    .height(Fill)
                    .content_fit(iced::ContentFit::Contain),
            ]
            .spacing(4)
            .width(Fill)
            .height(Fill)
            .into()
        }
        Err(e) => column![
            iced::widget::row![
                text(icons::IMAGE).font(icons::ICON_FONT).size(12),
                text(format!(" {}", name)).size(12),
            ]
            .spacing(2)
            .align_y(iced::Alignment::Center),
            text(size_str.to_owned())
                .size(10)
                .color(tokens.text_secondary),
            text(format!("Failed to decode: {}", e))
                .size(10)
                .color(tokens.error),
        ]
        .spacing(4)
        .into(),
    }
}

// ---------------------------------------------------------------------------
// Font preview — show sample text at multiple sizes
// ---------------------------------------------------------------------------
// TODO: Proper font preview
fn build_font_preview<'a>(
    name: &str,
    size_str: &str,
    _data: &[u8],
    tokens: &crate::theme::ThemeTokens,
) -> Element<'a, Message> {
    // We can't easily load a custom font into iced at runtime without
    // registering it as an application font. Show metadata & sample glyphs
    // rendered with the system font as a proxy.
    let sample_text = "The quick brown fox jumps over the lazy dog";
    let pangram_upper = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let digits = "0123456789 !@#$%^&*()";

    column![
        iced::widget::row![
            text(icons::FONT).font(icons::ICON_FONT).size(12),
            text(format!(" {}", name)).size(12),
        ]
        .spacing(2)
        .align_y(iced::Alignment::Center),
        text(size_str.to_owned())
            .size(10)
            .color(tokens.text_secondary),
        iced::widget::Space::new().height(8),
        text("Sample (system font):")
            .size(10)
            .color(tokens.text_secondary),
        text(sample_text.to_string()).size(24),
        text(sample_text.to_string()).size(16),
        text(sample_text.to_string()).size(12),
        iced::widget::Space::new().height(4),
        text(pangram_upper.to_string()).size(18),
        text(digits.to_string())
            .size(14)
            .color(tokens.text_secondary),
    ]
    .spacing(4)
    .width(Fill)
    .into()
}

// ---------------------------------------------------------------------------
// Video preview — decode first frame with FFmpeg and display as image
// ---------------------------------------------------------------------------
// TODO: Auto create player
fn build_video_preview<'a>(
    name: &str,
    size_str: &str,
    data: &[u8],
    tokens: &crate::theme::ThemeTokens,
    asset_name: &str,
    video_player: Option<&crate::video::VideoPlayerState>,
) -> Element<'a, Message> {
    use iced::widget::{button, image as img, row};

    // If we have an active player for this asset, show playback UI
    if let Some(player) = video_player {
        if player.asset_name == asset_name {
            if let Some((w, h, ref rgba)) = player.current_frame {
                let handle = img::Handle::from_rgba(w, h, rgba.clone());
                let pos_str = format!("{:.1}s / {:.1}s", player.position, player.duration);
                let progress = if player.duration > 0.0 {
                    (player.position / player.duration) as f32
                } else {
                    0.0
                };

                let play_icon = if player.playing {
                    icons::PAUSE
                } else {
                    icons::PLAY
                };

                return column![
                    iced::widget::row![
                        text(icons::VIDEO).font(icons::ICON_FONT).size(12),
                        text(format!(" {}", name)).size(12),
                    ]
                    .spacing(2)
                    .align_y(iced::Alignment::Center),
                    text(format!("{} | {}", size_str, player.format_info))
                        .size(10)
                        .color(tokens.text_secondary),
                    img::Image::new(handle)
                        .width(Fill)
                        .content_fit(iced::ContentFit::Contain),
                    // Controls bar
                    row![
                        button(text(play_icon).font(icons::ICON_FONT).size(14))
                            .on_press(Message::VideoTogglePlay)
                            .padding(4),
                        text(pos_str.clone()).size(10).color(tokens.text_secondary),
                        slider(0.0..=1.0, progress, Message::VideoSeek)
                            .width(Fill)
                            .step(0.01),
                        button(text(icons::STOP).font(icons::ICON_FONT).size(14))
                            .on_press(Message::VideoStop)
                            .padding(4),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                ]
                .spacing(4)
                .width(Fill)
                .height(Fill)
                .into();
            }
        }
    }

    // Fast-path: empty data can't be a video
    if data.is_empty() {
        return column![
            iced::widget::row![
                text(icons::VIDEO).font(icons::ICON_FONT).size(12),
                text(format!(" {}", name)).size(12),
            ]
            .spacing(2)
            .align_y(iced::Alignment::Center),
            text("Empty video file").size(10).color(tokens.error),
        ]
        .spacing(4)
        .into();
    }

    // Sanitize filename for temp path (replace / with _)
    let safe_name = name.replace('/', "_");
    // Use /dev/shm (tmpfs, memory-backed) on Linux to avoid disk I/O
    let shm_dir = std::path::Path::new("/dev/shm");
    let base_dir = if shm_dir.is_dir() {
        shm_dir.to_path_buf()
    } else {
        std::env::temp_dir()
    };
    let temp_path = base_dir.join(format!("kroma_preview_{}", safe_name));
    if let Err(e) = std::fs::write(&temp_path, data) {
        return column![
            iced::widget::row![
                text(icons::VIDEO).font(icons::ICON_FONT).size(12),
                text(format!(" {}", name)).size(12),
            ]
            .spacing(2)
            .align_y(iced::Alignment::Center),
            text(size_str.to_owned())
                .size(10)
                .color(tokens.text_secondary),
            text(format!("Cannot write temp file: {}", e))
                .size(10)
                .color(tokens.error),
        ]
        .spacing(4)
        .into();
    }

    // Initialize ffmpeg (safe to call multiple times)
    let _ = ffmpeg_next::init();

    let result = decode_first_frame(&temp_path);

    // Clean up temp file regardless of success/failure
    let _ = std::fs::remove_file(&temp_path);

    match result {
        Ok((w, h, rgba_data, duration)) => {
            let handle = img::Handle::from_rgba(w, h, rgba_data);
            let dur_str = if duration > 0.0 {
                format!(" | {:.1}s", duration)
            } else {
                String::new()
            };
            let owned_name = asset_name.to_string();

            column![
                iced::widget::row![
                    text(icons::VIDEO).font(icons::ICON_FONT).size(12),
                    text(format!(" {}", name)).size(12),
                ]
                .spacing(2)
                .align_y(iced::Alignment::Center),
                text(format!("{} | {}x{}{}", size_str, w, h, dur_str))
                    .size(10)
                    .color(tokens.text_secondary),
                img::Image::new(handle)
                    .width(Fill)
                    .height(Fill)
                    .content_fit(iced::ContentFit::Contain),
                iced::widget::button(
                    iced::widget::row![
                        text(icons::PLAY).font(icons::ICON_FONT).size(11),
                        text(" Play").size(11),
                    ]
                    .spacing(2)
                    .align_y(iced::Alignment::Center)
                )
                .on_press(Message::VideoPlay(owned_name))
                .padding(iced::Padding::from([4, 12])),
            ]
            .spacing(4)
            .width(Fill)
            .height(Fill)
            .into()
        }
        Err(e) => column![
            iced::widget::row![
                text(icons::VIDEO).font(icons::ICON_FONT).size(12),
                text(format!(" {}", name)).size(12),
            ]
            .spacing(2)
            .align_y(iced::Alignment::Center),
            text(size_str.to_owned())
                .size(10)
                .color(tokens.text_secondary),
            text(format!("Preview error: {}", e))
                .size(10)
                .color(tokens.error),
        ]
        .spacing(4)
        .into(),
    }
}

/// Decoded first frame data: (width, height, rgba_bytes, duration_secs).
pub type FirstFrame = (u32, u32, Vec<u8>, f64);

/// Decode the first video frame from a file path, returning (width, height, rgba_bytes, duration).
pub fn decode_first_frame(
    path: &std::path::Path,
) -> Result<FirstFrame, Box<dyn std::error::Error>> {
    let mut input = ffmpeg_next::format::input(path)?;

    let video_stream = input
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .ok_or("No video stream found")?;

    let video_stream_index = video_stream.index();
    let time_base = f64::from(video_stream.time_base());
    let raw_duration = video_stream.duration();
    let duration = if raw_duration > 0 {
        raw_duration as f64 * time_base
    } else {
        0.0
    };

    let context_decoder =
        ffmpeg_next::codec::context::Context::from_parameters(video_stream.parameters())?;
    let mut decoder = context_decoder.decoder().video()?;

    let width = decoder.width();
    let height = decoder.height();

    let mut scaler = ffmpeg_next::software::scaling::Context::get(
        decoder.format(),
        width,
        height,
        ffmpeg_next::format::Pixel::RGBA,
        width,
        height,
        ffmpeg_next::software::scaling::Flags::BILINEAR,
    )?;

    // Feed packets until we decode the first frame
    for (stream, packet) in input.packets() {
        if stream.index() != video_stream_index {
            continue;
        }
        decoder.send_packet(&packet)?;
        let mut decoded = ffmpeg_next::util::frame::Video::empty();
        if decoder.receive_frame(&mut decoded).is_ok() {
            let mut rgba_frame = ffmpeg_next::util::frame::Video::empty();
            scaler.run(&decoded, &mut rgba_frame)?;

            let data = rgba_frame.data(0);
            let stride = rgba_frame.stride(0);
            let w = width as usize;
            let h = height as usize;
            let mut buf = vec![0u8; w * h * 4];
            for y in 0..h {
                let src_start = y * stride;
                let dst_start = y * w * 4;
                let row_bytes = w * 4;
                if src_start + row_bytes <= data.len() && dst_start + row_bytes <= buf.len() {
                    buf[dst_start..dst_start + row_bytes]
                        .copy_from_slice(&data[src_start..src_start + row_bytes]);
                }
            }
            return Ok((width, height, buf, duration));
        }
    }

    Err("Could not decode any video frame".into())
}

// ---------------------------------------------------------------------------
// Audio info — basic format info from header bytes
// ---------------------------------------------------------------------------
// TODO: Use FFMPEG for perfect info and playback
fn build_audio_info<'a>(data: &[u8], tokens: &crate::theme::ThemeTokens) -> Element<'a, Message> {
    // Parse WAV header for basic info, otherwise just show size
    if data.len() >= 44 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
        let channels = u16::from_le_bytes([data[22], data[23]]);
        let sample_rate = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
        let bits_per_sample = u16::from_le_bytes([data[34], data[35]]);
        let data_size = if data.len() > 40 {
            u32::from_le_bytes([data[40], data[41], data[42], data[43]])
        } else {
            0
        };
        let duration_secs = if sample_rate > 0 && channels > 0 && bits_per_sample > 0 {
            data_size as f64 / (sample_rate as f64 * channels as f64 * bits_per_sample as f64 / 8.0)
        } else {
            0.0
        };

        column![
            text(format!(
                "WAV: {}ch, {} Hz, {}-bit",
                channels, sample_rate, bits_per_sample
            ))
            .size(10)
            .color(tokens.text_secondary),
            text(format!("Duration: {:.1}s", duration_secs))
                .size(10)
                .color(tokens.text_secondary),
        ]
        .spacing(2)
        .into()
    } else {
        text("Audio format info unavailable")
            .size(10)
            .color(tokens.text_secondary)
            .into()
    }
}
