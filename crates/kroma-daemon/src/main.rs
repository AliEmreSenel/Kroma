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
mod transitions;

use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use log::info;
use naga::valid::{Capabilities, ValidationFlags, Validator};
use serde::{Deserialize, Serialize};

use kroma_shared::{
    ipc::{
        CompileError, DaemonCommand, DaemonEvent, DaemonPhase, DaemonWaitReason, LoadRejectCode,
        maybe_send,
    },
    shade::LiveShadePackage,
    traits::DataProvider,
    transition::{parse_transition_spec, resolve_transition_spec},
    types::{
        ShadeConfig, ShadeMeta, ShadePhase, ShadeStateDef, ShadeStates, TextureDef, TextureType,
        TransitionDef,
    },
};

use crate::{
    backend::{
        Backend,
        wayland::{WaylandEvent, backend_events},
    },
    data::SystemDataProvider,
    renderer::{PreparedTransitionLane, Renderer, ShadeLoadOutcome},
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

struct PendingTransitionPlan {
    spec: String,
    duration: f64,
    transition_wgsl: String,
}

#[derive(Clone, Copy)]
struct HandoffTimingPlan {
    transition_duration: f64,
    outgoing_unload_start_offset: f64,
    outgoing_unload_length: f64,
    incoming_load_length: f64,
}

struct QueuedHandoff {
    request_path: String,
    incoming: RuntimeShade,
    // Invariant: when present, this was fully validated before transition start.
    preflight: Option<TransitionPreflightSession>,
    preload_ready: bool,
}

// Precomputed transition startup payload. If this exists, transition start must
// not perform compatibility discovery or static fallback decisions.
struct TransitionPreflightSession {
    plan: PendingTransitionPlan,
    timing: HandoffTimingPlan,
    outgoing_phase: ShadePhase,
    incoming_phase: ShadePhase,
    startup: TransitionStartupPreflight,
}

enum TransitionStartupPreflight {
    UnifiedLane {
        prepared_lane: PreparedTransitionLane,
    },
}

enum TransitionCompletion {
    ResumePhase(DaemonPhase),
    ActivateIncoming(RuntimeShade),
}

#[derive(Clone, Copy)]
struct TransitionLaneState {
    phase: Option<ShadePhase>,
    elapsed: f64,
    frame: u32,
}

struct ActiveTransition {
    spec: String,
    elapsed: f64,
    duration: f64,
    completion: TransitionCompletion,
    handoff_timing: Option<HandoffTimingPlan>,
    // Dual live lane state tracked independently during transition.
    outgoing: TransitionLaneState,
    outgoing_unload_started: bool,
    incoming: TransitionLaneState,
}

#[derive(Debug, Serialize, Deserialize)]
struct BuiltinTransitionCache {
    version: u32,
    ids: Vec<String>,
}

const BUILTIN_TRANSITION_CACHE_VERSION: u32 = 1;
const TIMING_EPSILON: f64 = 1e-6;

fn load_reject_for_runtime_state(
    force: bool,
    current_phase: DaemonPhase,
    has_active_transition: bool,
    has_queued_handoff: bool,
) -> Option<(LoadRejectCode, &'static str)> {
    if current_phase == DaemonPhase::Transitioning || has_active_transition {
        return Some((LoadRejectCode::BusyTransitioning, "transition is running"));
    }

    if force {
        return None;
    }

    if has_queued_handoff {
        return Some((LoadRejectCode::BusyWaitingBoundary, "load already queued"));
    }

    match current_phase {
        DaemonPhase::Load => Some((LoadRejectCode::BusyRunningLoad, "load phase is running")),
        DaemonPhase::Unload => Some((LoadRejectCode::BusyRunningUnload, "unload phase is running")),
        _ => None,
    }
}

fn crossed_active_boundary(prev_elapsed: f64, next_elapsed: f64, loop_len: f64) -> bool {
    if loop_len <= 0.0 {
        return true;
    }

    ((prev_elapsed.max(0.0) / loop_len).floor() as i64)
        < (((next_elapsed + TIMING_EPSILON) / loop_len).floor() as i64)
}

fn transition_elapsed_complete(elapsed: f64, duration: f64) -> bool {
    elapsed + TIMING_EPSILON >= duration
}

fn should_wait_for_incoming_preload(preload_ready: Option<bool>) -> bool {
    matches!(preload_ready, Some(false))
}

fn solve_handoff_timing_for_runtime(
    outgoing: &RuntimeShade,
    incoming: &RuntimeShade,
    transition_duration: f64,
) -> Result<HandoffTimingPlan> {
    if !transition_duration.is_finite() || transition_duration <= 0.0 {
        anyhow::bail!(
            "Transition duration must be finite and greater than zero (got {})",
            transition_duration
        );
    }

    let outgoing_unload_length = outgoing
        .state(ShadePhase::Unload)
        .map(|state| state.length.max(0.0))
        .unwrap_or(0.0);

    if outgoing_unload_length > transition_duration + TIMING_EPSILON {
        anyhow::bail!(
            "Outgoing unload length ({:.3}s) exceeds transition duration ({:.3}s)",
            outgoing_unload_length,
            transition_duration
        );
    }

    let outgoing_unload_start_offset = (transition_duration - outgoing_unload_length).max(0.0);
    if outgoing_unload_start_offset > TIMING_EPSILON && outgoing.state(ShadePhase::Active).is_none()
    {
        anyhow::bail!(
            "Outgoing shade has no active phase to backfill {:.3}s before unload",
            outgoing_unload_start_offset
        );
    }

    let incoming_load_length = incoming
        .state(ShadePhase::Load)
        .map(|state| state.length.max(0.0))
        .unwrap_or(0.0);
    if incoming_load_length > transition_duration + TIMING_EPSILON
        && incoming.state(ShadePhase::Active).is_none()
    {
        anyhow::bail!(
            "Incoming load length ({:.3}s) exceeds transition duration ({:.3}s) and no active phase exists to absorb overflow",
            incoming_load_length,
            transition_duration
        );
    }

    Ok(HandoffTimingPlan {
        transition_duration,
        outgoing_unload_start_offset,
        outgoing_unload_length,
        incoming_load_length,
    })
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
    _ipc_handle: JoinHandle<()>,
    builtin_transitions: IndexMap<String, TransitionDef>,
    runtime_shade: Option<RuntimeShade>,
    queued_handoff: Option<QueuedHandoff>,
    active_transition: Option<ActiveTransition>,
    unload_requested: bool,
    current_shade_path: Option<String>,
    start_time: Instant,
    frame: u32,
    phase_elapsed: f64,
    phase_frame: u32,
    loaded_phase: Option<ShadePhase>,
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
    fn builtin_transition_defs() -> IndexMap<String, TransitionDef> {
        crate::transitions::builtin_transition_defs()
    }

    fn builtin_transition_wgsl(id: &str) -> Option<&'static str> {
        crate::transitions::builtin_transition_wgsl(id)
    }

    fn builtin_transition_cache_ids(&self) -> Vec<String> {
        self.builtin_transitions.keys().cloned().collect()
    }

    fn builtin_transition_cache_path() -> Option<PathBuf> {
        if let Ok(path) = env::var("XDG_CACHE_HOME") {
            return Some(
                PathBuf::from(path)
                    .join("kroma")
                    .join("builtin_transitions.json"),
            );
        }

        env::var("HOME").ok().map(|home| {
            PathBuf::from(home)
                .join(".cache")
                .join("kroma")
                .join("builtin_transitions.json")
        })
    }

    fn builtin_transition_cache_is_fresh(&self) -> bool {
        let Some(path) = Self::builtin_transition_cache_path() else {
            return false;
        };

        let Ok(raw) = fs::read_to_string(path) else {
            return false;
        };

        let Ok(cache) = serde_json::from_str::<BuiltinTransitionCache>(&raw) else {
            return false;
        };

        cache.version == BUILTIN_TRANSITION_CACHE_VERSION
            && cache.ids == self.builtin_transition_cache_ids()
    }

    fn write_builtin_transition_cache(&self) {
        let Some(path) = Self::builtin_transition_cache_path() else {
            return;
        };

        let payload = BuiltinTransitionCache {
            version: BUILTIN_TRANSITION_CACHE_VERSION,
            ids: self.builtin_transition_cache_ids(),
        };

        let parent = path.parent().map(Path::to_path_buf);
        if let Some(parent) = parent
            && let Err(e) = fs::create_dir_all(&parent)
        {
            log::warn!(
                "Failed creating builtin transition cache directory '{}': {}",
                parent.display(),
                e
            );
            return;
        }

        match serde_json::to_string_pretty(&payload) {
            Ok(serialized) => {
                if let Err(e) = fs::write(&path, serialized) {
                    log::warn!(
                        "Failed writing builtin transition cache '{}': {}",
                        path.display(),
                        e
                    );
                }
            }
            Err(e) => {
                log::warn!("Failed serializing builtin transition cache: {}", e);
            }
        }
    }

    fn prewarm_builtin_transitions(&self) -> Result<()> {
        if self.builtin_transition_cache_is_fresh() {
            info!("Builtin transition cache is fresh; skipping WGSL validation");
            return Ok(());
        }

        for id in self.builtin_transitions.keys() {
            let source = Self::builtin_transition_wgsl(id)
                .with_context(|| format!("Missing WGSL source for builtin transition '{}'.", id))?;

            let module = naga::front::wgsl::parse_str(source)
                .with_context(|| format!("Failed parsing builtin transition '{}'.", id))?;
            let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
            validator
                .validate(&module)
                .with_context(|| format!("Failed validating builtin transition '{}'.", id))?;
        }

        self.write_builtin_transition_cache();
        info!(
            "Validated and cached {} builtin transitions",
            self.builtin_transitions.len()
        );

        Ok(())
    }

    fn compile_transition_shader(
        &self,
        scope: kroma_shared::transition::TransitionScope,
        id: &str,
        definition: &TransitionDef,
        outgoing: Option<&RuntimeShade>,
        incoming: &RuntimeShade,
    ) -> Result<String> {
        match scope {
            kroma_shared::transition::TransitionScope::Kroma => {
                let wgsl = Self::builtin_transition_wgsl(id)
                    .with_context(|| format!("Unknown builtin transition '{}'.", id))?;
                Ok(wgsl.to_string())
            }
            kroma_shared::transition::TransitionScope::Outgoing
            | kroma_shared::transition::TransitionScope::Incoming => {
                let package = match scope {
                    kroma_shared::transition::TransitionScope::Outgoing => outgoing
                        .map(|runtime| &runtime.package)
                        .with_context(|| "Outgoing transition scope unavailable")?,
                    kroma_shared::transition::TransitionScope::Incoming => &incoming.package,
                    kroma_shared::transition::TransitionScope::Kroma => unreachable!(),
                };

                let shader_bytes = package.read_asset(&definition.shader).with_context(|| {
                    format!("Missing transition shader asset: {}", definition.shader)
                })?;
                let shader_source = String::from_utf8(shader_bytes).with_context(|| {
                    format!(
                        "Transition shader '{}' is not valid UTF-8",
                        definition.shader
                    )
                })?;

                if definition.shader.ends_with(".wgsl") {
                    Ok(shader_source)
                } else {
                    let preprocessed = Self::preprocess_transition_glsl(&shader_source);
                    crate::renderer::glsl_to_wgsl(&preprocessed).with_context(|| {
                        format!(
                            "Failed to compile transition shader '{}' for {}.{}",
                            definition.shader,
                            match scope {
                                kroma_shared::transition::TransitionScope::Kroma => "kroma",
                                kroma_shared::transition::TransitionScope::Outgoing => "outgoing",
                                kroma_shared::transition::TransitionScope::Incoming => "incoming",
                            },
                            id
                        )
                    })
                }
            }
        }
    }

    fn preprocess_transition_glsl(source: &str) -> String {
        let filtered: Vec<&str> = source
            .lines()
            .filter(|line| {
                !(line.contains("uniform")
                    && line.contains("sampler2D")
                    && (line.contains("u_prev_frame") || line.contains("u_next_frame")))
            })
            .collect();

        let mut rewritten = filtered.join("\n");
        rewritten = rewritten.replace("u_prev_frame", "sampler2D(kroma_tex_0, kroma_samp_0)");
        rewritten = rewritten.replace("u_next_frame", "sampler2D(kroma_tex_1, kroma_samp_1)");

        let header = "layout(set = 1, binding = 0) uniform texture2D kroma_tex_0;\nlayout(set = 1, binding = 1) uniform sampler kroma_samp_0;\nlayout(set = 1, binding = 2) uniform texture2D kroma_tex_1;\nlayout(set = 1, binding = 3) uniform sampler kroma_samp_1;\n";

        if let Some(pos) = rewritten.find('\n') {
            let first_line = &rewritten[..pos];
            if first_line.trim_start().starts_with("#version") {
                let rest = &rewritten[pos + 1..];
                return format!("{}\n{}{}", first_line, header, rest);
            }
        }

        format!("{}{}", header, rewritten)
    }

    fn loaded_phase_or_first_defined(&self, runtime: &RuntimeShade) -> Result<ShadePhase> {
        if let Some(phase) = self.loaded_phase {
            return Ok(phase);
        }
        runtime
            .first_defined_phase()
            .with_context(|| "No phase is defined in states")
    }

    fn solve_handoff_timing(
        &self,
        outgoing: &RuntimeShade,
        incoming: &RuntimeShade,
        transition_duration: f64,
    ) -> Result<HandoffTimingPlan> {
        solve_handoff_timing_for_runtime(outgoing, incoming, transition_duration)
    }

    fn emit_transition_trace(
        &self,
        marker: &str,
        spec: &str,
        elapsed: f64,
        duration: f64,
        outgoing: TransitionLaneState,
        incoming: TransitionLaneState,
    ) {
        if !self.config.logging.transition_trace {
            return;
        }

        let progress = if duration > 0.0 {
            (elapsed / duration).clamp(0.0, 1.0)
        } else {
            1.0
        };

        log::debug!(
            "transition_trace frame={} marker={} spec={} progress={:.6} out_phase={:?} out_elapsed={:.6} out_frame={} in_phase={:?} in_elapsed={:.6} in_frame={}",
            self.frame,
            marker,
            spec,
            progress,
            outgoing.phase,
            outgoing.elapsed,
            outgoing.frame,
            incoming.phase,
            incoming.elapsed,
            incoming.frame
        );
    }

    fn start_transition_to_next(&mut self, queued: QueuedHandoff) -> Result<()> {
        let QueuedHandoff {
            request_path,
            incoming,
            preflight,
            ..
        } = queued;

        let TransitionPreflightSession {
            plan,
            timing: handoff_timing,
            outgoing_phase,
            incoming_phase,
            startup,
        } = preflight.with_context(|| "Missing preflight session for queued handoff")?;

        let (width, height) = self.renderer.surface_dimensions();

        info!(
            "Starting handoff '{}' with transition={:.3}s unload_start_offset={:.3}s",
            request_path,
            handoff_timing.transition_duration,
            handoff_timing.outgoing_unload_start_offset
        );

        let outgoing = self
            .runtime_shade
            .as_ref()
            .with_context(|| "No outgoing shade loaded for handoff transition")?;
        let current_outgoing_phase = self.loaded_phase_or_first_defined(outgoing)?;
        if current_outgoing_phase != outgoing_phase {
            anyhow::bail!(
                "Outgoing phase changed after preflight (planned {:?}, current {:?})",
                outgoing_phase,
                current_outgoing_phase
            );
        }

        let next_path = incoming.path.clone();

        let prepared_lane = match startup {
            TransitionStartupPreflight::UnifiedLane { prepared_lane } => prepared_lane,
        };

        self.renderer.begin_transition_with_preflighted_lane(
            &plan.spec,
            &plan.transition_wgsl,
            prepared_lane,
            width,
            height,
        )?;

        self.queued_handoff = None;
        self.pending_request_path = None;
        self.unload_requested = false;
        self.current_shade_path = Some(next_path.clone());

        let mut outgoing_lane = TransitionLaneState {
            phase: Some(outgoing_phase),
            elapsed: 0.0,
            frame: 0,
        };
        let mut outgoing_unload_started = false;

        if handoff_timing.outgoing_unload_start_offset <= TIMING_EPSILON {
            if handoff_timing.outgoing_unload_length <= TIMING_EPSILON {
                outgoing_lane.phase = Some(ShadePhase::Unload);
                outgoing_lane.frame = 0;
                outgoing_unload_started = true;
            } else if outgoing_lane.phase == Some(ShadePhase::Unload) {
                outgoing_unload_started = true;
            }
        }

        let incoming_phase = if incoming_phase == ShadePhase::Load
            && handoff_timing.incoming_load_length <= TIMING_EPSILON
        {
            if incoming.state(ShadePhase::Active).is_some() {
                Some(ShadePhase::Active)
            } else if incoming.state(ShadePhase::Unload).is_some() {
                Some(ShadePhase::Unload)
            } else {
                None
            }
        } else {
            Some(incoming_phase)
        };

        self.active_transition = Some(ActiveTransition {
            spec: plan.spec,
            elapsed: 0.0,
            duration: plan.duration,
            completion: TransitionCompletion::ActivateIncoming(incoming),
            handoff_timing: Some(handoff_timing),
            outgoing: outgoing_lane,
            outgoing_unload_started,
            incoming: TransitionLaneState {
                phase: incoming_phase,
                elapsed: 0.0,
                frame: 0,
            },
        });
        if let Some(active) = self.active_transition.as_ref() {
            self.emit_transition_trace(
                "START_HANDOFF",
                &active.spec,
                active.elapsed,
                active.duration,
                active.outgoing,
                active.incoming,
            );
        }
        self.current_phase = DaemonPhase::Transitioning;
        self.phase_elapsed = 0.0;
        self.phase_frame = 0;
        self.wait_reason = DaemonWaitReason::Idle;

        if self.config.runtime.persist_current_shade {
            self.config.current_shade = Some(next_path);
            if let Err(e) = self.config.save() {
                log::warn!("Failed to persist current shade to config: {}", e);
            }
        }

        Ok(())
    }

    fn start_transition_to_phase(&mut self, target: ShadePhase, spec: &str) -> Result<()> {
        let runtime = self
            .runtime_shade
            .as_ref()
            .with_context(|| "No runtime shade loaded")?;
        let parsed = parse_transition_spec(spec)
            .with_context(|| format!("Invalid lifecycle transition spec '{}'", spec))?;
        let resolved = resolve_transition_spec(
            &parsed,
            &self.builtin_transitions,
            Some(&runtime.package.config.transitions),
            Some(&runtime.package.config.transitions),
        )
        .with_context(|| format!("Unresolved lifecycle transition spec '{}'", spec))?;

        let transition_wgsl = self.compile_transition_shader(
            resolved.scope,
            resolved.id,
            resolved.definition,
            self.runtime_shade.as_ref(),
            runtime,
        )?;

        let outgoing_phase = self.loaded_phase_or_first_defined(runtime)?;
        let prepared_lane = self.renderer.preflight_transition_lane(
            Arc::clone(&runtime.package),
            target,
            runtime.wgsl(target),
            Some(&runtime.path),
        )?;

        let (width, height) = self.renderer.surface_dimensions();
        self.renderer.begin_transition_with_preflighted_lane(
            spec,
            &transition_wgsl,
            prepared_lane,
            width,
            height,
        )?;

        let resume_phase = match target {
            ShadePhase::Load => DaemonPhase::Load,
            ShadePhase::Active => DaemonPhase::Active,
            ShadePhase::Unload => DaemonPhase::Unload,
        };

        self.active_transition = Some(ActiveTransition {
            spec: spec.to_string(),
            elapsed: 0.0,
            duration: resolved.duration,
            completion: TransitionCompletion::ResumePhase(resume_phase),
            handoff_timing: None,
            outgoing: TransitionLaneState {
                phase: Some(outgoing_phase),
                elapsed: self.phase_elapsed,
                frame: self.phase_frame,
            },
            outgoing_unload_started: false,
            incoming: TransitionLaneState {
                phase: Some(target),
                elapsed: 0.0,
                frame: 0,
            },
        });
        if let Some(active) = self.active_transition.as_ref() {
            self.emit_transition_trace(
                "START_LIFECYCLE",
                &active.spec,
                active.elapsed,
                active.duration,
                active.outgoing,
                active.incoming,
            );
        }
        self.current_phase = DaemonPhase::Transitioning;
        self.phase_elapsed = 0.0;
        self.phase_frame = 0;
        self.wait_reason = DaemonWaitReason::Idle;

        Ok(())
    }

    fn precompile_package_transition_shaders(&self, package: &LiveShadePackage) -> Result<()> {
        for (id, transition) in package.config.transitions.iter() {
            let source = package.read_asset(&transition.shader).with_context(|| {
                format!(
                    "Missing transition shader asset '{}' for transitions.{}",
                    transition.shader, id
                )
            })?;
            let source = String::from_utf8(source).with_context(|| {
                format!(
                    "Transition shader '{}' for transitions.{} is not valid UTF-8",
                    transition.shader, id
                )
            })?;

            if transition.shader.ends_with(".wgsl") {
                let module = naga::front::wgsl::parse_str(&source).with_context(|| {
                    format!(
                        "Failed parsing WGSL transition shader '{}' for transitions.{}",
                        transition.shader, id
                    )
                })?;
                let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
                validator.validate(&module).with_context(|| {
                    format!(
                        "Failed validating WGSL transition shader '{}' for transitions.{}",
                        transition.shader, id
                    )
                })?;
            } else {
                let preprocessed = Self::preprocess_transition_glsl(&source);
                crate::renderer::glsl_to_wgsl(&preprocessed).with_context(|| {
                    format!(
                        "Failed compiling transition shader '{}' for transitions.{}",
                        transition.shader, id
                    )
                })?;
            }
        }

        Ok(())
    }

    fn validate_texture_transition_specs(
        &self,
        texture: &TextureDef,
        context: &str,
        incoming_transitions: &IndexMap<String, TransitionDef>,
    ) -> Result<()> {
        if let Some(spec) = texture.transition.as_deref() {
            let parsed = parse_transition_spec(spec)
                .with_context(|| format!("Invalid transition spec '{}' for {}", spec, context))?;

            let resolved = resolve_transition_spec(
                &parsed,
                &self.builtin_transitions,
                None,
                Some(incoming_transitions),
            )
            .with_context(|| format!("Unresolved transition spec '{}' for {}", spec, context))?;

            if texture.ty == TextureType::Slideshow
                && let Some(interval) = texture.interval
                && resolved.duration > interval
            {
                anyhow::bail!(
                    "Transition duration {} exceeds slideshow interval {} for {}",
                    resolved.duration,
                    interval,
                    context
                );
            }
        }

        for (name, child) in texture.textures.iter() {
            self.validate_texture_transition_specs(
                child,
                &format!("{} > textures.{}", context, name),
                incoming_transitions,
            )?;
        }
        for (idx, child) in texture.sources.iter().enumerate() {
            self.validate_texture_transition_specs(
                child,
                &format!("{} > sources[{}]", context, idx),
                incoming_transitions,
            )?;
        }

        Ok(())
    }

    fn validate_transition_config(&self, config: &ShadeConfig) -> Result<()> {
        for (transition_id, transition) in config.transitions.iter() {
            if transition.shader.trim().is_empty() {
                anyhow::bail!("transitions.{}.shader cannot be empty", transition_id);
            }
            if !transition.duration.is_finite() || transition.duration <= 0.0 {
                anyhow::bail!(
                    "transitions.{}.duration must be finite and greater than zero",
                    transition_id
                );
            }

            for (tex_name, texture) in transition.textures.iter() {
                self.validate_texture_transition_specs(
                    texture,
                    &format!("transitions.{}.textures.{}", transition_id, tex_name),
                    &config.transitions,
                )?;
            }
        }

        if let Some(spec) = config.transitions_usage.on_load_to_active.as_deref() {
            let parsed = parse_transition_spec(spec).with_context(|| {
                format!("Invalid transitions_usage.on_load_to_active '{}'", spec)
            })?;
            resolve_transition_spec(
                &parsed,
                &self.builtin_transitions,
                None,
                Some(&config.transitions),
            )
            .with_context(|| {
                format!("Unresolved transitions_usage.on_load_to_active '{}'", spec)
            })?;

            config.states.load.as_ref().with_context(
                || "transitions_usage.on_load_to_active requires [states.load] to be defined",
            )?;
            config.states.active.as_ref().with_context(
                || "transitions_usage.on_load_to_active requires [states.active] to be defined",
            )?;
        }

        if let Some(spec) = config.transitions_usage.on_active_to_unload.as_deref() {
            let parsed = parse_transition_spec(spec).with_context(|| {
                format!("Invalid transitions_usage.on_active_to_unload '{}'", spec)
            })?;
            resolve_transition_spec(
                &parsed,
                &self.builtin_transitions,
                None,
                Some(&config.transitions),
            )
            .with_context(|| {
                format!(
                    "Unresolved transitions_usage.on_active_to_unload '{}'",
                    spec
                )
            })?;

            config.states.active.as_ref().with_context(
                || "transitions_usage.on_active_to_unload requires [states.active] to be defined",
            )?;
            config.states.unload.as_ref().with_context(
                || "transitions_usage.on_active_to_unload requires [states.unload] to be defined",
            )?;
        }

        for (phase_name, state) in [
            ("load", config.states.load.as_ref()),
            ("active", config.states.active.as_ref()),
            ("unload", config.states.unload.as_ref()),
        ] {
            let Some(state) = state else {
                continue;
            };
            for (tex_name, texture) in state.textures.iter() {
                self.validate_texture_transition_specs(
                    texture,
                    &format!("states.{}.textures.{}", phase_name, tex_name),
                    &config.transitions,
                )?;
            }
        }

        Ok(())
    }

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
            transitions: Default::default(),
            transitions_usage: Default::default(),
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
                    transition: None,
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
        let (ipc_handle, ipc_status) = ipc_server::start(cmd_tx)?;
        info!("IPC server listening");

        let mut daemon = Self {
            frame_budget: config.frame_budget(),
            config,
            backend,
            data_provider,
            renderer,
            cmd_rx,
            ipc_status,
            _ipc_handle: ipc_handle,
            builtin_transitions: Self::builtin_transition_defs(),
            runtime_shade: None,
            queued_handoff: None,
            active_transition: None,
            unload_requested: false,
            current_shade_path: None,
            start_time: Instant::now(),
            frame: 0,
            phase_elapsed: 0.0,
            phase_frame: 0,
            loaded_phase: None,
            paused: false,
            current_phase: DaemonPhase::None,
            pending_request_path: None,
            wait_reason: DaemonWaitReason::Idle,
            active_workspace_id: 1,
            last_frame_time: Instant::now(),
            fps_counter: 0,
            fps_timer: Instant::now(),
        };

        daemon.prewarm_builtin_transitions()?;

        if let Some(shade_path) = daemon.config.current_shade.clone() {
            info!("Loading initial shade: {}", shade_path);
            daemon.handle_load_command(&shade_path, true, None, None)?;
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

            match self.process_ipc_commands() {
                Ok(LoopControl::Shutdown) => {
                    self.shutdown()?;
                    return Ok(());
                }
                Ok(LoopControl::Continue) => {}
                Err(e) => {
                    log::error!("IPC command processing failed: {}", e);
                }
            }

            if self.paused {
                self.last_frame_time = Instant::now();
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }

            if let Err(e) = self.render_tick() {
                log::error!("Render tick failed: {}", e);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    fn process_backend_events(&mut self) {
        if let Backend::Wayland { backend, .. } = &mut self.backend {
            let Some(rx) = backend_events(backend) else {
                return;
            };
            while let Ok(event) = rx.try_recv() {
                match event {
                    WaylandEvent::Fullscreen { fullscreen } if self.config.pause_on_fullscreen => {
                        self.paused = fullscreen;
                    }
                    WaylandEvent::WorkspaceChanged { id } => {
                        info!(
                            "Workspace changed to {} (was {})",
                            id, self.active_workspace_id
                        );
                        self.active_workspace_id = id;
                    }
                    WaylandEvent::MonitorChanged { ref name } => {
                        info!("Active monitor: {}", name);
                    }
                    WaylandEvent::Disconnected => {
                        log::warn!("Wayland compositor event source disconnected");
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
                DaemonCommand::LoadShade {
                    path,
                    force,
                    transition,
                } => {
                    if let Err(e) =
                        self.handle_load_command(&path, force, transition, response_tx.clone())
                    {
                        log::error!("Load command failed: {}", e);
                        maybe_send(
                            response_tx,
                            DaemonEvent::Error {
                                message: format!("Load command failed: {}", e),
                            },
                        )?;
                    }
                }
                DaemonCommand::UnloadShade => {
                    if let Err(e) = self.handle_unload_command() {
                        log::error!("Unload command failed: {}", e);
                        maybe_send(
                            response_tx,
                            DaemonEvent::Error {
                                message: format!("Unload command failed: {}", e),
                            },
                        )?;
                    }
                }
                DaemonCommand::Reload => {
                    if let Some(path) = self.current_shade_path.clone() {
                        info!("Reloading shade: {}", path);
                        if let Err(e) =
                            self.handle_load_command(&path, true, None, response_tx.clone())
                        {
                            log::error!("Reload failed: {}", e);
                            maybe_send(
                                response_tx,
                                DaemonEvent::Error {
                                    message: format!("Reload failed: {}", e),
                                },
                            )?;
                        }
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
                    let _ = (width, height);
                    maybe_send(
                        response_tx,
                        DaemonEvent::Error {
                            message: "Preview pipeline is disabled in unified renderer mode"
                                .to_string(),
                        },
                    )?;
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
        transition: Option<String>,
        tx: Option<mpsc::Sender<DaemonEvent>>,
    ) -> Result<()> {
        info!(
            "Load request: path='{}' force={} transition={:?}",
            path, force, transition
        );

        let parsed_transition = match transition.as_deref().map(parse_transition_spec).transpose() {
            Ok(parsed) => parsed,
            Err(e) => {
                let message = format!(
                    "Invalid transition spec '{}': {}",
                    transition.as_deref().unwrap_or(""),
                    e
                );
                return self.send_load_reject(tx, LoadRejectCode::InvalidTransitionSpec, &message);
            }
        };

        let queue_mode = !force
            && self.runtime_shade.is_some()
            && !matches!(
                self.current_phase,
                DaemonPhase::None | DaemonPhase::Terminal
            );

        if let Some((code, message)) = load_reject_for_runtime_state(
            force,
            self.current_phase,
            self.active_transition.is_some(),
            self.queued_handoff.is_some(),
        ) {
            return self.send_load_reject(tx, code, message);
        }

        let prepared = match self.prepare_runtime_shade(path) {
            Ok(runtime) => runtime,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("states.") || msg.contains("No phase is defined in states") {
                    return self.send_load_reject(tx, LoadRejectCode::InvalidStateDefinition, &msg);
                }

                if queue_mode {
                    self.wait_reason = DaemonWaitReason::IncomingPreloadFailed;
                    return self.send_load_reject(tx, LoadRejectCode::IncomingPreloadFailed, &msg);
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

        let requested_transition = if let Some(parsed) = parsed_transition.as_ref() {
            if force {
                log::warn!(
                    "Transition spec '{}' ignored because force=true requests immediate switch",
                    transition.as_deref().unwrap_or("")
                );
                None
            } else {
                let resolved = match resolve_transition_spec(
                    parsed,
                    &self.builtin_transitions,
                    self.runtime_shade
                        .as_ref()
                        .map(|runtime| &runtime.package.config.transitions),
                    Some(&prepared.package.config.transitions),
                ) {
                    Ok(resolved) => resolved,
                    Err(e) => {
                        let message = format!(
                            "Requested transition '{}' could not be resolved: {}",
                            transition.as_deref().unwrap_or(""),
                            e
                        );
                        return self.send_load_reject(
                            tx,
                            LoadRejectCode::InvalidTransitionSpec,
                            &message,
                        );
                    }
                };

                let transition_wgsl = match self.compile_transition_shader(
                    resolved.scope,
                    resolved.id,
                    resolved.definition,
                    self.runtime_shade.as_ref(),
                    &prepared,
                ) {
                    Ok(wgsl) => wgsl,
                    Err(e) => {
                        let message = format!(
                            "Failed to compile transition shader for '{}': {}",
                            transition.as_deref().unwrap_or(""),
                            e
                        );
                        return self.send_load_reject(
                            tx,
                            LoadRejectCode::InvalidTransitionSpec,
                            &message,
                        );
                    }
                };

                Some(PendingTransitionPlan {
                    spec: transition.clone().unwrap_or_default(),
                    duration: resolved.duration,
                    transition_wgsl,
                })
            }
        } else {
            None
        };

        if force
            || self.runtime_shade.is_none()
            || matches!(
                self.current_phase,
                DaemonPhase::None | DaemonPhase::Terminal
            )
        {
            if requested_transition.is_some() {
                log::warn!(
                    "Transition request '{}' ignored because there is no queued shade handoff",
                    transition.as_deref().unwrap_or("")
                );
            }
            self.queued_handoff = None;
            self.pending_request_path = None;
            self.unload_requested = false;
            return self.start_runtime_shade(prepared, tx);
        }

        let transition_preflight = if let Some(plan) = requested_transition {
            let outgoing = self
                .runtime_shade
                .as_ref()
                .with_context(|| "No outgoing shade loaded for transition preflight")?;
            let outgoing_phase = self.loaded_phase_or_first_defined(outgoing)?;
            let incoming_phase = prepared
                .first_defined_phase()
                .with_context(|| "No phase is defined in states")?;

            let timing = match self.solve_handoff_timing(outgoing, &prepared, plan.duration) {
                Ok(timing) => timing,
                Err(e) => {
                    self.wait_reason = DaemonWaitReason::TransitionTimingInfeasible;
                    return self.send_load_reject(
                        tx,
                        LoadRejectCode::TransitionTimingInfeasible,
                        &e.to_string(),
                    );
                }
            };

            let prepared_lane = match self.renderer.preflight_transition_lane(
                Arc::clone(&prepared.package),
                incoming_phase,
                prepared.wgsl(incoming_phase),
                Some(&prepared.path),
            ) {
                Ok(lane) => lane,
                Err(e) => {
                    self.wait_reason = DaemonWaitReason::IncomingPreloadFailed;
                    return self.send_load_reject(
                        tx,
                        LoadRejectCode::IncomingPreloadFailed,
                        &format!("Transition preflight failed: {}", e),
                    );
                }
            };

            Some(TransitionPreflightSession {
                plan,
                timing,
                outgoing_phase,
                incoming_phase,
                startup: TransitionStartupPreflight::UnifiedLane { prepared_lane },
            })
        } else {
            None
        };

        if let Some(preflight) = transition_preflight.as_ref() {
            log::info!(
                "Queued handoff timing: transition={:.3}s outgoing_unload_start={:.3}s outgoing_unload_len={:.3}s incoming_load_len={:.3}s",
                preflight.timing.transition_duration,
                preflight.timing.outgoing_unload_start_offset,
                preflight.timing.outgoing_unload_length,
                preflight.timing.incoming_load_length
            );
        }

        self.pending_request_path = Some(path.to_string());
        self.queued_handoff = Some(QueuedHandoff {
            request_path: path.to_string(),
            incoming: prepared,
            preflight: transition_preflight,
            preload_ready: true,
        });
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
        if self.current_phase == DaemonPhase::Transitioning || self.active_transition.is_some() {
            log::warn!("Ignoring unload command while transition is running");
            return Ok(());
        }

        if self.runtime_shade.is_none() {
            self.current_phase = DaemonPhase::None;
            self.wait_reason = DaemonWaitReason::Idle;
            return Ok(());
        }

        self.queued_handoff = None;
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

        self.validate_transition_config(&package.config)?;
        self.precompile_package_transition_shaders(&package)?;

        let package = Arc::new(package);
        let cache = PhaseWgslCache {
            load: self.precompile_phase_shader(&package, ShadePhase::Load)?,
            active: self.precompile_phase_shader(&package, ShadePhase::Active)?,
            unload: self.precompile_phase_shader(&package, ShadePhase::Unload)?,
        };

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
        self.active_transition = None;
        self.renderer.end_transition();
        self.queued_handoff = None;
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
        self.loaded_phase = Some(phase);
        self.wait_reason = match phase {
            ShadePhase::Load => DaemonWaitReason::RunningLoad,
            ShadePhase::Unload => DaemonWaitReason::RunningUnload,
            ShadePhase::Active => DaemonWaitReason::Idle,
        };

        Ok(outcome)
    }

    fn load_runtime_phase_for_transition(&mut self, phase: ShadePhase) -> Result<ShadeLoadOutcome> {
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
        self.loaded_phase = Some(phase);
        Ok(outcome)
    }

    fn execute_queued_handoff(&mut self, queued: QueuedHandoff) -> Result<()> {
        if queued.preflight.is_some() {
            self.start_transition_to_next(queued)
        } else {
            self.runtime_shade = None;
            self.start_runtime_shade(queued.incoming, None)
        }
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

        let to_active_spec = runtime
            .package
            .config
            .transitions_usage
            .on_load_to_active
            .clone();

        if runtime.state(ShadePhase::Active).is_some() {
            if let Some(spec) = to_active_spec.as_deref() {
                self.start_transition_to_phase(ShadePhase::Active, spec)?;
            } else {
                let outcome = self.enter_phase(ShadePhase::Active)?;
                self.handle_phase_load_outcome(outcome, None)?;
            }
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
        if should_wait_for_incoming_preload(self.queued_handoff.as_ref().map(|q| q.preload_ready)) {
            self.wait_reason = DaemonWaitReason::WaitingIncomingPreload;
            return Ok(());
        }

        if self.current_phase == DaemonPhase::Active
            && let Some(queued) = self.queued_handoff.take()
        {
            if queued.preflight.is_some() {
                self.unload_requested = false;
                self.wait_reason = DaemonWaitReason::Idle;
                return self.start_transition_to_next(queued);
            }
            self.queued_handoff = Some(queued);
        }

        if self
            .runtime_shade
            .as_ref()
            .and_then(|runtime| runtime.state(ShadePhase::Unload))
            .is_some()
        {
            let to_unload_spec = self.runtime_shade.as_ref().and_then(|runtime| {
                runtime
                    .package
                    .config
                    .transitions_usage
                    .on_active_to_unload
                    .clone()
            });
            if let Some(spec) = to_unload_spec.as_deref() {
                self.start_transition_to_phase(ShadePhase::Unload, spec)?;
            } else {
                let outcome = self.enter_phase(ShadePhase::Unload)?;
                self.handle_phase_load_outcome(outcome, None)?;
            }
        } else {
            self.current_phase = DaemonPhase::Terminal;
            self.wait_reason = DaemonWaitReason::Idle;
            self.unload_requested = false;
            if let Some(queued) = self.queued_handoff.take() {
                self.execute_queued_handoff(queued)?;
            }
        }
        Ok(())
    }

    fn finalize_unload(&mut self) -> Result<()> {
        if should_wait_for_incoming_preload(self.queued_handoff.as_ref().map(|q| q.preload_ready)) {
            self.wait_reason = DaemonWaitReason::WaitingIncomingPreload;
            return Ok(());
        }

        self.unload_requested = false;
        self.wait_reason = DaemonWaitReason::Idle;
        if let Some(queued) = self.queued_handoff.take() {
            self.execute_queued_handoff(queued)?;
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
                if self.unload_requested || self.queued_handoff.is_some() {
                    if should_wait_for_incoming_preload(
                        self.queued_handoff.as_ref().map(|q| q.preload_ready),
                    ) {
                        self.wait_reason = DaemonWaitReason::WaitingIncomingPreload;
                        return Ok(());
                    }

                    let loop_len = self.phase_length(ShadePhase::Active);
                    if loop_len <= 0.0 {
                        self.begin_unload_or_finalize()?;
                    } else {
                        let prev = (self.phase_elapsed - dt).max(0.0);
                        let crossed = crossed_active_boundary(prev, self.phase_elapsed, loop_len);
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
            DaemonPhase::Transitioning => {
                if self.active_transition.is_none() {
                    self.renderer.end_transition();
                    self.current_phase = DaemonPhase::Terminal;
                    self.wait_reason = DaemonWaitReason::Idle;
                    return Ok(());
                }

                let mut start_outgoing_unload = false;
                let mut trace_tick: Option<(
                    String,
                    f64,
                    f64,
                    TransitionLaneState,
                    TransitionLaneState,
                )> = None;
                if let Some(active) = self.active_transition.as_mut() {
                    active.elapsed += dt;
                    self.phase_elapsed = active.elapsed;

                    if active.outgoing.phase.is_some() {
                        active.outgoing.elapsed += dt;
                        active.outgoing.frame = active.outgoing.frame.wrapping_add(1);
                    }
                    if active.incoming.phase.is_some() {
                        active.incoming.elapsed += dt;
                        active.incoming.frame = active.incoming.frame.wrapping_add(1);
                    }

                    if let Some(timing) = active.handoff_timing {
                        if !active.outgoing_unload_started
                            && active.elapsed + 1e-6 >= timing.outgoing_unload_start_offset
                        {
                            if timing.outgoing_unload_length <= 1e-6 {
                                active.outgoing_unload_started = true;
                                active.outgoing.phase = Some(ShadePhase::Unload);
                                active.outgoing.elapsed = 0.0;
                                active.outgoing.frame = 0;
                            } else {
                                let has_unload = self
                                    .runtime_shade
                                    .as_ref()
                                    .and_then(|runtime| runtime.state(ShadePhase::Unload))
                                    .is_some();
                                if has_unload && active.outgoing.phase != Some(ShadePhase::Unload) {
                                    start_outgoing_unload = true;
                                }
                                active.outgoing_unload_started = true;
                            }
                        }

                        if active.incoming.phase == Some(ShadePhase::Load)
                            && active.incoming.elapsed + 1e-6 >= timing.incoming_load_length
                        {
                            active.incoming.phase = match &active.completion {
                                TransitionCompletion::ActivateIncoming(next) => {
                                    if next.state(ShadePhase::Active).is_some() {
                                        Some(ShadePhase::Active)
                                    } else if next.state(ShadePhase::Unload).is_some() {
                                        Some(ShadePhase::Unload)
                                    } else {
                                        None
                                    }
                                }
                                TransitionCompletion::ResumePhase(_) => None,
                            };
                            active.incoming.elapsed = 0.0;
                            active.incoming.frame = 0;
                        }
                    }

                    self.renderer.set_transition_lane_progress(
                        active.outgoing.elapsed,
                        active.outgoing.frame,
                        active.incoming.elapsed,
                        active.incoming.frame,
                    );

                    if active.duration > 0.0 {
                        self.renderer
                            .set_transition_progress((active.elapsed / active.duration) as f32);
                    }
                    trace_tick = Some((
                        active.spec.clone(),
                        active.elapsed,
                        active.duration,
                        active.outgoing,
                        active.incoming,
                    ));
                    self.wait_reason = DaemonWaitReason::Idle;
                }

                if let Some((spec, elapsed, duration, outgoing, incoming)) = trace_tick {
                    self.emit_transition_trace(
                        "TICK", &spec, elapsed, duration, outgoing, incoming,
                    );
                }

                if start_outgoing_unload {
                    let outcome = self.load_runtime_phase_for_transition(ShadePhase::Unload)?;
                    self.handle_phase_load_outcome(outcome, None)?;
                    self.renderer.update_textures(0.0)?;
                    if let Some(active) = self.active_transition.as_mut() {
                        active.outgoing.phase = Some(ShadePhase::Unload);
                        active.outgoing.elapsed = 0.0;
                        active.outgoing.frame = 0;
                        active.outgoing_unload_started = true;
                    }
                }

                if self.active_transition.as_ref().is_some_and(|active| {
                    transition_elapsed_complete(active.elapsed, active.duration)
                }) {
                    let active = self
                        .active_transition
                        .take()
                        .context("Missing active transition state")?;
                    info!(
                        "Transition '{}' completed after {:.3}s",
                        active.spec, active.duration
                    );
                    self.emit_transition_trace(
                        "COMPLETE",
                        &active.spec,
                        active.elapsed,
                        active.duration,
                        active.outgoing,
                        active.incoming,
                    );
                    match active.completion {
                        TransitionCompletion::ActivateIncoming(next) => {
                            if !self.renderer.promote_live_transition_incoming()? {
                                self.renderer.end_transition();
                                self.current_phase = DaemonPhase::Terminal;
                                self.wait_reason = DaemonWaitReason::Idle;
                                anyhow::bail!(
                                    "Transition cutover failed: incoming lane promotion unavailable"
                                );
                            }

                            self.emit_transition_trace(
                                "CUTOVER_ACTIVATE",
                                &active.spec,
                                active.elapsed,
                                active.duration,
                                active.outgoing,
                                active.incoming,
                            );

                            let promoted_phase = active
                                .incoming
                                .phase
                                .or_else(|| next.first_defined_phase())
                                .unwrap_or(ShadePhase::Active);
                            self.runtime_shade = Some(next);
                            self.queued_handoff = None;
                            self.pending_request_path = None;
                            self.unload_requested = false;
                            self.loaded_phase = Some(promoted_phase);
                            self.current_phase = match promoted_phase {
                                ShadePhase::Load => DaemonPhase::Load,
                                ShadePhase::Active => DaemonPhase::Active,
                                ShadePhase::Unload => DaemonPhase::Unload,
                            };
                            self.wait_reason = match self.current_phase {
                                DaemonPhase::Load => DaemonWaitReason::RunningLoad,
                                DaemonPhase::Unload => DaemonWaitReason::RunningUnload,
                                _ => DaemonWaitReason::Idle,
                            };
                            self.phase_elapsed = active.incoming.elapsed;
                            self.phase_frame = active.incoming.frame;
                            self.resolve_zero_length_chain()?;
                        }
                        TransitionCompletion::ResumePhase(resume_phase) => {
                            let promoted_phase = active.incoming.phase;
                            if promoted_phase.is_some() {
                                if !self.renderer.promote_live_transition_incoming()? {
                                    self.renderer.end_transition();
                                    self.current_phase = DaemonPhase::Terminal;
                                    self.wait_reason = DaemonWaitReason::Idle;
                                    anyhow::bail!(
                                        "Transition cutover failed: lifecycle incoming lane promotion unavailable"
                                    );
                                }
                            } else {
                                self.renderer.end_transition();
                            }

                            self.emit_transition_trace(
                                "CUTOVER_RESUME",
                                &active.spec,
                                active.elapsed,
                                active.duration,
                                active.outgoing,
                                active.incoming,
                            );

                            if let Some(phase) = promoted_phase {
                                self.loaded_phase = Some(phase);
                            }
                            self.current_phase = resume_phase;
                            self.wait_reason = match self.current_phase {
                                DaemonPhase::Load => DaemonWaitReason::RunningLoad,
                                DaemonPhase::Unload => DaemonWaitReason::RunningUnload,
                                _ => DaemonWaitReason::Idle,
                            };
                            if active.incoming.phase.is_some() {
                                self.phase_elapsed = active.incoming.elapsed;
                                self.phase_frame = active.incoming.frame;
                            } else if self.current_phase == DaemonPhase::Unload
                                && active.handoff_timing.is_some()
                                && active.outgoing.phase == Some(ShadePhase::Unload)
                            {
                                self.phase_elapsed = active.outgoing.elapsed;
                                self.phase_frame = active.outgoing.frame;
                            } else {
                                self.phase_elapsed = 0.0;
                                self.phase_frame = 0;
                            }
                            if self.current_phase == DaemonPhase::Unload {
                                let unload_len = self.phase_length(ShadePhase::Unload);
                                if transition_elapsed_complete(self.phase_elapsed, unload_len) {
                                    self.finalize_unload()?;
                                }
                            }
                            self.resolve_zero_length_chain()?;
                        }
                    }
                }
            }
            DaemonPhase::Terminal | DaemonPhase::None => {
                if let Some(queued) = self.queued_handoff.take() {
                    self.pending_request_path = None;
                    self.unload_requested = false;
                    self.execute_queued_handoff(queued)?;
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
        self.renderer.uniforms.u_transition_t =
            if let Some(active) = self.active_transition.as_ref() {
                if active.duration > 0.0 {
                    (active.elapsed / active.duration).clamp(0.0, 1.0) as f32
                } else {
                    1.0
                }
            } else {
                0.0
            };
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

        self.update_fps_and_status();

        let elapsed = frame_start.elapsed();
        if elapsed < self.frame_budget {
            std::thread::sleep(self.frame_budget - elapsed);
        }

        Ok(())
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

#[cfg(test)]
mod tests {
    use super::{
        PhaseWgslCache, RuntimeShade, crossed_active_boundary, load_reject_for_runtime_state,
        should_wait_for_incoming_preload, solve_handoff_timing_for_runtime,
        transition_elapsed_complete,
    };
    use std::sync::Arc;

    use kroma_shared::ipc::{DaemonPhase, LoadRejectCode};
    use kroma_shared::shade::LiveShadePackage;
    use kroma_shared::types::{
        RenderingConfig, ShadeConfig, ShadeMeta, ShadeStateDef, ShadeStates, ShadeTransitionsUsage,
    };

    fn mk_runtime(load: Option<f64>, active: Option<f64>, unload: Option<f64>) -> RuntimeShade {
        let config = ShadeConfig {
            meta: ShadeMeta {
                name: "Test".to_string(),
                author: "Test".to_string(),
                version: "1.0".to_string(),
                description: String::new(),
                tags: Vec::new(),
            },
            rendering: RenderingConfig::default(),
            states: ShadeStates {
                load: load.map(|len| ShadeStateDef {
                    length: len,
                    shader: None,
                    uniforms: Default::default(),
                    textures: Default::default(),
                    buffers: Default::default(),
                }),
                active: active.map(|len| ShadeStateDef {
                    length: len,
                    shader: None,
                    uniforms: Default::default(),
                    textures: Default::default(),
                    buffers: Default::default(),
                }),
                unload: unload.map(|len| ShadeStateDef {
                    length: len,
                    shader: None,
                    uniforms: Default::default(),
                    textures: Default::default(),
                    buffers: Default::default(),
                }),
            },
            transitions: Default::default(),
            transitions_usage: ShadeTransitionsUsage::default(),
        };

        RuntimeShade {
            path: "test.shade".to_string(),
            package: Arc::new(LiveShadePackage::new_empty(config)),
            cache: PhaseWgslCache::default(),
        }
    }

    #[test]
    fn handoff_timing_solver_accepts_active_backfill_case() {
        let outgoing = mk_runtime(None, Some(0.0), Some(0.3));
        let incoming = mk_runtime(Some(0.1), Some(0.0), None);

        let timing = solve_handoff_timing_for_runtime(&outgoing, &incoming, 0.8)
            .expect("expected feasible timing");
        assert!((timing.outgoing_unload_start_offset - 0.5).abs() < 1e-6);
        assert!((timing.outgoing_unload_length - 0.3).abs() < 1e-6);
    }

    #[test]
    fn handoff_timing_solver_rejects_missing_active_backfill() {
        let outgoing = mk_runtime(None, None, Some(0.3));
        let incoming = mk_runtime(Some(0.1), Some(0.0), None);

        assert!(solve_handoff_timing_for_runtime(&outgoing, &incoming, 0.8).is_err());
    }

    #[test]
    fn handoff_timing_solver_rejects_incoming_load_overflow_without_active() {
        let outgoing = mk_runtime(None, Some(0.0), Some(0.3));
        let incoming = mk_runtime(Some(1.2), None, None);

        assert!(solve_handoff_timing_for_runtime(&outgoing, &incoming, 0.8).is_err());
    }

    #[test]
    fn handoff_timing_solver_accepts_equal_unload_and_transition_without_active() {
        let outgoing = mk_runtime(None, None, Some(0.8));
        let incoming = mk_runtime(Some(0.1), Some(0.0), None);

        let timing = solve_handoff_timing_for_runtime(&outgoing, &incoming, 0.8)
            .expect("expected equal-length unload to be feasible");
        assert!((timing.outgoing_unload_start_offset - 0.0).abs() < 1e-6);
        assert!((timing.outgoing_unload_length - 0.8).abs() < 1e-6);
    }

    #[test]
    fn handoff_timing_solver_rejects_outgoing_unload_overflow() {
        let outgoing = mk_runtime(None, Some(0.0), Some(0.9));
        let incoming = mk_runtime(Some(0.1), Some(0.0), None);

        assert!(solve_handoff_timing_for_runtime(&outgoing, &incoming, 0.8).is_err());
    }

    #[test]
    fn handoff_timing_solver_accepts_zero_load_and_unload() {
        let outgoing = mk_runtime(None, Some(0.0), None);
        let incoming = mk_runtime(None, Some(0.0), None);

        let timing = solve_handoff_timing_for_runtime(&outgoing, &incoming, 0.6)
            .expect("expected missing load/unload to be treated as zero");
        assert!((timing.outgoing_unload_length - 0.0).abs() < 1e-6);
        assert!((timing.incoming_load_length - 0.0).abs() < 1e-6);
        assert!((timing.outgoing_unload_start_offset - 0.6).abs() < 1e-6);
    }

    #[test]
    fn handoff_timing_solver_accepts_incoming_load_overflow_with_active() {
        let outgoing = mk_runtime(None, Some(0.0), Some(0.1));
        let incoming = mk_runtime(Some(1.2), Some(0.0), None);

        let timing = solve_handoff_timing_for_runtime(&outgoing, &incoming, 0.8)
            .expect("expected incoming overflow to be absorbed by active phase");
        assert!((timing.incoming_load_length - 1.2).abs() < 1e-6);
    }

    #[test]
    fn handoff_timing_solver_rejects_outgoing_without_active_backfill_window() {
        let outgoing = mk_runtime(Some(0.1), None, None);
        let incoming = mk_runtime(Some(0.1), Some(0.0), None);

        assert!(solve_handoff_timing_for_runtime(&outgoing, &incoming, 0.8).is_err());
    }

    #[test]
    fn handoff_timing_solver_rejects_non_positive_duration() {
        let outgoing = mk_runtime(None, Some(0.0), Some(0.1));
        let incoming = mk_runtime(Some(0.1), Some(0.0), None);

        assert!(solve_handoff_timing_for_runtime(&outgoing, &incoming, 0.0).is_err());
        assert!(solve_handoff_timing_for_runtime(&outgoing, &incoming, -0.2).is_err());
    }

    #[test]
    fn load_reject_helper_covers_busy_states() {
        assert_eq!(
            load_reject_for_runtime_state(false, DaemonPhase::Transitioning, false, false)
                .map(|v| v.0),
            Some(LoadRejectCode::BusyTransitioning)
        );
        assert_eq!(
            load_reject_for_runtime_state(false, DaemonPhase::Active, true, false).map(|v| v.0),
            Some(LoadRejectCode::BusyTransitioning)
        );
        assert_eq!(
            load_reject_for_runtime_state(false, DaemonPhase::Active, false, true).map(|v| v.0),
            Some(LoadRejectCode::BusyWaitingBoundary)
        );
        assert_eq!(
            load_reject_for_runtime_state(false, DaemonPhase::Load, false, false).map(|v| v.0),
            Some(LoadRejectCode::BusyRunningLoad)
        );
        assert_eq!(
            load_reject_for_runtime_state(false, DaemonPhase::Unload, false, false).map(|v| v.0),
            Some(LoadRejectCode::BusyRunningUnload)
        );
        assert!(load_reject_for_runtime_state(true, DaemonPhase::Load, false, true).is_none());
        assert!(load_reject_for_runtime_state(false, DaemonPhase::Active, false, false).is_none());
    }

    #[test]
    fn active_boundary_cross_detection_matches_expected_edges() {
        assert!(!crossed_active_boundary(0.10, 0.49, 0.50));
        assert!(crossed_active_boundary(0.49, 0.50, 0.50));
        assert!(crossed_active_boundary(0.49, 1.02, 0.50));
    }

    #[test]
    fn transition_completion_uses_epsilon_cutover() {
        assert!(transition_elapsed_complete(0.999_999_9, 1.0));
        assert!(!transition_elapsed_complete(0.95, 1.0));
    }

    #[test]
    fn preload_gate_waits_only_when_incoming_not_ready() {
        assert!(should_wait_for_incoming_preload(Some(false)));
        assert!(!should_wait_for_incoming_preload(Some(true)));
        assert!(!should_wait_for_incoming_preload(None));
    }
}
