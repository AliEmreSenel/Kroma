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
    ipc::{
        CompileError, DaemonCommand, DaemonEvent, DaemonPhase, DaemonWaitReason, LoadRejectCode,
        maybe_send,
    },
    shade::LiveShadePackage,
    traits::DataProvider,
    types::{
        ShadeConfig, ShadeMeta, ShadePhase, ShadeStateDef, ShadeStates, TextureDef, TextureType,
    },
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

#[derive(Default)]
struct PhaseWgslCache {
    load: Option<String>,
    active: Option<String>,
    unload: Option<String>,
}

struct RuntimeShade {
    path: String,
    package: Arc<LiveShadePackage>,
    cache: PhaseWgslCache,
}

impl RuntimeShade {
    fn state(&self, phase: ShadePhase) -> Option<&ShadeStateDef> {
        match phase {
            ShadePhase::Load => self.package.config.states.load.as_ref(),
            ShadePhase::Active => self.package.config.states.active.as_ref(),
            ShadePhase::Unload => self.package.config.states.unload.as_ref(),
        }
    }

    fn wgsl(&self, phase: ShadePhase) -> Option<&str> {
        match phase {
            ShadePhase::Load => self.cache.load.as_deref(),
            ShadePhase::Active => self.cache.active.as_deref(),
            ShadePhase::Unload => self.cache.unload.as_deref(),
        }
    }

    fn first_defined_phase(&self) -> Option<ShadePhase> {
        if self.state(ShadePhase::Load).is_some() {
            Some(ShadePhase::Load)
        } else if self.state(ShadePhase::Active).is_some() {
            Some(ShadePhase::Active)
        } else if self.state(ShadePhase::Unload).is_some() {
            Some(ShadePhase::Unload)
        } else {
            None
        }
    }
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
    runtime_shade: Option<RuntimeShade>,
    pending_shade: Option<RuntimeShade>,
    unload_requested: bool,
    current_shade_path: Option<String>,
    start_time: Instant,
    frame: u32,
    phase_elapsed: f64,
    phase_frame: u32,
    paused: bool,
    current_phase: DaemonPhase,
    pending_request_path: Option<String>,
    wait_reason: DaemonWaitReason,
    active_workspace_id: i64,
    last_frame_time: Instant,
    frame_budget: Duration,
    fps_counter: u32,
    fps_timer: Instant,
}

impl Daemon {
    fn detect_direct_media_type(path: &Path) -> Option<TextureType> {
        let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "avif" | "tif" | "tiff" => {
                Some(TextureType::Image)
            }
            "mp4" | "webm" | "mkv" | "avi" | "mov" | "m4v" | "mpg" | "mpeg" | "wmv" => {
                Some(TextureType::Video)
            }
            _ => None,
        }
    }

    fn package_from_media_path(path: &Path) -> Option<LiveShadePackage> {
        let ty = Self::detect_direct_media_type(path)?;

        let display_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Direct Media".to_string());

        let mut config = ShadeConfig {
            meta: ShadeMeta {
                name: display_name,
                author: "Kroma Auto".into(),
                version: "1.0".into(),
                description: "Auto-generated package for direct media playback".into(),
                tags: vec!["direct-media".into(), ty.as_str().into()],
            },
            rendering: Default::default(),
            states: ShadeStates {
                load: None,
                active: Some(ShadeStateDef {
                    length: 0.0,
                    shader: None,
                    uniforms: Default::default(),
                    textures: Default::default(),
                    buffers: Default::default(),
                }),
                unload: None,
            },
        };

        if let Some(active) = config.states.active.as_mut() {
            active.textures.insert(
                "iChannel0".into(),
                TextureDef {
                    ty,
                    source: Some(path.to_string_lossy().to_string()),
                    seed: None,
                    input: None,
                    sources: Vec::new(),
                    looping: true,
                    filter: Default::default(),
                    wrap: Default::default(),
                    binding: Some(0),
                    font_size: None,
                    interval: None,
                    shuffle: false,
                    fft_bands: None,
                    hot_reload: true,
                    optional: false,
                    shader: None,
                    width: None,
                    height: None,
                    textures: Default::default(),
                    uniforms: Default::default(),
                },
            );
        }

        Some(LiveShadePackage::new_empty(config))
    }

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
            runtime_shade: None,
            pending_shade: None,
            unload_requested: false,
            current_shade_path: None,
            start_time: Instant::now(),
            frame: 0,
            phase_elapsed: 0.0,
            phase_frame: 0,
            paused: false,
            current_phase: DaemonPhase::None,
            pending_request_path: None,
            wait_reason: DaemonWaitReason::Idle,
            active_workspace_id: 1,
            last_frame_time: Instant::now(),
            fps_counter: 0,
            fps_timer: Instant::now(),
        };

        if let Some(shade_path) = daemon.config.current_shade.clone() {
            info!("Loading initial shade: {}", shade_path);
            daemon.handle_load_command(&shade_path, true, None)?;
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
                self.last_frame_time = Instant::now();
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
                DaemonCommand::LoadShade { path, force } => {
                    self.handle_load_command(&path, force, response_tx)?;
                }
                DaemonCommand::UnloadShade => {
                    self.handle_unload_command()?;
                }
                DaemonCommand::Reload => {
                    if let Some(path) = self.current_shade_path.clone() {
                        info!("Reloading shade: {}", path);
                        self.handle_load_command(&path, true, response_tx)?;
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

    fn handle_load_command(
        &mut self,
        path: &str,
        force: bool,
        tx: Option<mpsc::Sender<DaemonEvent>>,
    ) -> Result<()> {
        info!("Load request: path='{}' force={}", path, force);

        if !force {
            if self.pending_shade.is_some() {
                return self.send_load_reject(
                    tx,
                    LoadRejectCode::BusyWaitingBoundary,
                    "load already queued",
                );
            }
            if self.current_phase == DaemonPhase::Load {
                return self.send_load_reject(
                    tx,
                    LoadRejectCode::BusyRunningLoad,
                    "load phase is running",
                );
            }
            if self.current_phase == DaemonPhase::Unload {
                return self.send_load_reject(
                    tx,
                    LoadRejectCode::BusyRunningUnload,
                    "unload phase is running",
                );
            }
        }

        let prepared = match self.prepare_runtime_shade(path) {
            Ok(runtime) => runtime,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("states.") || msg.contains("No phase is defined in states") {
                    return self.send_load_reject(tx, LoadRejectCode::InvalidStateDefinition, &msg);
                }
                self.renderer
                    .switch_to_load_error(path, &format!("{:#}", e))?;
                self.current_phase = DaemonPhase::Terminal;
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
                return Ok(());
            }
        };

        if force
            || self.runtime_shade.is_none()
            || matches!(
                self.current_phase,
                DaemonPhase::None | DaemonPhase::Terminal
            )
        {
            self.pending_shade = None;
            self.pending_request_path = None;
            self.unload_requested = false;
            return self.start_runtime_shade(prepared, tx);
        }

        self.pending_request_path = Some(path.to_string());
        self.pending_shade = Some(prepared);
        self.unload_requested = true;

        maybe_send(
            tx,
            DaemonEvent::CompileResult {
                success: true,
                errors: vec![],
                warnings: vec!["queued_load".to_string()],
            },
        )?;

        if self.current_phase == DaemonPhase::Active {
            let active_len = self.phase_length(ShadePhase::Active);
            if active_len <= 0.0 {
                self.begin_unload_or_finalize()?;
            } else {
                self.wait_reason = DaemonWaitReason::WaitingActiveBoundary;
            }
        }

        Ok(())
    }

    fn handle_unload_command(&mut self) -> Result<()> {
        if self.runtime_shade.is_none() {
            self.current_phase = DaemonPhase::None;
            self.wait_reason = DaemonWaitReason::Idle;
            return Ok(());
        }

        self.pending_shade = None;
        self.pending_request_path = None;
        self.unload_requested = true;

        if self.current_phase == DaemonPhase::Active && self.phase_length(ShadePhase::Active) > 0.0
        {
            self.wait_reason = DaemonWaitReason::WaitingActiveBoundary;
            return Ok(());
        }

        self.begin_unload_or_finalize()
    }

    fn send_load_reject(
        &self,
        tx: Option<mpsc::Sender<DaemonEvent>>,
        code: LoadRejectCode,
        message: &str,
    ) -> Result<()> {
        maybe_send(
            tx,
            DaemonEvent::LoadRejected {
                code,
                message: message.to_string(),
            },
        )
    }

    fn prepare_runtime_shade(&self, path: &str) -> Result<RuntimeShade> {
        let requested_path = Path::new(path);
        let package = if requested_path.extension().and_then(|s| s.to_str()) == Some("shade") {
            LiveShadePackage::load(requested_path)?
        } else if let Some(pkg) = Self::package_from_media_path(requested_path) {
            pkg
        } else {
            anyhow::bail!(
                "Unsupported input '{}'. Expected .shade package or direct image/video file",
                path
            )
        };

        if !package.config.states.has_any() {
            anyhow::bail!("Unsupported runtime config: missing [states.*] phase blocks");
        }

        for (name, state) in [
            ("load", package.config.states.load.as_ref()),
            ("active", package.config.states.active.as_ref()),
            ("unload", package.config.states.unload.as_ref()),
        ] {
            if let Some(state) = state
                && state.length < 0.0
            {
                anyhow::bail!("states.{}.length cannot be negative", name);
            }
        }

        let package = Arc::new(package);
        let mut cache = PhaseWgslCache::default();
        cache.load = self.precompile_phase_shader(&package, ShadePhase::Load)?;
        cache.active = self.precompile_phase_shader(&package, ShadePhase::Active)?;
        cache.unload = self.precompile_phase_shader(&package, ShadePhase::Unload)?;

        Ok(RuntimeShade {
            path: path.to_string(),
            package,
            cache,
        })
    }

    fn precompile_phase_shader(
        &self,
        pkg: &Arc<LiveShadePackage>,
        phase: ShadePhase,
    ) -> Result<Option<String>> {
        let state = match phase {
            ShadePhase::Load => pkg.config.states.load.as_ref(),
            ShadePhase::Active => pkg.config.states.active.as_ref(),
            ShadePhase::Unload => pkg.config.states.unload.as_ref(),
        };
        let Some(state) = state else {
            return Ok(None);
        };
        let Some(shader_path) = &state.shader else {
            return Ok(None);
        };

        let shader_bytes = pkg
            .read_asset(shader_path)
            .with_context(|| format!("Missing phase shader asset: {}", shader_path))?;
        let shader_source = String::from_utf8(shader_bytes)
            .with_context(|| format!("Phase shader '{}' is not valid UTF-8", shader_path))?;
        let wgsl = crate::renderer::glsl_to_wgsl(&shader_source)
            .with_context(|| format!("Failed to compile {:?} phase shader", phase))?;
        Ok(Some(wgsl))
    }

    fn start_runtime_shade(
        &mut self,
        runtime: RuntimeShade,
        tx: Option<mpsc::Sender<DaemonEvent>>,
    ) -> Result<()> {
        let path = runtime.path.clone();
        let first_phase = runtime
            .first_defined_phase()
            .with_context(|| "No phase is defined in states")?;

        self.runtime_shade = Some(runtime);
        self.pending_shade = None;
        self.pending_request_path = None;
        self.unload_requested = false;
        self.current_shade_path = Some(path.clone());
        self.wait_reason = DaemonWaitReason::RunningLoad;

        let outcome = self.enter_phase(first_phase)?;
        self.handle_phase_load_outcome(outcome, tx)?;

        self.renderer.update_textures(0.0)?;
        self.resolve_zero_length_chain()?;

        if self.config.runtime.persist_current_shade {
            self.config.current_shade = Some(path);
            if let Err(e) = self.config.save() {
                log::warn!("Failed to persist current shade to config: {}", e);
            }
        }

        Ok(())
    }

    fn handle_phase_load_outcome(
        &mut self,
        outcome: ShadeLoadOutcome,
        tx: Option<mpsc::Sender<DaemonEvent>>,
    ) -> Result<()> {
        match outcome {
            ShadeLoadOutcome::Success => {
                maybe_send(
                    tx,
                    DaemonEvent::CompileResult {
                        success: true,
                        errors: vec![],
                        warnings: vec![],
                    },
                )?;
            }
            ShadeLoadOutcome::TextureError(failures) => {
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
                maybe_send(
                    tx,
                    DaemonEvent::CompileResult {
                        success: false,
                        errors: msgs,
                        warnings: vec![],
                    },
                )?;
            }
            ShadeLoadOutcome::CompileError(msg) => {
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
        }
        Ok(())
    }

    fn enter_phase(&mut self, phase: ShadePhase) -> Result<ShadeLoadOutcome> {
        let (pkg, path, wgsl) = {
            let runtime = self
                .runtime_shade
                .as_ref()
                .with_context(|| "No runtime shade loaded")?;
            (
                Arc::clone(&runtime.package),
                runtime.path.clone(),
                runtime.wgsl(phase).map(|s| s.to_string()),
            )
        };

        let outcome = self
            .renderer
            .load_phase(pkg, phase, wgsl.as_deref(), Some(&path))?;

        self.phase_elapsed = 0.0;
        self.phase_frame = 0;
        self.current_phase = match phase {
            ShadePhase::Load => DaemonPhase::Load,
            ShadePhase::Active => DaemonPhase::Active,
            ShadePhase::Unload => DaemonPhase::Unload,
        };
        self.wait_reason = match phase {
            ShadePhase::Load => DaemonWaitReason::RunningLoad,
            ShadePhase::Unload => DaemonWaitReason::RunningUnload,
            ShadePhase::Active => DaemonWaitReason::Idle,
        };

        Ok(outcome)
    }

    fn phase_length(&self, phase: ShadePhase) -> f64 {
        self.runtime_shade
            .as_ref()
            .and_then(|runtime| runtime.state(phase))
            .map(|s| s.length)
            .unwrap_or(0.0)
    }

    fn resolve_zero_length_chain(&mut self) -> Result<()> {
        // Prevent accidental infinite loops from malformed transition logic.
        for _ in 0..6 {
            match self.current_phase {
                DaemonPhase::Load if self.phase_length(ShadePhase::Load) <= 0.0 => {
                    self.transition_after_load()?;
                }
                DaemonPhase::Unload if self.phase_length(ShadePhase::Unload) <= 0.0 => {
                    self.finalize_unload()?;
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn transition_after_load(&mut self) -> Result<()> {
        let Some(runtime) = self.runtime_shade.as_ref() else {
            self.current_phase = DaemonPhase::None;
            return Ok(());
        };

        if runtime.state(ShadePhase::Active).is_some() {
            let outcome = self.enter_phase(ShadePhase::Active)?;
            self.handle_phase_load_outcome(outcome, None)?;
        } else if runtime.state(ShadePhase::Unload).is_some() {
            let outcome = self.enter_phase(ShadePhase::Unload)?;
            self.handle_phase_load_outcome(outcome, None)?;
        } else {
            self.current_phase = DaemonPhase::Terminal;
            self.wait_reason = DaemonWaitReason::Idle;
        }
        Ok(())
    }

    fn begin_unload_or_finalize(&mut self) -> Result<()> {
        if self
            .runtime_shade
            .as_ref()
            .and_then(|runtime| runtime.state(ShadePhase::Unload))
            .is_some()
        {
            let outcome = self.enter_phase(ShadePhase::Unload)?;
            self.handle_phase_load_outcome(outcome, None)?;
        } else {
            self.current_phase = DaemonPhase::Terminal;
            self.wait_reason = DaemonWaitReason::Idle;
            self.unload_requested = false;
            if let Some(next) = self.pending_shade.take() {
                self.runtime_shade = None;
                self.start_runtime_shade(next, None)?;
            }
        }
        Ok(())
    }

    fn finalize_unload(&mut self) -> Result<()> {
        self.unload_requested = false;
        self.wait_reason = DaemonWaitReason::Idle;
        if let Some(next) = self.pending_shade.take() {
            self.runtime_shade = None;
            self.start_runtime_shade(next, None)?;
        } else {
            self.current_phase = DaemonPhase::Terminal;
        }
        Ok(())
    }

    fn advance_lifecycle(&mut self, dt: f64) -> Result<()> {
        self.phase_elapsed += dt;

        match self.current_phase {
            DaemonPhase::Load => {
                let length = self.phase_length(ShadePhase::Load);
                if self.phase_elapsed + (dt + 1e-6) >= length {
                    self.transition_after_load()?;
                    self.resolve_zero_length_chain()?;
                }
            }
            DaemonPhase::Active => {
                if self.unload_requested || self.pending_shade.is_some() {
                    let loop_len = self.phase_length(ShadePhase::Active);
                    if loop_len <= 0.0 {
                        self.begin_unload_or_finalize()?;
                    } else {
                        let prev = (self.phase_elapsed - dt).max(0.0);
                        let crossed = ((prev / loop_len).floor() as i64)
                            < (((self.phase_elapsed + (dt + 1e-6)) / loop_len).floor() as i64);
                        if crossed {
                            self.begin_unload_or_finalize()?;
                        } else {
                            self.wait_reason = DaemonWaitReason::WaitingActiveBoundary;
                        }
                    }
                } else {
                    self.wait_reason = DaemonWaitReason::Idle;
                }
            }
            DaemonPhase::Unload => {
                let length = self.phase_length(ShadePhase::Unload);
                if self.phase_elapsed + (dt + 1e-6) >= length {
                    self.finalize_unload()?;
                    self.resolve_zero_length_chain()?;
                }
            }
            DaemonPhase::Terminal | DaemonPhase::None => {
                if let Some(next) = self.pending_shade.take() {
                    self.start_runtime_shade(next, None)?;
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

        self.advance_lifecycle(dt as f64)?;

        self.renderer.uniforms.u_time = self.phase_elapsed as f32;
        self.renderer.uniforms.u_delta_time = dt;
        self.renderer.uniforms.u_frame = self.phase_frame;
        self.phase_frame = self.phase_frame.wrapping_add(1);

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
                status.current_phase = self.current_phase;
                status.pending_request_path = self.pending_request_path.clone();
                status.wait_reason = self.wait_reason;
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
