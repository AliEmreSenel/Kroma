//! Kroma Daemon — the headless wallpaper rendering engine.
//!
//! This binary holds the wgpu context, manages the render loop,
//! data aggregation threads, and IPC communication with the GUI.

mod backend;
mod config;
mod data;
mod fallback;
mod ipc_server;
mod renderer;
mod textures;

use std::{
    env, fs,
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use log::info;

use kroma_shared::{
    ipc::{CompileError, DaemonCommand, DaemonEvent, maybe_send},
    shade::LiveShadePackage,
    traits::DataProvider,
};

use crate::{
    backend::{
        Backend,
        wayland::{WaylandBackend, hyprland::HyprlandEvent},
    },
    data::SystemDataProvider,
    renderer::{Renderer, ShadeLoadOutcome},
};

enum LoopControl {
    Continue,
    Shutdown,
}

struct Daemon {
    config: config::DaemonConfig,
    backend: Backend,
    data_provider: SystemDataProvider,
    renderer: Renderer,
    cmd_rx: mpsc::Receiver<ipc_server::InternalCommand>,
    ipc_status: Arc<Mutex<ipc_server::DaemonStatus>>,
    preview_stream: Arc<Mutex<ipc_server::PreviewStreamState>>,
    _ipc_handle: JoinHandle<()>,
    current_shade_path: Option<String>,
    start_time: Instant,
    frame: u32,
    paused: bool,
    active_workspace_id: i64,
    last_frame_time: Instant,
    frame_budget: Duration,
    fps_counter: u32,
    fps_timer: Instant,
}

impl Daemon {
    fn new() -> Result<Self> {
        let config = config::DaemonConfig::load()?;
        info!(
            "Target FPS: {}, GPU power: {:?}",
            config.target_fps, config.gpu_power
        );

        let session_type = env::var("XDG_SESSION_TYPE").unwrap_or_default();
        let desktop_env = env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();

        let backend = Backend::new(&session_type, &desktop_env)?;
        let data_provider = SystemDataProvider::new()?;
        let mut renderer = Renderer::new()?;

        let (surf_w, surf_h) = {
            let surface = backend.surface().context("A surface must exist")?;
            info!("Initializing GPU with surface...");

            if let Err(e) = renderer.init_gpu_with_surface(&config.gpu_power, surface) {
                log::error!("GPU init with surface failed: {}", e);
            } else {
                info!("GPU initialized with surface");
            }

            let primary_monitor = surface
                .list_monitors()?
                .first()
                .cloned()
                .context("At least one monitor should exist")?;

            surface.size(primary_monitor.id)?
        };

        renderer.uniforms.u_resolution = [surf_w as f32, surf_h as f32];

        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ipc_handle, ipc_status, preview_stream) =
            ipc_server::start(cmd_tx, config.preview.clone())?;
        info!("IPC server listening");

        let mut daemon = Self {
            frame_budget: config.frame_budget(),
            config,
            backend,
            data_provider,
            renderer,
            cmd_rx,
            ipc_status,
            preview_stream,
            _ipc_handle: ipc_handle,
            current_shade_path: None,
            start_time: Instant::now(),
            frame: 0,
            paused: false,
            active_workspace_id: 1,
            last_frame_time: Instant::now(),
            fps_counter: 0,
            fps_timer: Instant::now(),
        };

        if let Some(shade_path) = daemon.config.current_shade.clone() {
            info!("Loading initial shade: {}", shade_path);
            daemon.load_shade(&shade_path, None)?;
        } else {
            info!("No startup shade configured. Load a .shade file to start.");
            daemon.renderer.show_no_shade_fallback()?;
        }

        info!(
            "Daemon initialized ({}x{} @ {} FPS target)",
            surf_w, surf_h, daemon.config.target_fps
        );

        Ok(daemon)
    }

    fn run(mut self) -> Result<()> {
        info!("Entering render loop");

        loop {
            self.process_backend_events();

            if matches!(self.process_ipc_commands()?, LoopControl::Shutdown) {
                self.shutdown()?;
                return Ok(());
            }

            if self.paused {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }

            self.render_tick()?;
        }
    }

    fn process_backend_events(&mut self) {
        if let Backend::Wayland {
            backend: WaylandBackend::Hyprland { rx: hypr_rx, .. },
            ..
        } = &mut self.backend
        {
            while let Ok(event) = hypr_rx.try_recv() {
                match event {
                    HyprlandEvent::Fullscreen { fullscreen } if self.config.pause_on_fullscreen => {
                        self.paused = fullscreen;
                    }
                    HyprlandEvent::WorkspaceChanged { id } => {
                        info!(
                            "Workspace changed to {} (was {})",
                            id, self.active_workspace_id
                        );
                        self.active_workspace_id = id;
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
        }
    }

    fn process_ipc_commands(&mut self) -> Result<LoopControl> {
        while let Ok(internal) = self.cmd_rx.try_recv() {
            let ipc_server::InternalCommand {
                command,
                response_tx,
            } = internal;

            match command {
                DaemonCommand::Pause => {
                    self.paused = true;
                    info!("Rendering paused");
                }
                DaemonCommand::Resume => {
                    self.paused = false;
                    info!("Rendering resumed");
                }
                DaemonCommand::Shutdown => {
                    info!("Shutdown requested");
                    return Ok(LoopControl::Shutdown);
                }
                DaemonCommand::LoadShade { path } => {
                    info!("Loading shade package: {}", path);
                    self.load_shade(&path, response_tx)?;
                }
                DaemonCommand::Reload => {
                    if let Some(path) = self.current_shade_path.clone() {
                        info!("Reloading shade: {}", path);
                        self.load_shade(&path, response_tx)?;
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
                    self.renderer.set_custom_uniform(&name, &value);
                }
                DaemonCommand::StatusQuery => {
                    // Handled inline in ipc_server, shouldn't reach here
                }
                DaemonCommand::QuerySystemInfo => {
                    // Handled inline in ipc_server, shouldn't reach here
                }
                DaemonCommand::RequestPreviewFrame { width, height } => {
                    match self.renderer.capture_preview_frame(width, height) {
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
                    let result = kroma_shared::translator::translate(
                        &glsl_source,
                        "live-preview",
                        "Kroma Editor",
                    );
                    let warnings: Vec<String> = result.warnings.clone();
                    for w in &warnings {
                        log::warn!("Translation warning: {}", w);
                    }

                    match self.renderer.load_glsl_source(&result.shader_source) {
                        Ok(()) => {
                            self.current_shade_path = Some("live-preview".to_string());
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

        Ok(LoopControl::Continue)
    }

    fn load_shade(&mut self, path: &str, tx: Option<mpsc::Sender<DaemonEvent>>) -> Result<()> {
        let mut loaded_successfully = false;

        match LiveShadePackage::load(Path::new(path)) {
            Ok(pkg) => {
                let pkg_name = pkg.config.meta.name.clone();
                match self.renderer.load_shade(pkg, Some(path)) {
                    Ok(ShadeLoadOutcome::Success) => {
                        info!("Loaded: {}", pkg_name);
                        loaded_successfully = true;
                        maybe_send(
                            tx,
                            DaemonEvent::CompileResult {
                                success: true,
                                errors: vec![],
                                warnings: vec![],
                            },
                        )?;
                    }
                    Ok(ShadeLoadOutcome::TextureError(failures)) => {
                        let msgs: Vec<CompileError> = failures
                            .iter()
                            .map(|f| CompileError {
                                message: format!(
                                    "Required texture '{}' ({}): {}",
                                    f.name, f.source, f.error
                                ),
                                line: None,
                                column: None,
                            })
                            .collect();
                        log::error!(
                            "Shade '{}' loaded with {} texture error(s) — fallback displayed",
                            pkg_name,
                            msgs.len()
                        );
                        // Consider it "loaded" so the package stays active;
                        // the daemon renders the fallback error image.
                        loaded_successfully = true;
                        maybe_send(
                            tx,
                            DaemonEvent::CompileResult {
                                success: false,
                                errors: msgs,
                                warnings: vec![],
                            },
                        )?;
                    }
                    Ok(ShadeLoadOutcome::CompileError(msg)) => {
                        log::error!(
                            "Shade '{}' shader compile error — fallback displayed",
                            pkg_name
                        );
                        loaded_successfully = true;
                        maybe_send(
                            tx,
                            DaemonEvent::CompileResult {
                                success: false,
                                errors: vec![CompileError {
                                    message: msg,
                                    line: None,
                                    column: None,
                                }],
                                warnings: vec![],
                            },
                        )?;
                    }
                    Err(e) => {
                        log::error!("Failed to load shade '{}': {:#}", pkg_name, e);
                        self.renderer
                            .switch_to_load_error(path, &format!("{:#}", e))?;
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
                log::error!("Failed to load shade: {:#}", e);
                self.renderer
                    .switch_to_load_error(path, &format!("{:#}", e))?;
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

        if loaded_successfully {
            // Force initial texture update (first frame decode for videos, etc.)
            self.renderer.update_textures(0.0)?;
            self.current_shade_path = Some(path.to_string());

            if self.config.runtime.persist_current_shade {
                self.config.current_shade = Some(path.to_string());
                if let Err(e) = self.config.save() {
                    log::warn!("Failed to persist current shade to config: {}", e);
                }
            }
        }

        Ok(())
    }

    fn render_tick(&mut self) -> Result<()> {
        let frame_start = Instant::now();

        self.backend
            .surface_mut()
            .context("There must be a surface")?
            .dispatch()?;

        let dt = frame_start
            .duration_since(self.last_frame_time)
            .as_secs_f32();
        self.last_frame_time = frame_start;

        self.renderer.uniforms.u_time = self.start_time.elapsed().as_secs_f32();
        self.renderer.uniforms.u_delta_time = dt;
        self.renderer.uniforms.u_frame = self.frame;

        let stats = self.data_provider.get_system_stats();
        self.renderer.uniforms.apply_system_stats(&stats);
        let cursor = self
            .backend
            .cursor_pos()
            .unwrap_or_else(|| self.data_provider.get_cursor_pos());
        self.renderer.uniforms.apply_cursor(cursor);

        // Advance all texture sources (video decoding, slideshow timers, audio, etc.)
        self.renderer.update_textures(dt as f64)?;

        // Derive audio level from audio texture sources
        self.renderer.uniforms.u_audio_level = self.renderer.get_audio_level();

        self.renderer.render_frame()?;
        self.frame = self.frame.wrapping_add(1);

        self.maybe_stream_preview_frame();
        self.update_fps_and_status();

        let elapsed = frame_start.elapsed();
        if elapsed < self.frame_budget {
            std::thread::sleep(self.frame_budget - elapsed);
        }

        Ok(())
    }

    fn maybe_stream_preview_frame(&mut self) {
        if let Ok(mut ps) = self.preview_stream.try_lock()
            && ps.active
        {
            let interval = Duration::from_secs_f64(1.0 / ps.target_fps.max(1) as f64);
            if ps.last_frame_time.elapsed() >= interval {
                let pw = ps.width;
                let ph = ps.height;
                match self.renderer.capture_preview_frame(pw, ph) {
                    Ok(jpeg_bytes) => {
                        let event = DaemonEvent::PreviewFrame {
                            jpeg: jpeg_bytes,
                            width: pw,
                            height: ph,
                        };
                        if let Ok(json) = serde_json::to_string(&event)
                            && let Some(ref mut w) = ps.writer
                        {
                            use std::io::Write;
                            if writeln!(w, "{}", json).is_err() || w.flush().is_err() {
                                ps.active = false;
                                ps.writer = None;
                                log::info!("Preview stream client disconnected");
                            }
                        }
                        ps.last_frame_time = Instant::now();
                    }
                    Err(e) => {
                        log::warn!("Preview capture error: {}", e);
                    }
                }
            }
        }
    }

    fn update_fps_and_status(&mut self) {
        self.fps_counter += 1;
        let elapsed = self.fps_timer.elapsed().as_secs_f32();

        if elapsed >= 1.0 {
            let current_fps = self.fps_counter as f32 / elapsed;
            let effective_target_fps = self.config.target_fps.max(1);
            let log_interval =
                effective_target_fps * self.config.logging.fps_log_interval_secs.max(1);
            if self.frame % log_interval < effective_target_fps {
                info!(
                    "FPS: {:.1} | time: {:.1}s | shader: {}",
                    current_fps,
                    self.start_time.elapsed().as_secs_f32(),
                    self.current_shade_path
                        .as_deref()
                        .unwrap_or("fallback (load a .shade file)")
                );
            }
            self.fps_counter = 0;
            self.fps_timer = Instant::now();

            if let Ok(mut status) = self.ipc_status.lock() {
                status.fps = current_fps;
                status.paused = self.paused;
                status.loaded_shade = self.current_shade_path.clone();
                status.cpu_usage = self.renderer.uniforms.u_cpu * 100.0;
                status.ram_usage = self.renderer.uniforms.u_ram * 100.0;
                status.battery = if self.renderer.uniforms.u_battery >= 0.0 {
                    Some(self.renderer.uniforms.u_battery * 100.0)
                } else {
                    None
                };
                status.audio_level = self.renderer.uniforms.u_audio_level;
                status.cursor_x = self.renderer.uniforms.u_mouse[0];
                status.cursor_y = self.renderer.uniforms.u_mouse[1];
            }
        }
    }

    fn shutdown(&mut self) -> Result<()> {
        self.cleanup_socket()?;
        info!("Daemon shutdown complete");
        Ok(())
    }

    fn cleanup_socket(&self) -> Result<()> {
        let sock = kroma_shared::ipc::socket_path();
        if sock.exists() {
            fs::remove_file(&sock)
                .with_context(|| format!("Failed to remove IPC socket: {}", sock.display()))?;
        }
        Ok(())
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Err(e) = self.cleanup_socket() {
            log::debug!("IPC socket cleanup skipped: {}", e);
        }
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    info!("Kroma Daemon v{}", env!("CARGO_PKG_VERSION"));
    info!("Initializing...");

    let daemon = Daemon::new()?;
    daemon.run()
}
