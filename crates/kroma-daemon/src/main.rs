//! Kroma Daemon — the headless wallpaper rendering engine.
//!
//! This binary holds the wgpu context, manages the render loop,
//! data aggregation threads, and IPC communication with the GUI.

mod audio;
mod config;
mod data;
mod hyprland;
mod ipc_server;
mod renderer;
mod surface;
mod video;

use anyhow::Result;
use log::info;

use kroma_shared::traits::{DataProvider, SurfaceProvider};

use crate::audio::{AudioProvider, SimulatedAudioProvider, SilentAudioProvider, CpalAudioProvider};

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    info!("Kroma Daemon v{}", env!("CARGO_PKG_VERSION"));
    info!("Initializing...");

    // ---------------------------------------------------------------
    // 0. Load daemon configuration
    // ---------------------------------------------------------------
    let daemon_config = config::DaemonConfig::load()?;
    info!("Target FPS: {}, GPU power: {}", daemon_config.target_fps, daemon_config.gpu_power);

    // ---------------------------------------------------------------
    // 1. Initialize the Wayland surface provider
    // ---------------------------------------------------------------
    let mut surface_provider = surface::WaylandSurfaceProvider::new();
    surface_provider.connect()?;

    // Create layer shell surfaces on all monitors
    surface_provider.create_all_surfaces()?;

    let monitors = surface_provider.list_monitors()?;
    info!("Active on {} monitor(s):", monitors.len());
    for m in &monitors {
        info!("  - {} ({}x{} @ {},{}, scale {})",
            m.name, m.width, m.height, m.x, m.y, m.scale);
    }

    // ---------------------------------------------------------------
    // 2. Start the data provider
    // ---------------------------------------------------------------
    let data_provider = data::SystemDataProvider::new();
    info!("Data provider initialized");

    // ---------------------------------------------------------------
    // 2b. Start audio provider
    // ---------------------------------------------------------------
    let audio_provider: Box<dyn AudioProvider> = if std::env::var("KROMA_SIMULATE_AUDIO").is_ok() {
        info!("Using simulated audio provider");
        Box::new(SimulatedAudioProvider::new())
    } else {
        match CpalAudioProvider::new() {
            Ok(provider) => {
                info!("Using cpal audio provider (real audio capture)");
                Box::new(provider)
            }
            Err(e) => {
                info!("cpal audio unavailable ({}), using silent provider", e);
                Box::new(SilentAudioProvider)
            }
        }
    };

    // ---------------------------------------------------------------
    // 3. Start the IPC server (async, background)
    // ---------------------------------------------------------------
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let (_ipc_handle, ipc_status) = ipc_server::start(cmd_tx)?;
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
            log::warn!("Hyprland events unavailable: {} — running without compositor awareness", e);
            None
        }
    };

    // ---------------------------------------------------------------
    // 4. Initialize the renderer with real Wayland surfaces
    // ---------------------------------------------------------------
    let mut render_state = renderer::RenderState::new()?;

    // Get the primary monitor and its surface pointers
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
    let (surf_w, surf_h) = surface_provider
        .surface_size(primary.id.0)
        .unwrap_or((primary.width, primary.height));

    if let (Some(display_ptr), Some(surface_ptr)) = (display_ptr, surface_ptr) {
        info!("Initializing GPU with Wayland surface ({}x{})...", surf_w, surf_h);
        match unsafe { render_state.init_gpu_with_surface(display_ptr, surface_ptr, surf_w, surf_h) } {
            Ok(()) => info!("GPU initialized with real surface"),
            Err(e) => {
                log::error!("GPU init with surface failed: {} — trying headless", e);
                match render_state.init_gpu_headless() {
                    Ok(()) => info!("GPU initialized in headless mode (no visible output)"),
                    Err(e2) => log::error!("GPU init headless failed too: {} — rendering disabled", e2),
                }
            }
        }
    } else {
        log::warn!("No Wayland surface pointers available — trying headless GPU");
        match render_state.init_gpu_headless() {
            Ok(()) => info!("GPU initialized in headless mode"),
            Err(e) => log::warn!("GPU init failed: {} — rendering disabled", e),
        }
    }

    // Set resolution from primary monitor
    render_state.uniforms.u_resolution = [surf_w as f32, surf_h as f32];

    // Load initial shade if configured
    let mut current_shade_path: Option<String> = None;
    if let Some(ref shade_path) = daemon_config.current_shade {
        info!("Loading initial shade: {}", shade_path);
        match kroma_shared::shade::ShadePackage::load(std::path::Path::new(shade_path)) {
            Ok(pkg) => {
                match render_state.load_shade(&pkg) {
                    Ok(()) => {
                        current_shade_path = Some(shade_path.clone());
                        info!("Initial shade loaded: {}", pkg.config.meta.name);
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
    info!("Entering render loop ({}x{} @ {} FPS target)", surf_w, surf_h, daemon_config.target_fps);
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
                hyprland::HyprlandEvent::Fullscreen { fullscreen } if daemon_config.pause_on_fullscreen => {
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
                    log::debug!("Active workspace is now {}", active_workspace_id);
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
        while let Ok(cmd) = cmd_rx.try_recv() {
            use kroma_shared::ipc::DaemonCommand;
            match cmd {
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
                    match kroma_shared::shade::ShadePackage::load(std::path::Path::new(&path)) {
                        Ok(pkg) => {
                            match render_state.load_shade(&pkg) {
                                Ok(()) => {
                                    current_shade_path = Some(path.clone());
                                    info!("Loaded: {}", pkg.config.meta.name);
                                }
                                Err(e) => log::error!("Failed to compile shade shader: {}", e),
                            }
                        }
                        Err(e) => log::error!("Failed to load shade: {}", e),
                    }
                }
                DaemonCommand::Reload => {
                    if let Some(ref path) = current_shade_path {
                        info!("Reloading shade: {}", path);
                        match kroma_shared::shade::ShadePackage::load(std::path::Path::new(path)) {
                            Ok(pkg) => {
                                match render_state.load_shade(&pkg) {
                                    Ok(()) => info!("Reloaded: {}", pkg.config.meta.name),
                                    Err(e) => log::error!("Failed to compile reloaded shader: {}", e),
                                }
                            }
                            Err(e) => log::error!("Failed to reload shade: {}", e),
                        }
                    } else {
                        log::warn!("No shade loaded to reload");
                    }
                }
                DaemonCommand::SetUniform { name, value } => {
                    log::debug!("Setting uniform {} = {:?}", name, value);
                    render_state.set_custom_uniform(&name, &value);
                }
            }
        }

        if paused {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }

        // Dispatch Wayland events
        if let Err(e) = surface_provider.dispatch() {
            log::warn!("Wayland dispatch error: {}", e);
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
        // Normalise cursor to 0..1 using actual surface resolution
        let norm_cursor = glam::Vec2::new(
            cursor.x / render_state.uniforms.u_resolution[0].max(1.0),
            cursor.y / render_state.uniforms.u_resolution[1].max(1.0),
        );
        render_state.uniforms.apply_cursor(norm_cursor);

        // Gather audio data
        let _audio_spectrum = audio_provider.get_spectrum();
        let audio_level = audio_provider.get_level();
        render_state.uniforms.u_audio_level = audio_level;

        // Render frame
        render_state.render_frame()?;
        frame = frame.wrapping_add(1);

        // FPS tracking
        fps_counter += 1;
        if fps_timer.elapsed().as_secs_f32() >= 1.0 {
            current_fps = fps_counter as f32 / fps_timer.elapsed().as_secs_f32();
            if frame % (daemon_config.target_fps * 5) < daemon_config.target_fps {
                info!("FPS: {:.1} | time: {:.1}s | shader: {}",
                    current_fps,
                    start_time.elapsed().as_secs_f32(),
                    current_shade_path.as_deref().unwrap_or("default"));
            }
            fps_counter = 0;
            fps_timer = std::time::Instant::now();

            // Update shared IPC status
            if let Ok(mut status) = ipc_status.lock() {
                status.fps = current_fps;
                status.paused = paused;
                status.loaded_shade = current_shade_path.clone();
            }
        }

        // Frame rate limiting
        let elapsed = now.elapsed();
        if elapsed < frame_budget {
            std::thread::sleep(frame_budget - elapsed);
        }
    }
}
