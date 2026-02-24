//! Kroma Daemon — the headless wallpaper rendering engine.
//!
//! This binary holds the wgpu context, manages the render loop,
//! data aggregation threads, and IPC communication with the GUI.

mod audio;
mod backend;
mod config;
mod data;
mod font;
mod ipc_server;
mod renderer;
mod video;

use std::{
    env, fs,
    hash::{DefaultHasher, Hash, Hasher},
    sync::mpsc,
    time::Instant,
};

use anyhow::Result;
use glam::Vec2;
use log::info;

use kroma_shared::{
    ipc::{CompileError, DaemonCommand, DaemonEvent, maybe_send},
    shade::LiveShadePackage,
    traits::{DataProvider, SurfaceProvider, VideoDecoder},
};

use crate::{
    audio::{AudioProvider, CpalAudioProvider},
    backend::{
        Backend,
        wayland::{WaylandBackend, hyprland::HyprlandEvent},
    },
    data::SystemDataProvider,
    renderer::{RenderState, SlideshowEvent},
    video::DefaultVideoDecoder,
};

fn video_decoder_for_source(
    pkg: &LiveShadePackage,
    source: &String,
) -> Option<DefaultVideoDecoder> {
    // We need to know the index of each texture to update the correct slot in RenderState.
    // RenderState sorts textures by binding index. We must replicate that sort order here.
    if let Some(video_data) = pkg.read_asset(source) {
        match extract_video_to_temp(source, &video_data) {
            Ok(temp_path) => match DefaultVideoDecoder::load(&temp_path) {
                Ok(decoder) => Some(decoder),
                Err(e) => {
                    log::warn!("Decoder error: {}", e);
                    None
                }
            },
            Err(e) => {
                log::warn!("Extract error: {}", e);
                None
            }
        }
    }
    // 2. Try loading from disk (Folder package)
    else {
        let video_path = std::path::Path::new(source);
        match DefaultVideoDecoder::load(video_path) {
            Ok(decoder) => {
                log::info!("Video decoder created for texture ({})", source);
                Some(decoder)
            }
            Err(e) => {
                log::warn!("Failed to create video decoder for '{}': {}", source, e);
                None
            }
        }
    }
}

/// Try to create a video decoder for a shade package.
///
/// Scans the package's texture definitions for any `ty == "video"` entries
/// and attempts to create an FFmpeg decoder for the first one found.
/// Works regardless of WallpaperMode — any package can include video textures.
/// Try to create a video decoder for a shade package.
///
/// Scans the package's texture definitions for any `ty == "video"` entries
/// and attempts to create an FFmpeg decoder for the first one found.
/// Works regardless of WallpaperMode — any package can include video textures.
fn try_create_video_decoders(pkg: &LiveShadePackage) -> Vec<(usize, DefaultVideoDecoder)> {
    let mut decoders = Vec::new();

    // We need to know the index of each texture to update the correct slot in RenderState.
    // RenderState sorts textures by binding index. We must replicate that sort order here.
    let mut tex_defs: Vec<_> = pkg.config.textures.iter().collect();
    tex_defs.sort_by_key(|(_, def)| def.binding.unwrap_or(u32::MAX));

    // Iterate through sorted textures to match RenderState's internal `self.textures` vector
    for (i, (_name, def)) in tex_defs.iter().enumerate() {
        if def.ty == "video" {
            if let Some(ref source) = def.source {
                // 1. Try loading from embedded assets (ZIP package)
                if let Some(d) = video_decoder_for_source(pkg, source) {
                    decoders.push((i, d))
                }
            }
        }
    }

    decoders
}

/// Extract embedded video data to a temp file so FFmpeg can open it.
fn extract_video_to_temp(source: &str, data: &[u8]) -> Result<std::path::PathBuf> {
    let extension = std::path::Path::new(source)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4");
    let temp_dir = std::env::temp_dir().join("kroma-video");
    std::fs::create_dir_all(&temp_dir)?;
    // Use a unique filename based on content hash to avoid clobbering
    // when multiple video textures or daemon instances exist.
    let hash = {
        let mut h = DefaultHasher::new();
        source.hash(&mut h);
        data.len().hash(&mut h);
        h.finish()
    };
    let temp_path = temp_dir.join(format!("kroma-video_{:016x}.{}", hash, extension));
    std::fs::write(&temp_path, data)?;
    log::info!("Extracted video to temp: {}", temp_path.display());
    Ok(temp_path)
}

fn load_shade(
    path: &str,
    render_state: &mut RenderState,
    audio_provider: &mut CpalAudioProvider,
    tx: Option<mpsc::Sender<DaemonEvent>>,
) -> Result<(Vec<(usize, DefaultVideoDecoder)>, Vec<f64>)> {
    let mut video_decoders = vec![];
    let mut video_frame_accums = vec![];
    match LiveShadePackage::load(std::path::Path::new(path)) {
        Ok(pkg) => {
            if let Some(audio_conf) = pkg.config.audio.as_ref()
                && audio_conf.enabled
            {
                audio_provider.switch(&audio_conf)?;
            } else {
                audio_provider.close();
            }

            if pkg.config.slideshow.is_none() {
                video_decoders = try_create_video_decoders(&pkg);
            }
            video_frame_accums = [0.0].repeat(video_decoders.len());
            let pkg_name = pkg.config.meta.name.clone();
            match render_state.load_shade(pkg) {
                Ok(_) => {
                    info!("Loaded: {}", pkg_name);
                    maybe_send(
                        tx,
                        DaemonEvent::CompileResult {
                            success: true,
                            errors: vec![],
                            warnings: vec![],
                        },
                    )?;
                }
                Err(e) => {
                    log::error!("Failed to compile shader: {}", e);
                    maybe_send(
                        tx,
                        DaemonEvent::CompileResult {
                            success: false,
                            errors: vec![CompileError {
                                message: e.to_string(),
                                line: None,
                                column: None,
                            }],
                            warnings: vec![],
                        },
                    )?;
                }
            }
        }
        Err(e) => {
            log::error!("Failed to load shade: {}", e);
            maybe_send(
                tx,
                DaemonEvent::CompileResult {
                    success: false,
                    errors: vec![CompileError {
                        message: format!("Package load error: {}", e),
                        line: None,
                        column: None,
                    }],
                    warnings: vec![],
                },
            )?;
        }
    }
    Ok((video_decoders, video_frame_accums))
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    info!("Kroma Daemon v{}", env!("CARGO_PKG_VERSION"));
    info!("Initializing...");

    // ---------------------------------------------------------------
    // 0. Load daemon configuration
    // ---------------------------------------------------------------
    let daemon_config = config::DaemonConfig::load()?;
    info!(
        "Target FPS: {}, GPU power: {:?}",
        daemon_config.target_fps, daemon_config.gpu_power
    );

    let session_type = env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let desktop_env = env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();

    let mut backend = Backend::new(&session_type, &desktop_env)?;
    let data_provider = SystemDataProvider::new()?;
    let mut audio_provider = CpalAudioProvider::new();

    // ---------------------------------------------------------------
    // 3. Start the IPC server (async, background)
    // ---------------------------------------------------------------
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (_ipc_handle, ipc_status, preview_stream) = ipc_server::start(cmd_tx)?;
    info!("IPC server listening");

    // ---------------------------------------------------------------
    // 4. Initialize the renderer
    // ---------------------------------------------------------------
    let mut render_state = RenderState::new()?;
    let (surf_w, surf_h): (u32, u32);

    match backend.surface() {
        Some(surface) => {
            // Rust's or-pattern (|) works here because 'surface' is
            // the same type or satisfies the trait in both variants.
            info!("Initializing GPU with surface...");

            if let Err(e) = render_state.init_gpu_with_surface(surface) {
                log::error!("GPU init with surface failed: {}", e);
            } else {
                info!("GPU initialized with surface");
            }

            (surf_w, surf_h) = surface.size(
                surface
                    .list_monitors()?
                    .first()
                    .expect("At least one monitor should exist")
                    .id,
            )?
        }
        None => {
            surf_w = 1920;
            surf_h = 1080;
            render_state.init_gpu_headless()?;
        }
    }

    // Set resolution
    render_state.uniforms.u_resolution = [surf_w as f32, surf_h as f32];

    // Load initial shade if configured
    let mut current_shade_path: Option<String> = None;
    let mut video_decoders: Vec<(usize, DefaultVideoDecoder)> = vec![];
    let mut video_frame_accums: Vec<f64> = vec![]; // Time accumulator for video frame pacing
    if let Some(ref shade_path) = daemon_config.current_shade {
        info!("Loading initial shade: {}", shade_path);
        (video_decoders, video_frame_accums) =
            load_shade(shade_path, &mut render_state, &mut audio_provider, None)?;
    }

    // ---------------------------------------------------------------
    // 5. Main loop
    // ---------------------------------------------------------------
    info!(
        "Entering render loop ({}x{} @ {} FPS target)",
        surf_w, surf_h, daemon_config.target_fps
    );
    let start_time = Instant::now();
    let mut frame: u32 = 0;
    let mut paused = false;
    let mut active_workspace_id: i64 = 1;
    let mut last_frame_time = Instant::now();
    let frame_budget = daemon_config.frame_budget();

    // FPS tracking
    let mut fps_counter: u32 = 0;
    let mut fps_timer = Instant::now();
    let mut current_fps: f32;

    loop {
        // Process Hyprland compositor events (non-blocking)
        if let Backend::Wayland {
            backend: WaylandBackend::Hyprland { rx: hypr_rx, .. },
            ..
        } = &mut backend
        {
            while let Ok(event) = hypr_rx.try_recv() {
                match event {
                    HyprlandEvent::Fullscreen { fullscreen }
                        if daemon_config.pause_on_fullscreen =>
                    {
                        if fullscreen {
                            paused = true;
                            info!("Fullscreen detected — pausing render");
                        } else {
                            paused = false;
                            info!("Fullscreen exited — resuming render");
                        }
                    }
                    HyprlandEvent::WorkspaceChanged { id } => {
                        info!("Workspace changed to {} (was {})", id, active_workspace_id);
                        active_workspace_id = id;
                        // Pause when workspace changes away (wallpaper is always on all workspaces,
                        // but we can save GPU cycles when the user isn't looking at it).
                        if daemon_config.pause_on_inactive {
                            // On Hyprland the wallpaper layer is visible on every workspace,
                            // so we interpret "inactive" as special workspaces (negative IDs)
                            // which are overlaid and hide the desktop.
                            if id < 0 {
                                if !paused {
                                    paused = true;
                                    info!("Special workspace active — pausing render");
                                }
                            } else if paused {
                                // Only resume if fullscreen doesn't keep us paused
                                paused = false;
                                info!("Normal workspace active — resuming render");
                            }
                        }
                    }
                    HyprlandEvent::MonitorChanged { ref name } => {
                        info!("Active monitor: {}", name);
                    }
                    HyprlandEvent::Disconnected => {
                        log::warn!("Hyprland event socket disconnected");
                    }
                    _ => {}
                }
            }
        };

        // Process IPC commands (non-blocking)
        while let Ok(internal) = cmd_rx.try_recv() {
            let ipc_server::InternalCommand {
                command,
                response_tx,
            } = internal;

            match command {
                DaemonCommand::Pause => {
                    paused = true;
                    info!("Rendering paused");
                }
                DaemonCommand::Resume => {
                    paused = false;
                    info!("Rendering resumed");
                }
                DaemonCommand::Shutdown => {
                    info!("Shutdown requested — cleaning up");
                    let sock = kroma_shared::ipc::socket_path();
                    if sock.exists() {
                        let _ = fs::remove_file(&sock);
                    }
                    return Ok(());
                }
                DaemonCommand::LoadShade { path } => {
                    info!("Loading shade package: {}", path);
                    (video_decoders, video_frame_accums) =
                        load_shade(&path, &mut render_state, &mut audio_provider, response_tx)?;
                }
                DaemonCommand::Reload => {
                    if let Some(ref path) = current_shade_path {
                        info!("Reloading shade: {}", path);
                        (video_decoders, video_frame_accums) =
                            load_shade(path, &mut render_state, &mut audio_provider, response_tx)?;
                    } else {
                        log::warn!("No shade loaded to reload");
                        maybe_send(
                            response_tx,
                            DaemonEvent::Error {
                                message: "No shade loaded to reload".into(),
                            },
                        )?;
                    }
                }
                DaemonCommand::SetUniform { name, value } => {
                    log::debug!("Setting uniform {} = {:?}", name, value);
                    render_state.set_custom_uniform(&name, &value);
                }
                DaemonCommand::StatusQuery => {
                    // Handled inline in ipc_server, shouldn't reach here
                }
                DaemonCommand::QuerySystemInfo => {
                    // Handled inline in ipc_server, shouldn't reach here
                }
                DaemonCommand::RequestPreviewFrame { width, height } => {
                    match render_state.capture_preview_frame(width, height) {
                        Ok(jpeg_bytes) => {
                            maybe_send(
                                response_tx,
                                DaemonEvent::PreviewFrame {
                                    jpeg: jpeg_bytes,
                                    width,
                                    height,
                                },
                            )?;
                        }
                        Err(e) => {
                            log::warn!("Preview capture failed: {}", e);
                            maybe_send(
                                response_tx,
                                DaemonEvent::Error {
                                    message: format!("Preview capture failed: {}", e),
                                },
                            )?;
                        }
                    }
                }
                DaemonCommand::StartPreviewStream { .. } | DaemonCommand::StopPreviewStream => {
                    // Handled inline in ipc_server
                }
                DaemonCommand::LiveReload { glsl_source } => {
                    log::info!("Live reload: {} bytes of GLSL", glsl_source.len());
                    // Translate the raw Shadertoy GLSL with our translator first
                    let result = kroma_shared::translator::translate(
                        &glsl_source,
                        "live-preview",
                        "Kroma Editor",
                    );
                    let warnings: Vec<String> = result.warnings.clone();
                    for w in &warnings {
                        log::warn!("Translation warning: {}", w);
                    }
                    match render_state.load_glsl_source(&result.shader_source) {
                        Ok(()) => {
                            current_shade_path = Some("live-preview".to_string());
                            maybe_send(
                                response_tx,
                                DaemonEvent::CompileResult {
                                    success: true,
                                    errors: vec![],
                                    warnings,
                                },
                            )?;
                        }
                        Err(e) => {
                            log::error!("Live reload failed: {}", e);
                            // Try to extract line numbers from error message
                            let compile_error = CompileError::from(e.to_string());
                            maybe_send(
                                response_tx,
                                DaemonEvent::CompileResult {
                                    success: false,
                                    errors: vec![compile_error],
                                    warnings,
                                },
                            )?;
                        }
                    }
                }
            }
        }

        if paused {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }

        // Dispatch Wayland events
        if let Backend::Wayland { surface, .. } = &mut backend {
            if let Err(e) = surface.dispatch() {
                log::warn!("Wayland dispatch error: {}", e);
            }
        }

        // Update timing uniforms
        let now = Instant::now();
        let dt = now.duration_since(last_frame_time).as_secs_f32();
        last_frame_time = now;

        render_state.uniforms.u_time = start_time.elapsed().as_secs_f32();
        render_state.uniforms.u_delta_time = dt;
        render_state.uniforms.u_frame = frame;

        // Gather system data
        let stats = data_provider.get_system_stats();
        render_state.uniforms.apply_system_stats(&stats);
        let cursor = data_provider.get_cursor_pos();
        // Flip mouse Y: Hyprland uses Y=0 at top, Shadertoy expects Y=0 at bottom
        let flipped_cursor = Vec2::new(cursor.x, render_state.uniforms.u_resolution[1] - cursor.y);
        render_state.uniforms.apply_cursor(flipped_cursor);

        // Gather audio data
        let audio_spectrum = audio_provider.get_spectrum();
        let audio_level = audio_provider.get_level();
        render_state.uniforms.u_audio_level = audio_level;

        // Upload audio spectrum to GPU texture
        render_state.update_audio_spectrum(&audio_spectrum);

        // Advance slideshow if active

        match render_state.update_slideshow(dt as f64) {
            Ok(res) => match res {
                SlideshowEvent::SwappedToVideo { source } => {
                    video_decoders = vec![(
                        0,
                        video_decoder_for_source(
                            render_state.active_package.as_ref().unwrap(),
                            &source,
                        )
                        .unwrap(),
                    )];
                    video_frame_accums = [0.0].repeat(video_decoders.len());
                }
                SlideshowEvent::SwappedToImage => {
                    video_decoders = vec![];
                }
                SlideshowEvent::None => (),
            },

            Err(e) => {
                panic!("{:?}", e)
            }
        }

        // Decode and upload next video frame at the video's native FPS

        for (i, (tex_index, decoder)) in video_decoders.iter_mut().enumerate() {
            video_frame_accums[i] += dt as f64;
            while video_frame_accums[i] >= decoder.frame_interval() {
                video_frame_accums[i] -= decoder.frame_interval();
                let (vw, vh) = decoder.dimensions();
                match decoder.next_frame() {
                    Some(rgba_data) => {
                        // Update the specific texture slot associated with this video
                        render_state.update_video_frame(rgba_data, vw, vh, *tex_index);
                    }
                    None => {
                        // Loop video
                        if let Err(e) = decoder.seek(0.0) {
                            log::warn!("Video {} seek failed: {}", tex_index, e);
                        }
                    }
                }
            }
        }

        // Render frame
        render_state.render_frame()?;
        frame = frame.wrapping_add(1);

        // Send preview frame if streaming is active
        if let Ok(mut ps) = preview_stream.try_lock() {
            if ps.active {
                let interval =
                    std::time::Duration::from_secs_f64(1.0 / ps.target_fps.max(1) as f64);
                if ps.last_frame_time.elapsed() >= interval {
                    let pw = ps.width;
                    let ph = ps.height;
                    match render_state.capture_preview_frame(pw, ph) {
                        Ok(jpeg_bytes) => {
                            let event = DaemonEvent::PreviewFrame {
                                jpeg: jpeg_bytes,
                                width: pw,
                                height: ph,
                            };
                            if let Ok(json) = serde_json::to_string(&event) {
                                if let Some(ref mut w) = ps.writer {
                                    use std::io::Write;
                                    if writeln!(w, "{}", json).is_err() || w.flush().is_err() {
                                        // Writer broken — stop streaming
                                        ps.active = false;
                                        ps.writer = None;
                                        log::info!("Preview stream client disconnected");
                                    }
                                }
                            }
                            ps.last_frame_time = std::time::Instant::now();
                        }
                        Err(e) => {
                            log::warn!("Preview capture error: {}", e);
                        }
                    }
                }
            }
        }

        // FPS tracking
        fps_counter += 1;
        if fps_timer.elapsed().as_secs_f32() >= 1.0 {
            current_fps = fps_counter as f32 / fps_timer.elapsed().as_secs_f32();
            let log_interval = daemon_config.target_fps.max(1) * 5;
            if frame % log_interval < daemon_config.target_fps.max(1) {
                info!(
                    "FPS: {:.1} | time: {:.1}s | shader: {}",
                    current_fps,
                    start_time.elapsed().as_secs_f32(),
                    current_shade_path.as_deref().unwrap_or("default")
                );
            }
            fps_counter = 0;
            fps_timer = std::time::Instant::now();

            // Update shared IPC status
            if let Ok(mut status) = ipc_status.lock() {
                status.fps = current_fps;
                status.paused = paused;
                status.loaded_shade = current_shade_path.clone();
                status.cpu_usage = render_state.uniforms.u_cpu * 100.0;
                status.ram_usage = render_state.uniforms.u_ram * 100.0;
                status.battery = if render_state.uniforms.u_battery >= 0.0 {
                    Some(render_state.uniforms.u_battery * 100.0)
                } else {
                    None
                };
                status.audio_level = render_state.uniforms.u_audio_level;
                status.cursor_x = render_state.uniforms.u_mouse[0];
                status.cursor_y = render_state.uniforms.u_mouse[1];
            }
        }

        // Frame rate limiting
        let elapsed = now.elapsed();
        if elapsed < frame_budget {
            std::thread::sleep(frame_budget - elapsed);
        }
    }
}
