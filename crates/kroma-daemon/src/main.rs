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
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use log::info;

use kroma_shared::{
    ipc::{CompileError, DaemonCommand, DaemonEvent, maybe_send},
    shade::LiveShadePackage,
    traits::{DataProvider, VideoDecoder},
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
fn try_create_video_decoders(pkg: &LiveShadePackage) -> Vec<(usize, DefaultVideoDecoder, f64)> {
    let mut decoders = Vec::new();

    // We need to know the index of each texture to update the correct slot in RenderState.
    // RenderState sorts textures by binding index. We must replicate that sort order here.
    let mut tex_defs: Vec<_> = pkg.config.textures.iter().collect();
    tex_defs.sort_by_key(|(_, def)| def.binding.unwrap_or(u32::MAX));

    // Iterate through sorted textures to match RenderState's internal `self.textures` vector
    for (i, (_name, def)) in tex_defs.iter().enumerate() {
        if def.ty == "video"
            && let Some(ref source) = def.source
        {
            // 1. Try loading from embedded assets (ZIP package)
            if let Some(d) = video_decoder_for_source(pkg, source) {
                decoders.push((i, d, 0.0))
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

fn update_video(
    render_state: &mut RenderState,
    video_decoders: &mut Vec<(usize, DefaultVideoDecoder, f64)>,
    dt: f32,
) {
    for (tex_index, decoder, accum) in video_decoders.iter_mut() {
        let frame_interval = {
            let raw = decoder.frame_interval();
            if raw.is_finite() && raw > 0.0 {
                raw
            } else {
                1.0 / 30.0
            }
        };

        if dt == 0.0 {
            let (vw, vh) = decoder.dimensions();
            match decoder.next_frame() {
                Some(rgba_data) => {
                    render_state.update_video_frame(rgba_data, vw, vh, *tex_index);
                }
                None => {
                    if let Err(e) = decoder.seek(0.0) {
                        log::warn!("Video {} seek failed: {}", tex_index, e);
                    }
                }
            }
            continue;
        }

        *accum += dt as f64;
        while *accum >= frame_interval {
            *accum -= frame_interval;
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
}

fn update(
    render_state: &mut RenderState,
    video_decoders: &mut Vec<(usize, DefaultVideoDecoder, f64)>,
    dt: f32,
) -> Result<()> {
    match render_state.update_slideshow(dt as f64) {
        Ok(res) => match res {
            SlideshowEvent::SwappedToVideo { source } => {
                if let Some(pkg) = render_state.active_package.as_ref() {
                    if let Some(decoder) = video_decoder_for_source(pkg, &source) {
                        *video_decoders = vec![(0, decoder, 0.0)];
                    } else {
                        log::warn!("Slideshow swap requested video '{}' but decoder init failed", source);
                        video_decoders.clear();
                    }
                } else {
                    log::warn!("Slideshow swap requested video '{}' but no active package", source);
                    video_decoders.clear();
                }
            }
            SlideshowEvent::SwappedToImage => {
                video_decoders.clear();
            }
            SlideshowEvent::None => {}
        },
        Err(e) => {
            log::warn!("Slideshow update failed: {}", e);
        }
    }

    update_video(render_state, video_decoders, dt);
    Ok(())
}

fn load_shade(
    path: &str,
    render_state: &mut RenderState,
    audio_provider: &mut CpalAudioProvider,
    tx: Option<mpsc::Sender<DaemonEvent>>,
) -> Result<Vec<(usize, DefaultVideoDecoder, f64)>> {
    let mut video_decoders = vec![];
    match LiveShadePackage::load(std::path::Path::new(path)) {
        Ok(pkg) => {
            if let Some(audio_conf) = pkg.config.audio.as_ref()
                && audio_conf.enabled
            {
                audio_provider.switch(audio_conf)?;
            } else {
                audio_provider.close();
            }

            if pkg.config.slideshow.is_none() {
                video_decoders = try_create_video_decoders(&pkg);
            }
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

    update(render_state, &mut video_decoders, 0.0)?;

    Ok(video_decoders)
}

type VideoDecoders = Vec<(usize, DefaultVideoDecoder, f64)>;

enum LoopControl {
    Continue,
    Shutdown,
}

struct Daemon {
    config: config::DaemonConfig,
    backend: Backend,
    data_provider: SystemDataProvider,
    audio_provider: CpalAudioProvider,
    render_state: RenderState,
    cmd_rx: mpsc::Receiver<ipc_server::InternalCommand>,
    ipc_status: Arc<Mutex<ipc_server::DaemonStatus>>,
    preview_stream: Arc<Mutex<ipc_server::PreviewStreamState>>,
    _ipc_handle: JoinHandle<()>,
    current_shade_path: Option<String>,
    video_decoders: VideoDecoders,
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
        let audio_provider = CpalAudioProvider::new();
        let mut render_state = RenderState::new()?;

        let (surf_w, surf_h) = {
            let surface = backend.surface().context("A surface must exist")?;
            info!("Initializing GPU with surface...");

            if let Err(e) = render_state.init_gpu_with_surface(surface) {
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

        render_state.uniforms.u_resolution = [surf_w as f32, surf_h as f32];

        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ipc_handle, ipc_status, preview_stream) = ipc_server::start(cmd_tx)?;
        info!("IPC server listening");

        let mut daemon = Self {
            frame_budget: config.frame_budget(),
            config,
            backend,
            data_provider,
            audio_provider,
            render_state,
            cmd_rx,
            ipc_status,
            preview_stream,
            _ipc_handle: ipc_handle,
            current_shade_path: None,
            video_decoders: vec![],
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
                        if fullscreen {
                            self.paused = true;
                            info!("Fullscreen detected — pausing render");
                        } else {
                            self.paused = false;
                            info!("Fullscreen exited — resuming render");
                        }
                    }
                    HyprlandEvent::WorkspaceChanged { id } => {
                        info!("Workspace changed to {} (was {})", id, self.active_workspace_id);
                        self.active_workspace_id = id;

                        if self.config.pause_on_inactive {
                            if id < 0 {
                                if !self.paused {
                                    self.paused = true;
                                    info!("Special workspace active — pausing render");
                                }
                            } else if self.paused {
                                self.paused = false;
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
        }
    }

    fn process_ipc_commands(&mut self) -> Result<LoopControl> {
        while let Ok(internal) = self.cmd_rx.try_recv() {
            let ipc_server::InternalCommand {
                command,
                response_tx,
            } = internal;

            if matches!(self.handle_command(command, response_tx)?, LoopControl::Shutdown) {
                return Ok(LoopControl::Shutdown);
            }
        }

        Ok(LoopControl::Continue)
    }

    fn handle_command(
        &mut self,
        command: DaemonCommand,
        response_tx: Option<mpsc::Sender<DaemonEvent>>,
    ) -> Result<LoopControl> {
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
                self.render_state.set_custom_uniform(&name, &value);
            }
            DaemonCommand::StatusQuery => {
                // Handled inline in ipc_server, shouldn't reach here
            }
            DaemonCommand::QuerySystemInfo => {
                // Handled inline in ipc_server, shouldn't reach here
            }
            DaemonCommand::RequestPreviewFrame { width, height } => {
                match self.render_state.capture_preview_frame(width, height) {
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
                let result =
                    kroma_shared::translator::translate(&glsl_source, "live-preview", "Kroma Editor");
                let warnings: Vec<String> = result.warnings.clone();
                for w in &warnings {
                    log::warn!("Translation warning: {}", w);
                }

                match self.render_state.load_glsl_source(&result.shader_source) {
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

        Ok(LoopControl::Continue)
    }

    fn load_shade(&mut self, path: &str, tx: Option<mpsc::Sender<DaemonEvent>>) -> Result<()> {
        self.video_decoders = load_shade(
            path,
            &mut self.render_state,
            &mut self.audio_provider,
            tx,
        )?;
        self.current_shade_path = Some(path.to_string());
        Ok(())
    }

    fn render_tick(&mut self) -> Result<()> {
        let frame_start = Instant::now();

        self.backend
            .surface_mut()
            .context("There must be a surface")?
            .dispatch()?;

        let dt = frame_start.duration_since(self.last_frame_time).as_secs_f32();
        self.last_frame_time = frame_start;

        self.render_state.uniforms.u_time = self.start_time.elapsed().as_secs_f32();
        self.render_state.uniforms.u_delta_time = dt;
        self.render_state.uniforms.u_frame = self.frame;

        let stats = self.data_provider.get_system_stats();
        self.render_state.uniforms.apply_system_stats(&stats);
        let cursor = self
            .backend
            .cursor_pos()
            .unwrap_or_else(|| self.data_provider.get_cursor_pos());
        self.render_state.uniforms.apply_cursor(cursor);

        let audio_spectrum = self.audio_provider.get_spectrum();
        let audio_level = self.audio_provider.get_level();
        self.render_state.uniforms.u_audio_level = audio_level;
        self.render_state.update_audio_spectrum(&audio_spectrum);

        update(&mut self.render_state, &mut self.video_decoders, dt)?;

        self.render_state.render_frame()?;
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
                match self.render_state.capture_preview_frame(pw, ph) {
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
            let log_interval = self.config.target_fps.max(1) * 5;
            if self.frame % log_interval < self.config.target_fps.max(1) {
                info!(
                    "FPS: {:.1} | time: {:.1}s | shader: {}",
                    current_fps,
                    self.start_time.elapsed().as_secs_f32(),
                    self.current_shade_path.as_deref().unwrap_or("default")
                );
            }
            self.fps_counter = 0;
            self.fps_timer = Instant::now();

            if let Ok(mut status) = self.ipc_status.lock() {
                status.fps = current_fps;
                status.paused = self.paused;
                status.loaded_shade = self.current_shade_path.clone();
                status.cpu_usage = self.render_state.uniforms.u_cpu * 100.0;
                status.ram_usage = self.render_state.uniforms.u_ram * 100.0;
                status.battery = if self.render_state.uniforms.u_battery >= 0.0 {
                    Some(self.render_state.uniforms.u_battery * 100.0)
                } else {
                    None
                };
                status.audio_level = self.render_state.uniforms.u_audio_level;
                status.cursor_x = self.render_state.uniforms.u_mouse[0];
                status.cursor_y = self.render_state.uniforms.u_mouse[1];
            }
        }
    }

    fn shutdown(&mut self) -> Result<()> {
        self.audio_provider.close();
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
