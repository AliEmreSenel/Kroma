//! Kroma Daemon — the headless wallpaper rendering engine.
//!
//! This binary holds the wgpu context, manages the render loop,
//! data aggregation threads, and IPC communication with the GUI.

mod audio;
mod config;
mod data;
mod font;
mod hyprland;
mod ipc_server;
mod renderer;
mod surface;
mod surface_x11;
mod video;

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::Path,
};

use anyhow::Result;
use glam::Vec2;
use log::info;

use kroma_shared::{
    shade::LiveShadePackage,
    traits::{DataProvider, SurfaceProvider, VideoDecoder},
};

use crate::{
    audio::{AudioProvider, CpalAudioProvider, SilentAudioProvider},
    renderer::{LoadResult, SlideshowEvent},
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
                Ok(decoder) => {
                    return Some(decoder);
                }
                Err(e) => {
                    log::warn!("Decoder error: {}", e);
                    return None;
                }
            },
            Err(e) => {
                log::warn!("Extract error: {}", e);
                return None;
            }
        }
    }
    // 2. Try loading from disk (Folder package)
    else {
        let video_path = std::path::Path::new(source);
        match DefaultVideoDecoder::load(video_path) {
            Ok(decoder) => {
                log::info!("Video decoder created for texture ({})", source);
                return Some(decoder);
            }
            Err(e) => {
                log::warn!("Failed to create video decoder for '{}': {}", source, e);
                return None;
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
                match video_decoder_for_source(pkg, source) {
                    Some(d) => decoders.push((i, d)),
                    None => (),
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

    // ---------------------------------------------------------------
    // 1. Detect session type and initialize the surface provider
    // ---------------------------------------------------------------
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let desktop_env = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    info!("Session: type={}, desktop={}", session_type, desktop_env);

    // Persisted Wayland surface provider for dispatch in the render loop.
    let mut wayland_surface: Option<surface::WaylandSurfaceProvider> = None;

    enum Backend {
        Wayland {
            display_ptr: Option<std::ptr::NonNull<std::ffi::c_void>>,
            surface_ptr: Option<std::ptr::NonNull<std::ffi::c_void>>,
            logical_w: u32,
            logical_h: u32,
            scale: f64,
            _monitors: Vec<kroma_shared::types::MonitorConfig>,
        },
        X11 {
            window_id: u32,
            screen_num: i32,
            width: u32,
            height: u32,
            _monitors: Vec<kroma_shared::types::MonitorConfig>,
        },
        Headless,
    }

    let backend = if session_type == "wayland" || std::env::var("WAYLAND_DISPLAY").is_ok() {
        info!("Detected Wayland session — using layer shell backend");
        let mut surface_provider = surface::WaylandSurfaceProvider::new();
        match surface_provider
            .connect()
            .and_then(|_| surface_provider.create_all_surfaces())
        {
            Ok(()) => {
                let monitors = surface_provider.list_monitors()?;
                info!("Active on {} Wayland monitor(s):", monitors.len());
                for m in &monitors {
                    info!(
                        "  - {} ({}x{} @ {},{}, scale {})",
                        m.name, m.width, m.height, m.x, m.y, m.scale
                    );
                }
                let primary = monitors.first().cloned().unwrap_or_else(|| {
                    kroma_shared::types::MonitorConfig {
                        id: kroma_shared::types::MonitorId(0),
                        name: "default".into(),
                        width: 1920,
                        height: 1080,
                        x: 0,
                        y: 0,
                        scale: 1.0,
                    }
                });
                let display_ptr = surface_provider.display_ptr();
                let surface_ptr = surface_provider.surface_ptr(primary.id.0);
                let (logical_w, logical_h) = surface_provider
                    .surface_size(primary.id.0)
                    .unwrap_or((primary.width, primary.height));
                let scale = primary.scale.max(1.0);

                wayland_surface = Some(surface_provider);

                Backend::Wayland {
                    display_ptr,
                    surface_ptr,
                    logical_w,
                    logical_h,
                    scale,
                    _monitors: monitors,
                }
            }
            Err(e) => {
                log::warn!(
                    "Wayland surface creation failed: {} — trying X11 fallback",
                    e
                );
                // Try X11 as fallback before going headless
                let mut x11_provider = surface_x11::X11SurfaceProvider::new();
                match x11_provider
                    .connect()
                    .and_then(|_| x11_provider.discover_monitors())
                    .and_then(|_| x11_provider.create_all_windows())
                {
                    Ok(()) => {
                        let monitors = x11_provider.monitors().to_vec();
                        info!(
                            "Wayland failed, fell back to X11 on {} monitor(s):",
                            monitors.len()
                        );
                        for m in &monitors {
                            info!(
                                "  - {} ({}x{} @ {},{})",
                                m.name, m.width, m.height, m.x, m.y
                            );
                        }
                        let primary = monitors.first().cloned().unwrap_or_else(|| {
                            kroma_shared::types::MonitorConfig {
                                id: kroma_shared::types::MonitorId(0),
                                name: "default".into(),
                                width: 1920,
                                height: 1080,
                                x: 0,
                                y: 0,
                                scale: 1.0,
                            }
                        });
                        let window_id = x11_provider.get_window_id(primary.id.0).unwrap_or(0);
                        Backend::X11 {
                            window_id,
                            screen_num: x11_provider.screen_num(),
                            width: primary.width,
                            height: primary.height,
                            _monitors: monitors,
                        }
                    }
                    Err(e2) => {
                        log::warn!("X11 fallback also failed: {} — going headless", e2);
                        Backend::Headless
                    }
                }
            }
        }
    } else if session_type == "x11" || std::env::var("DISPLAY").is_ok() {
        info!("Detected X11 session — using desktop window backend");
        let mut x11_provider = surface_x11::X11SurfaceProvider::new();
        match x11_provider
            .connect()
            .and_then(|_| x11_provider.discover_monitors())
            .and_then(|_| x11_provider.create_all_windows())
        {
            Ok(()) => {
                let monitors = x11_provider.monitors().to_vec();
                info!("Active on {} X11 monitor(s):", monitors.len());
                for m in &monitors {
                    info!(
                        "  - {} ({}x{} @ {},{})",
                        m.name, m.width, m.height, m.x, m.y
                    );
                }
                let primary = monitors.first().cloned().unwrap_or_else(|| {
                    kroma_shared::types::MonitorConfig {
                        id: kroma_shared::types::MonitorId(0),
                        name: "default".into(),
                        width: 1920,
                        height: 1080,
                        x: 0,
                        y: 0,
                        scale: 1.0,
                    }
                });
                let window_id = x11_provider.get_window_id(primary.id.0).unwrap_or(0);
                Backend::X11 {
                    window_id,
                    screen_num: x11_provider.screen_num(),
                    width: primary.width,
                    height: primary.height,
                    _monitors: monitors,
                }
            }
            Err(e) => {
                log::warn!(
                    "X11 surface creation failed: {} — falling back to headless",
                    e
                );
                Backend::Headless
            }
        }
    } else {
        log::warn!("No display server detected — running headless");
        Backend::Headless
    };

    // ---------------------------------------------------------------
    // 2. Start the data provider
    // ---------------------------------------------------------------
    let data_provider = data::SystemDataProvider::new()?;
    info!("Data provider initialized");

    // ---------------------------------------------------------------
    // 2b. Start audio provider
    // ---------------------------------------------------------------
    let audio_provider: Box<dyn AudioProvider> = match CpalAudioProvider::new() {
        Ok(provider) => {
            info!("Using cpal audio provider (real audio capture)");
            Box::new(provider)
        }
        Err(e) => {
            info!("cpal audio unavailable ({}), using silent provider", e);
            Box::new(SilentAudioProvider)
        }
    };

    // ---------------------------------------------------------------
    // 3. Start the IPC server (async, background)
    // ---------------------------------------------------------------
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let (_ipc_handle, ipc_status, preview_stream) = ipc_server::start(cmd_tx)?;
    info!("IPC server listening");

    // ---------------------------------------------------------------
    // 3b. Start Hyprland event listener (optional)
    // ---------------------------------------------------------------
    let (hypr_tx, hypr_rx) = std::sync::mpsc::channel();
    let _hypr_handle = match hyprland::start_listener(hypr_tx) {
        Ok(h) => {
            info!("Hyprland event listener started");
            Some(h)
        }
        Err(e) => {
            log::warn!(
                "Hyprland events unavailable: {} — running without compositor awareness",
                e
            );
            None
        }
    };

    // ---------------------------------------------------------------
    // 4. Initialize the renderer
    // ---------------------------------------------------------------
    let mut render_state = renderer::RenderState::new()?;
    let (surf_w, surf_h): (u32, u32);

    match &backend {
        Backend::Wayland {
            display_ptr,
            surface_ptr,
            logical_w,
            logical_h,
            scale,
            ..
        } => {
            let s = scale.max(1.0);
            surf_w = (*logical_w as f64 * s) as u32;
            surf_h = (*logical_h as f64 * s) as u32;
            if let (Some(dp), Some(sp)) = (*display_ptr, *surface_ptr) {
                info!(
                    "Initializing GPU with Wayland surface ({}x{} physical, scale {:.1})...",
                    surf_w, surf_h, s
                );
                match unsafe { render_state.init_gpu_with_surface(dp, sp, surf_w, surf_h) } {
                    Ok(()) => info!("GPU initialized with Wayland surface"),
                    Err(e) => {
                        log::error!(
                            "GPU init with Wayland surface failed: {} — trying headless",
                            e
                        );
                        render_state.init_gpu_headless()?;
                    }
                }
            } else {
                log::warn!("No Wayland surface pointers available — trying headless");
                render_state.init_gpu_headless()?;
            }
        }
        Backend::X11 {
            window_id,
            screen_num,
            width,
            height,
            ..
        } => {
            surf_w = *width;
            surf_h = *height;
            info!(
                "Initializing GPU with X11 surface ({}x{})...",
                surf_w, surf_h
            );
            match unsafe { render_state.init_gpu_with_x11(*window_id, *screen_num, surf_w, surf_h) }
            {
                Ok(()) => info!("GPU initialized with X11 surface"),
                Err(e) => {
                    log::error!("GPU init with X11 failed: {} — trying headless", e);
                    render_state.init_gpu_headless()?;
                }
            }
        }
        Backend::Headless => {
            surf_w = 1920;
            surf_h = 1080;
            render_state.init_gpu_headless()?;
        }
    }

    // Set resolution
    render_state.uniforms.u_resolution = [surf_w as f32, surf_h as f32];

    // Create audio spectrum texture (must happen after GPU init, before shade load)
    if let Err(e) = render_state.create_audio_spectrum_texture() {
        log::warn!("Failed to create audio spectrum texture: {}", e);
    }

    // Load initial shade if configured
    let mut current_shade_path: Option<String> = None;
    let mut video_decoders: Vec<(usize, DefaultVideoDecoder)> = vec![];
    let mut video_frame_accums: Vec<f64> = vec![]; // Time accumulator for video frame pacing
    if let Some(ref shade_path) = daemon_config.current_shade {
        info!("Loading initial shade: {}", shade_path);
        match LiveShadePackage::load(Path::new(shade_path)) {
            Ok(pkg) => {
                if !(pkg.config.slideshow.interval > 0.0) {
                    video_decoders = try_create_video_decoders(&pkg);
                } else {
                    video_decoders = vec![];
                }
                video_frame_accums = vec![0.0].repeat(video_decoders.len());

                let pkg_name = pkg.config.meta.name.clone();
                match render_state.load_shade(pkg) {
                    Ok(res) => {
                        match res {
                            LoadResult::Slideshow(event) => match event {
                                SlideshowEvent::SwappedToVideo { source } => {
                                    video_decoders.push((
                                        0,
                                        video_decoder_for_source(
                                            render_state.active_package.as_ref().unwrap(),
                                            &source,
                                        )
                                        .unwrap(),
                                    ));
                                }
                                SlideshowEvent::SwappedToImage | SlideshowEvent::None => (),
                            },
                            LoadResult::Other => (),
                        }
                        current_shade_path = Some(shade_path.clone());
                        info!("Initial shade loaded: {}", pkg_name);
                    }
                    Err(e) => log::warn!("Failed to compile initial shade shader: {}", e),
                }
            }
            Err(e) => log::warn!("Failed to load initial shade: {}", e),
        }
    }

    // ---------------------------------------------------------------
    // 5. Main loop
    // ---------------------------------------------------------------
    info!(
        "Entering render loop ({}x{} @ {} FPS target)",
        surf_w, surf_h, daemon_config.target_fps
    );
    let start_time = std::time::Instant::now();
    let mut frame: u32 = 0;
    let mut paused = false;
    let mut active_workspace_id: i64 = 1;
    let mut last_frame_time = std::time::Instant::now();
    let frame_budget = daemon_config.frame_budget();

    // FPS tracking
    let mut fps_counter: u32 = 0;
    let mut fps_timer = std::time::Instant::now();
    let mut current_fps: f32;

    loop {
        // Process Hyprland compositor events (non-blocking)
        while let Ok(event) = hypr_rx.try_recv() {
            match event {
                hyprland::HyprlandEvent::Fullscreen { fullscreen }
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
                hyprland::HyprlandEvent::WorkspaceChanged { id } => {
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
                hyprland::HyprlandEvent::MonitorChanged { ref name } => {
                    info!("Active monitor: {}", name);
                }
                hyprland::HyprlandEvent::Disconnected => {
                    log::warn!("Hyprland event socket disconnected");
                }
                _ => {}
            }
        }

        // Process IPC commands (non-blocking)
        while let Ok(internal) = cmd_rx.try_recv() {
            use kroma_shared::ipc::{CompileError, DaemonCommand, DaemonEvent};
            let ipc_server::InternalCommand {
                command,
                response_tx,
            } = internal;

            /// Helper: send a compile result event through the response channel.
            fn send_response(
                tx: &Option<std::sync::mpsc::Sender<DaemonEvent>>,
                event: DaemonEvent,
            ) {
                if let Some(ref tx) = tx {
                    let _ = tx.send(event);
                }
            }

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
                        let _ = std::fs::remove_file(&sock);
                    }
                    return Ok(());
                }
                DaemonCommand::LoadShade { path } => {
                    info!("Loading shade package: {}", path);
                    match LiveShadePackage::load(std::path::Path::new(&path)) {
                        Ok(pkg) => {
                            if !(pkg.config.slideshow.interval > 0.0) {
                                video_decoders = try_create_video_decoders(&pkg);
                            } else {
                                video_decoders = vec![];
                            }
                            video_frame_accums = vec![0.0].repeat(video_decoders.len());
                            let pkg_name = pkg.config.meta.name.clone();
                            match render_state.load_shade(pkg) {
                                Ok(res) => {
                                    match res {
                                        LoadResult::Slideshow(event) => match event {
                                            SlideshowEvent::SwappedToVideo { source } => {
                                                video_decoders.push((
                                                    0,
                                                    video_decoder_for_source(
                                                        render_state
                                                            .active_package
                                                            .as_ref()
                                                            .unwrap(),
                                                        &source,
                                                    )
                                                    .unwrap(),
                                                ));
                                            }
                                            SlideshowEvent::SwappedToImage
                                            | SlideshowEvent::None => (),
                                        },
                                        LoadResult::Other => (),
                                    }
                                    info!("Reloaded: {}", pkg_name);
                                    send_response(
                                        &response_tx,
                                        DaemonEvent::CompileResult {
                                            success: true,
                                            errors: vec![],
                                            warnings: vec![],
                                        },
                                    );
                                }
                                Err(e) => {
                                    log::error!("Failed to compile shade shader: {}", e);
                                    send_response(
                                        &response_tx,
                                        DaemonEvent::CompileResult {
                                            success: false,
                                            errors: vec![CompileError {
                                                message: e.to_string(),
                                                line: None,
                                                column: None,
                                            }],
                                            warnings: vec![],
                                        },
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to load shade: {}", e);
                            send_response(
                                &response_tx,
                                DaemonEvent::CompileResult {
                                    success: false,
                                    errors: vec![CompileError {
                                        message: format!("Package load error: {}", e),
                                        line: None,
                                        column: None,
                                    }],
                                    warnings: vec![],
                                },
                            );
                        }
                    }
                }
                DaemonCommand::Reload => {
                    if let Some(ref path) = current_shade_path {
                        info!("Reloading shade: {}", path);
                        match LiveShadePackage::load(std::path::Path::new(path)) {
                            Ok(pkg) => {
                                if !(pkg.config.slideshow.interval > 0.0) {
                                    video_decoders = try_create_video_decoders(&pkg);
                                } else {
                                    video_decoders = vec![];
                                }
                                video_frame_accums = vec![0.0].repeat(video_decoders.len());
                                let pkg_name = pkg.config.meta.name.clone();
                                match render_state.load_shade(pkg) {
                                    Ok(res) => {
                                        match res {
                                            LoadResult::Slideshow(event) => match event {
                                                SlideshowEvent::SwappedToVideo { source } => {
                                                    video_decoders.push((
                                                        0,
                                                        video_decoder_for_source(
                                                            render_state
                                                                .active_package
                                                                .as_ref()
                                                                .unwrap(),
                                                            &source,
                                                        )
                                                        .unwrap(),
                                                    ));
                                                }
                                                SlideshowEvent::SwappedToImage
                                                | SlideshowEvent::None => (),
                                            },
                                            LoadResult::Other => (),
                                        }
                                        info!("Reloaded: {}", pkg_name);
                                        send_response(
                                            &response_tx,
                                            DaemonEvent::CompileResult {
                                                success: true,
                                                errors: vec![],
                                                warnings: vec![],
                                            },
                                        );
                                    }
                                    Err(e) => {
                                        log::error!("Failed to compile reloaded shader: {}", e);
                                        send_response(
                                            &response_tx,
                                            DaemonEvent::CompileResult {
                                                success: false,
                                                errors: vec![CompileError {
                                                    message: e.to_string(),
                                                    line: None,
                                                    column: None,
                                                }],
                                                warnings: vec![],
                                            },
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to reload shade: {}", e);
                                send_response(
                                    &response_tx,
                                    DaemonEvent::CompileResult {
                                        success: false,
                                        errors: vec![CompileError {
                                            message: format!("Package load error: {}", e),
                                            line: None,
                                            column: None,
                                        }],
                                        warnings: vec![],
                                    },
                                );
                            }
                        }
                    } else {
                        log::warn!("No shade loaded to reload");
                        send_response(
                            &response_tx,
                            DaemonEvent::Error {
                                message: "No shade loaded to reload".into(),
                            },
                        );
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
                            use base64::Engine;
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);
                            send_response(
                                &response_tx,
                                DaemonEvent::PreviewFrame {
                                    jpeg_base64: b64,
                                    width,
                                    height,
                                },
                            );
                        }
                        Err(e) => {
                            log::warn!("Preview capture failed: {}", e);
                            send_response(
                                &response_tx,
                                DaemonEvent::Error {
                                    message: format!("Preview capture failed: {}", e),
                                },
                            );
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
                            send_response(
                                &response_tx,
                                DaemonEvent::CompileResult {
                                    success: true,
                                    errors: vec![],
                                    warnings,
                                },
                            );
                        }
                        Err(e) => {
                            log::error!("Live reload failed: {}", e);
                            // Try to extract line numbers from error message
                            let compile_error = parse_compile_error(&e.to_string());
                            send_response(
                                &response_tx,
                                DaemonEvent::CompileResult {
                                    success: false,
                                    errors: vec![compile_error],
                                    warnings,
                                },
                            );
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
        if let Some(ref mut sp) = wayland_surface {
            if let Err(e) = sp.dispatch() {
                log::warn!("Wayland dispatch error: {}", e);
            }
        }

        // Update timing uniforms
        let now = std::time::Instant::now();
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
                    video_decoders = vec![];
                    video_decoders.push((
                        0,
                        video_decoder_for_source(
                            render_state.active_package.as_ref().unwrap(),
                            &source,
                        )
                        .unwrap(),
                    ));
                    video_frame_accums = vec![0.0].repeat(video_decoders.len());
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
                            use base64::Engine;
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&jpeg_bytes);
                            let event = kroma_shared::ipc::DaemonEvent::PreviewFrame {
                                jpeg_base64: b64,
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

/// Parse a shader compile error message and extract line/column numbers.
///
/// Handles patterns like:
/// - `error: 5:23: 'foo' : ...` (shaderc format: `source:line`)
/// - `error at line 42` (generic)
fn parse_compile_error(msg: &str) -> kroma_shared::ipc::CompileError {
    // shaderc pattern: "N:LINE:" where N is the source id
    // Look for two consecutive numbers separated by colon followed by colon
    if let Some(line_num) = extract_shaderc_line(msg) {
        return kroma_shared::ipc::CompileError {
            message: msg.to_string(),
            line: Some(line_num),
            column: None,
        };
    }

    // Generic "line N" pattern (case-insensitive manual search)
    let lower = msg.to_lowercase();
    if let Some(idx) = lower.find("line ") {
        let after = &msg[idx + 5..];
        let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = num_str.parse::<u32>() {
            return kroma_shared::ipc::CompileError {
                message: msg.to_string(),
                line: Some(n),
                column: None,
            };
        }
    }

    kroma_shared::ipc::CompileError {
        message: msg.to_string(),
        line: None,
        column: None,
    }
}

/// Extract line number from shaderc-style error messages (e.g., "0:42: error").
fn extract_shaderc_line(msg: &str) -> Option<u32> {
    // Find patterns like "N:LINE:" where both are digits
    let bytes = msg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for a digit followed by ':'
        if bytes[i].is_ascii_digit() {
            // Skip the source ID digits
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b':' {
                i += 1;
                // Now try to parse the line number
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > start && i < bytes.len() && bytes[i] == b':' {
                    if let Ok(line) = msg[start..i].parse::<u32>() {
                        return Some(line);
                    }
                }
            }
        }
        i += 1;
    }
    None
}
