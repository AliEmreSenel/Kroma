//! wgpu render pipeline for Kroma.
//!
//! Manages the GPU device, shader module, uniform buffer, and frame
//! rendering. Renders to a real Wayland or X11 surface via wgpu.

use std::collections::HashMap;
use std::sync::{Arc, mpsc};

use anyhow::{Context, Result};
use indexmap::IndexMap;
use kroma_shared::traits::SurfaceProvider;
use log::{info, warn};

use kroma_shared::shade::LiveShadePackage;
use kroma_shared::traits::LightingFrame;
use kroma_shared::types::{
    MonitorConfig, MonitorId, ShadePhase, ShaderUniforms, TextureDef, UniformDef,
};

mod render_flow;
mod shader_translation;

use crate::{
    config::GpuPower,
    fallback,
    textures::{self, GpuContext, TextureSource, TextureUpdate},
};

/// Fullscreen triangle vertex shader.
const FULLSCREEN_VERT_WGSL: &str = include_str!("shaders/fullscreen.vert.wgsl");

/// Fragment shader for image/video mode — samples texture 0.
const IMAGE_SAMPLER_FRAG_WGSL: &str = include_str!("shaders/image_sampler.frag.wgsl");

/// Fallback fragment shader for error display (text bitmap + GPU background).
const FALLBACK_ERROR_FRAG_WGSL: &str = include_str!("shaders/fallback_error.frag.wgsl");

/// Minimal solid-color shader used only during GPU init, before the real
/// fallback (which needs a texture bind group) is set up.
const INIT_FRAG_WGSL: &str = include_str!("shaders/init.frag.wgsl");

/// Texture-copy shader used for final presentation and lighting downsampling.
const OUTPUT_BLIT_FRAG_WGSL: &str = include_str!("shaders/output_blit.frag.wgsl");

/// The preferred surface texture format.
const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

/// Builds a `SurfaceConfiguration` whose format, alpha mode, and present
/// mode are validated against the adapter's reported surface capabilities
/// (the preferred format is used when supported, the first srgb-capable
/// one otherwise, and the first reported one as a last resort).
pub(crate) fn compatible_surface_config(
    caps: &wgpu::SurfaceCapabilities,
    preferred: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> Option<wgpu::SurfaceConfiguration> {
    let format = if caps.formats.contains(&preferred) {
        preferred
    } else if let Some(srgb) = caps.formats.iter().find(|f| f.is_srgb()) {
        *srgb
    } else {
        *caps.formats.first()?
    };
    let alpha_mode = *caps.alpha_modes.first()?;
    if width == 0 || height == 0 || !caps.usages.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
        return None;
    }
    Some(wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: width.max(1),
        height: height.max(1),
        present_mode: if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else if caps.present_modes.contains(&wgpu::PresentMode::Fifo) {
            wgpu::PresentMode::Fifo
        } else {
            *caps.present_modes.first()?
        },
        alpha_mode,
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    })
}

/// Maximum number of custom uniform float slots.
const MAX_CUSTOM_UNIFORMS: usize = 32;

/// Width of the exact final-output lighting capture.
const LIGHTING_CAPTURE_WIDTH: u32 = 128;

/// Height of the exact final-output lighting capture.
const LIGHTING_CAPTURE_HEIGHT: u32 = 64;

/// Padded byte width of one lighting-capture row.
const LIGHTING_CAPTURE_BYTES_PER_ROW: u32 = 512;

// ---------------------------------------------------------------------------
// Shade load outcome
// ---------------------------------------------------------------------------

/// Outcome of [`Renderer::load_shade`].
///
/// The daemon should inspect this to decide what IPC message to send.
#[derive(Debug)]
pub enum ShadeLoadOutcome {
    /// Shade loaded and compiled successfully.
    Success,
    /// One or more **required** textures could not be loaded.
    /// The daemon is now rendering a fallback error image showing the paths.
    TextureError(Vec<TextureLoadFailure>),
    /// The GLSL shader failed to compile / translate.
    /// The daemon is now rendering a fallback error image showing the message.
    CompileError(String),
}

/// Describes a single texture that failed to load.
#[derive(Debug, Clone)]
pub struct TextureLoadFailure {
    /// Config key / channel name.
    pub name: String,
    /// The `source` path from config (or "<unknown>").
    pub source: String,
    /// Human-readable error message.
    pub error: String,
}

/// A loaded GPU texture with its sampler.
#[derive(Debug)]
struct LoadedTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    width: u32,
    height: u32,
    /// Pixel format (needed for correct uploads and bind group layout).
    format: wgpu::TextureFormat,
}

struct TransitionRenderState {
    _spec: String,
    pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    incoming_lane: RendererLane,
}

/// Independent monitor output and asynchronous lighting readback.
struct OutputCaptureState {
    _final_texture: wgpu::Texture,
    final_view: wgpu::TextureView,
    output_bind_group: wgpu::BindGroup,
    /// Pipeline layout wrapping the canvas sampler/texture bind group.
    blit_pipeline_layout: wgpu::PipelineLayout,
    /// Fragment module sampling the canvas (OUTPUT_BLIT_FRAG_WGSL).
    blit_fragment: wgpu::ShaderModule,
    capture_texture: wgpu::Texture,
    capture_view: wgpu::TextureView,
    capture_pipeline: wgpu::RenderPipeline,
    readback_buffer: wgpu::Buffer,
    completion_tx: mpsc::Sender<std::result::Result<(), wgpu::BufferAsyncError>>,
    completion_rx: mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
    mapping_pending: bool,
}

#[derive(Clone, Copy)]
struct TransitionLaneUniformState {
    outgoing_elapsed: f32,
    outgoing_frame: u32,
    incoming_elapsed: f32,
    incoming_frame: u32,
}

struct RendererLane {
    lane_id: u64,
    shader_defs: Vec<(usize, Arc<LiveShadePackage>, TextureDef)>,
    active_package: Option<Arc<LiveShadePackage>>,
    custom_uniforms: HashMap<String, kroma_shared::ipc::UniformValue>,
    custom_uniform_indices: HashMap<String, usize>,
    custom_uniform_data: Vec<f32>,
    custom_uniform_buffer: Option<wgpu::Buffer>,
    pipeline: Option<wgpu::RenderPipeline>,
    bind_group: Option<wgpu::BindGroup>,
    bind_group_layout: Option<wgpu::BindGroupLayout>,
    pipeline_layout: Option<wgpu::PipelineLayout>,
    texture_bind_group: Option<wgpu::BindGroup>,
    texture_bind_group_layout: Option<wgpu::BindGroupLayout>,
    textures: Vec<LoadedTexture>,
    texture_sources: Vec<Option<Box<dyn TextureSource>>>,
    current_frag_wgsl: String,
    buffer_passes: Vec<BufferPassState>,
}

impl RendererLane {
    fn blank() -> Self {
        Self {
            lane_id: 0,
            shader_defs: Vec::new(),
            active_package: None,
            custom_uniforms: HashMap::new(),
            custom_uniform_indices: HashMap::new(),
            custom_uniform_data: vec![0.0; MAX_CUSTOM_UNIFORMS],
            custom_uniform_buffer: None,
            pipeline: None,
            bind_group: None,
            bind_group_layout: None,
            pipeline_layout: None,
            texture_bind_group: None,
            texture_bind_group_layout: None,
            textures: Vec::new(),
            texture_sources: Vec::new(),
            current_frag_wgsl: INIT_FRAG_WGSL.to_string(),
            buffer_passes: Vec::new(),
        }
    }
}

pub(crate) struct PreparedTransitionLane {
    lane: RendererLane,
}

/// State for a single render buffer pass (multi-pass rendering).
#[allow(dead_code)]
struct BufferPassState {
    /// Name of this buffer (e.g. "A", "B", "C", "D").
    name: String,
    /// Render pipeline for this buffer pass.
    pipeline: wgpu::RenderPipeline,
    /// Bind group for this pass (uniforms + inputs).
    bind_group: wgpu::BindGroup,
    /// Current render target texture.
    texture: wgpu::Texture,
    /// Texture view for rendering to.
    texture_view: wgpu::TextureView,
    /// Texture view for sampling from (used by subsequent passes).
    sampler_view: wgpu::TextureView,
    /// Previous frame texture (for feedback buffers).
    prev_texture: Option<wgpu::Texture>,
    prev_view: Option<wgpu::TextureView>,
    /// Whether this buffer uses self-feedback.
    feedback: bool,
}

/// Presentation resources for a single monitor.
///
/// Keyed by monitor *name* (stable across re-enumeration). Each monitor owns
/// its wgpu surface, the configuration it was last configured with, and the
/// pipeline that stretches this monitor's independently rendered output.
pub(crate) struct MonitorTarget {
    /// Provider-level id of this monitor (for occlusion matching).
    pub(crate) id: MonitorId,
    /// Native window handle; kept so a lost wgpu surface can be recreated.
    pub(crate) window: Option<raw_window_handle::RawWindowHandle>,
    pub(crate) surface: Option<wgpu::Surface<'static>>,
    pub(crate) config: Option<wgpu::SurfaceConfiguration>,
    pub(crate) blit: Option<wgpu::RenderPipeline>,
    output: OutputCaptureState,
    globals: [wgpu::Buffer; 3],
    transition: Option<MonitorTransitionTargets>,
    shader_sources: HashMap<u64, MonitorShaderSources>,
    needs_reconfigure: bool,
    retry_after: std::time::Instant,
}

type MonitorShaderSources = Vec<(usize, Box<dyn TextureSource>)>;

struct MonitorTransitionTargets {
    prev_view: wgpu::TextureView,
    next_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

fn target_ready(target: &MonitorTarget, occluded: &[MonitorId], now: std::time::Instant) -> bool {
    !occluded.contains(&target.id)
        && now >= target.retry_after
        && (target.window.is_none() || target.surface.is_some())
}

/// GPU-bound resources owned by the renderer.
///
/// This struct groups low-level wgpu handles away from orchestration state,
/// which keeps [`Renderer`] easier to reason about.
///
/// Scene programs and media are shared; each target owns its render attachments.
#[derive(Default)]
pub(crate) struct GpuResources {
    instance: Option<wgpu::Instance>,
    /// Kept so surfaces can be re-validated (capabilities) on hotplug.
    adapter: Option<wgpu::Adapter>,
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
    pipeline: Option<wgpu::RenderPipeline>,
    uniform_buffer: Option<wgpu::Buffer>,
    bind_group: Option<wgpu::BindGroup>,
    bind_group_layout: Option<wgpu::BindGroupLayout>,
    pipeline_layout: Option<wgpu::PipelineLayout>,
    vert_module: Option<wgpu::ShaderModule>,
    texture_bind_group: Option<wgpu::BindGroup>,
    texture_bind_group_layout: Option<wgpu::BindGroupLayout>,
    canvas_size: Option<(u32, u32)>,
    /// Render targets keyed by monitor name.
    targets: HashMap<String, MonitorTarget>,
    /// Display handle used to create wgpu surfaces for hotplugged monitors.
    raw_display: Option<raw_window_handle::RawDisplayHandle>,
}

/// Orchestrates rendering, shader loading, uniforms, and texture updates.
pub struct Renderer {
    lane_id: u64,
    next_lane_id: u64,
    shader_defs: Vec<(usize, Arc<LiveShadePackage>, TextureDef)>,
    /// Current shader uniforms (CPU side).
    pub uniforms: ShaderUniforms,

    pub active_package: Option<Arc<LiveShadePackage>>,

    /// Custom uniforms set via IPC (name → value).
    custom_uniforms: HashMap<String, kroma_shared::ipc::UniformValue>,
    /// Mapping from custom uniform name → index in the storage buffer.
    custom_uniform_indices: HashMap<String, usize>,
    /// CPU-side custom uniform data (uploaded to GPU each frame).
    custom_uniform_data: Vec<f32>,
    /// GPU storage buffer for custom uniform values.
    custom_uniform_buffer: Option<wgpu::Buffer>,

    /// Low-level GPU handles and pipeline objects.
    gpu: GpuResources,

    // Texture resources (bind group 1)
    textures: Vec<LoadedTexture>,
    /// Self-contained texture sources (image, video, font, slideshow, audio).
    /// Parallel to `textures` — `texture_sources[i]` drives `textures[i]`.
    texture_sources: Vec<Option<Box<dyn TextureSource>>>,
    /// Current fragment shader source (WGSL).
    current_frag_wgsl: String,
    /// Active transition tag while daemon runs a transition handoff.
    transition_spec: Option<String>,
    /// Normalized transition progress in [0, 1].
    transition_progress: f32,
    transition_state: Option<TransitionRenderState>,
    transition_lane_uniforms: Option<TransitionLaneUniformState>,

    /// Multi-pass buffer states (Shadertoy-style Buffer A/B/C/D).
    buffer_passes: Vec<BufferPassState>,
}

impl Renderer {
    /// Creates a renderer instance without initializing GPU resources.
    pub fn new() -> Result<Self> {
        Ok(Self {
            lane_id: 0,
            next_lane_id: 1,
            shader_defs: Vec::new(),
            active_package: None,
            uniforms: ShaderUniforms::default(),
            custom_uniforms: std::collections::HashMap::new(),
            custom_uniform_indices: std::collections::HashMap::new(),
            custom_uniform_data: vec![0.0; MAX_CUSTOM_UNIFORMS],
            custom_uniform_buffer: None,
            gpu: GpuResources::default(),
            textures: Vec::new(),
            texture_sources: Vec::new(),
            current_frag_wgsl: INIT_FRAG_WGSL.to_string(),
            transition_spec: None,
            transition_progress: 0.0,
            transition_state: None,
            transition_lane_uniforms: None,
            buffer_passes: Vec::new(),
        })
    }

    /// Logical canvas size: the reference monitor's dimensions.
    ///
    /// All scene rendering and `u_resolution` are based on this size, and
    /// each monitor's physical surface is fed by stretching the canvas to
    /// fit. Falls back to the current uniform resolution before GPU init.
    pub fn surface_dimensions(&self) -> (u32, u32) {
        self.gpu.canvas_size.unwrap_or((
            self.uniforms.u_resolution[0].max(1.0) as u32,
            self.uniforms.u_resolution[1].max(1.0) as u32,
        ))
    }

    fn take_active_lane(&mut self) -> RendererLane {
        RendererLane {
            lane_id: self.lane_id,
            shader_defs: std::mem::take(&mut self.shader_defs),
            active_package: self.active_package.take(),
            custom_uniforms: std::mem::take(&mut self.custom_uniforms),
            custom_uniform_indices: std::mem::take(&mut self.custom_uniform_indices),
            custom_uniform_data: std::mem::take(&mut self.custom_uniform_data),
            custom_uniform_buffer: self.custom_uniform_buffer.take(),
            pipeline: self.gpu.pipeline.take(),
            bind_group: self.gpu.bind_group.take(),
            bind_group_layout: self.gpu.bind_group_layout.take(),
            pipeline_layout: self.gpu.pipeline_layout.take(),
            texture_bind_group: self.gpu.texture_bind_group.take(),
            texture_bind_group_layout: self.gpu.texture_bind_group_layout.take(),
            textures: std::mem::take(&mut self.textures),
            texture_sources: std::mem::take(&mut self.texture_sources),
            current_frag_wgsl: std::mem::take(&mut self.current_frag_wgsl),
            buffer_passes: std::mem::take(&mut self.buffer_passes),
        }
    }

    fn install_active_lane(&mut self, lane: RendererLane) {
        let RendererLane {
            lane_id,
            shader_defs,
            active_package,
            custom_uniforms,
            custom_uniform_indices,
            custom_uniform_data,
            custom_uniform_buffer,
            pipeline,
            bind_group,
            bind_group_layout,
            pipeline_layout,
            texture_bind_group,
            texture_bind_group_layout,
            textures,
            texture_sources,
            current_frag_wgsl,
            buffer_passes,
        } = lane;

        self.lane_id = lane_id;
        self.shader_defs = shader_defs;
        self.active_package = active_package;
        self.custom_uniforms = custom_uniforms;
        self.custom_uniform_indices = custom_uniform_indices;
        self.custom_uniform_data = custom_uniform_data;
        self.custom_uniform_buffer = custom_uniform_buffer;
        self.gpu.pipeline = pipeline;
        self.gpu.bind_group = bind_group;
        self.gpu.bind_group_layout = bind_group_layout;
        self.gpu.pipeline_layout = pipeline_layout;
        self.gpu.texture_bind_group = texture_bind_group;
        self.gpu.texture_bind_group_layout = texture_bind_group_layout;
        self.textures = textures;
        self.texture_sources = texture_sources;
        self.current_frag_wgsl = current_frag_wgsl;
        self.buffer_passes = buffer_passes;
    }

    fn begin_transition_internal(
        &mut self,
        spec: &str,
        transition_wgsl: &str,
        incoming_lane: RendererLane,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let device = self.gpu.device.as_ref().context("GPU not initialised")?;
        let bgl0 = self
            .gpu
            .bind_group_layout
            .as_ref()
            .context("Uniform BGL missing")?;

        let fmt = self.active_surface_format();
        let _ = (width, height); // The playback canvas determines all target sizes.
        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kroma-transition-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let transition_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("kroma-transition-pl"),
            bind_group_layouts: &[Some(bgl0), Some(&texture_bgl)],
            immediate_size: 0,
        });

        let vert_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kroma-transition-vert"),
            source: wgpu::ShaderSource::Wgsl(FULLSCREEN_VERT_WGSL.into()),
        });
        let frag_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kroma-transition-frag"),
            source: wgpu::ShaderSource::Wgsl(transition_wgsl.into()),
        });

        let pipeline = Self::create_fullscreen_pipeline(
            device,
            &transition_layout,
            &vert_module,
            &frag_module,
            fmt,
            "kroma-transition-pipeline",
        );

        for target in self.gpu.targets.values_mut() {
            target.transition = None;
        }
        self.transition_spec = Some(spec.to_string());
        self.transition_progress = 0.0;
        self.transition_state = Some(TransitionRenderState {
            _spec: spec.to_string(),
            pipeline,
            texture_layout: texture_bgl,
            incoming_lane,
        });
        self.transition_lane_uniforms = Some(TransitionLaneUniformState {
            outgoing_elapsed: 0.0,
            outgoing_frame: 0,
            incoming_elapsed: 0.0,
            incoming_frame: 0,
        });

        Ok(())
    }

    pub(crate) fn preflight_transition_lane(
        &mut self,
        pkg: Arc<LiveShadePackage>,
        phase: ShadePhase,
        precompiled_wgsl: Option<&str>,
        shade_path: Option<&str>,
    ) -> Result<PreparedTransitionLane> {
        let outgoing_lane = self.take_active_lane();
        self.install_active_lane(RendererLane::blank());
        self.rebuild_bind_group_0()
            .with_context(|| "Failed to initialize incoming lane uniform bindings")?;

        let load_result = self.load_phase(pkg, phase, precompiled_wgsl, shade_path);
        let incoming_lane = self.take_active_lane();
        self.install_active_lane(outgoing_lane);

        let outcome = load_result?;
        match outcome {
            ShadeLoadOutcome::Success => Ok(PreparedTransitionLane {
                lane: incoming_lane,
            }),
            ShadeLoadOutcome::TextureError(failures) => {
                let summary = failures
                    .iter()
                    .map(|f| format!("{} ({}): {}", f.name, f.source, f.error))
                    .collect::<Vec<_>>()
                    .join("; ");
                anyhow::bail!(
                    "Incoming transition lane preflight failed due to texture errors: {}",
                    summary
                );
            }
            ShadeLoadOutcome::CompileError(msg) => {
                anyhow::bail!(
                    "Incoming transition lane preflight shader compile failed: {}",
                    msg
                );
            }
        }
    }

    pub(crate) fn begin_transition_with_preflighted_lane(
        &mut self,
        spec: &str,
        transition_wgsl: &str,
        prepared: PreparedTransitionLane,
        width: u32,
        height: u32,
    ) -> Result<()> {
        self.begin_transition_internal(spec, transition_wgsl, prepared.lane, width, height)
    }

    /// Updates normalized transition progress.
    pub fn set_transition_progress(&mut self, progress: f32) {
        self.transition_progress = progress.clamp(0.0, 1.0);
    }

    pub fn set_transition_lane_progress(
        &mut self,
        outgoing_elapsed: f64,
        outgoing_frame: u32,
        incoming_elapsed: f64,
        incoming_frame: u32,
    ) {
        self.transition_lane_uniforms = Some(TransitionLaneUniformState {
            outgoing_elapsed: outgoing_elapsed.max(0.0) as f32,
            outgoing_frame,
            incoming_elapsed: incoming_elapsed.max(0.0) as f32,
            incoming_frame,
        });
    }

    /// Ends an active daemon transition handoff.
    pub fn end_transition(&mut self) {
        self.transition_progress = 0.0;
        self.transition_spec = None;
        self.transition_state = None;
        self.transition_lane_uniforms = None;
    }

    pub fn promote_live_transition_incoming(&mut self) -> Result<bool> {
        let Some(transition) = self.transition_state.take() else {
            return Ok(false);
        };

        self.install_active_lane(transition.incoming_lane);

        self.transition_progress = 0.0;
        self.transition_spec = None;
        self.transition_state = None;
        self.transition_lane_uniforms = None;

        Ok(true)
    }

    /// Returns the active surface format used for pipeline and texture setup.
    fn active_surface_format(&self) -> wgpu::TextureFormat {
        render_flow::active_surface_format(self)
    }

    /// Creates the fullscreen render pipeline for the current shader pair.
    fn create_fullscreen_pipeline(
        device: &wgpu::Device,
        pipeline_layout: &wgpu::PipelineLayout,
        vert_module: &wgpu::ShaderModule,
        frag_module: &wgpu::ShaderModule,
        format: wgpu::TextureFormat,
        label: &str,
    ) -> wgpu::RenderPipeline {
        render_flow::create_fullscreen_pipeline(
            device,
            pipeline_layout,
            vert_module,
            frag_module,
            format,
            label,
        )
    }

    /// Initializes one GPU context and independent targets for enabled outputs.
    pub fn init_gpu_with_surface(
        &mut self,
        gpu_pref: &GpuPower,
        surface: &dyn SurfaceProvider,
    ) -> Result<()> {
        let mut monitors = surface.list_monitors()?;
        monitors.sort_by(|a, b| a.name.cmp(&b.name));
        let (canvas_w, canvas_h) = monitors
            .first()
            .map(|m| surface.size(m.id).unwrap_or((m.width, m.height)))
            .unwrap_or((1920, 1080));
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let raw_display = if surface.is_headless() {
            None
        } else {
            Some(surface.display_handle()?)
        };
        // Select against an actual presentation surface when one is ready.
        // SAFETY: the provider owns these native handles throughout initialization.
        let compatible = raw_display.and_then(|display| {
            monitors.iter().find_map(|monitor| {
                let window = surface.create_surface(monitor.id).ok()?;
                unsafe {
                    instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                        raw_display_handle: Some(display),
                        raw_window_handle: window,
                    })
                }
                .ok()
            })
        });
        let adapter = pollster_block(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: gpu_pref.into(),
            compatible_surface: compatible.as_ref(),
            force_fallback_adapter: false,
        }))
        .context("No GPU adapter found")?;
        log::info!("Selected GPU: {:?}", adapter.get_info());
        drop(compatible);
        let (device, queue) = pollster_block(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::FLOAT32_FILTERABLE,
            label: Some("kroma-device"),
            ..Default::default()
        }))
        .context("Failed to create GPU device")?;
        let device = Arc::new(device);
        let queue = Arc::new(queue);
        let format = SURFACE_FORMAT;
        // Create uniform buffer
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("kroma-uniforms"),
            size: std::mem::size_of::<ShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kroma-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kroma-bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("kroma-pl"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // Shader modules
        let vert_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kroma-vert"),
            source: wgpu::ShaderSource::Wgsl(FULLSCREEN_VERT_WGSL.into()),
        });

        let frag_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kroma-frag"),
            source: wgpu::ShaderSource::Wgsl(self.current_frag_wgsl.clone().into()),
        });

        let pipeline = Self::create_fullscreen_pipeline(
            &device,
            &pipeline_layout,
            &vert_module,
            &frag_module,
            format,
            "kroma-pipeline",
        );

        self.gpu.instance = Some(instance);
        self.gpu.adapter = Some(adapter);
        self.gpu.raw_display = raw_display;
        self.gpu.device = Some(Arc::clone(&device));
        self.gpu.queue = Some(queue);
        self.gpu.pipeline = Some(pipeline);
        self.gpu.uniform_buffer = Some(uniform_buffer);
        self.gpu.bind_group = Some(bind_group);
        self.gpu.bind_group_layout = Some(bind_group_layout);
        self.gpu.pipeline_layout = Some(pipeline_layout);
        self.gpu.vert_module = Some(vert_module);
        let canvas_w = canvas_w.clamp(1, device.limits().max_texture_dimension_2d);
        let canvas_h = canvas_h.clamp(1, device.limits().max_texture_dimension_2d);
        self.gpu.canvas_size = Some((canvas_w, canvas_h));
        self.uniforms.u_resolution = [canvas_w.max(1) as f32, canvas_h.max(1) as f32];
        self.sync_targets(surface, &monitors);
        info!(
            "GPU initialized: canvas {}x{}, {} independent monitor targets",
            canvas_w,
            canvas_h,
            self.gpu.targets.len()
        );
        Ok(())
    }

    pub fn load_phase(
        &mut self,
        pkg: Arc<LiveShadePackage>,
        phase: ShadePhase,
        precompiled_wgsl: Option<&str>,
        shade_path: Option<&str>,
    ) -> Result<ShadeLoadOutcome> {
        let state = match phase {
            ShadePhase::Load => pkg.config.states.load.as_ref(),
            ShadePhase::Active => pkg.config.states.active.as_ref(),
            ShadePhase::Unload => pkg.config.states.unload.as_ref(),
        }
        .with_context(|| format!("State {:?} is not defined in config.toml", phase))?;

        // Reset texture state
        self.textures.clear();
        self.texture_sources.clear();
        self.buffer_passes.clear();

        // Load all textures from package config
        let tex_failures = self.load_package_textures(&pkg, &state.textures)?;

        // If any *required* textures failed, switch to fallback error display.
        if !tex_failures.is_empty() {
            let paths: Vec<String> = tex_failures
                .iter()
                .map(|f| format!("{}: {} ({})", f.name, f.source, f.error))
                .collect();
            self.switch_to_fallback_error(fallback::render_texture_error, &paths)?;
            info!(
                "Shade '{}' loaded with texture errors — showing fallback",
                pkg.config.meta.name,
            );
            self.active_package = Some(pkg);
            return Ok(ShadeLoadOutcome::TextureError(tex_failures));
        }

        // Initialize custom uniforms (creates storage buffer + rebuilds BGL0).
        // Must happen BEFORE shader compilation so GLSL referencing binding=1 works.
        self.init_custom_uniforms(&state.uniforms)?;

        match &state.shader {
            Some(shader_path) => {
                let shader_bytes = pkg
                    .read_asset(shader_path)
                    .with_context(|| format!("Missing phase shader asset: {}", shader_path))?;
                let glsl_source = String::from_utf8(shader_bytes).with_context(|| {
                    format!("Phase shader '{}' is not valid UTF-8", shader_path)
                })?;
                info!(
                    "Compiling shade shader ({} bytes GLSL)...",
                    glsl_source.len()
                );

                let wgsl_source = match precompiled_wgsl {
                    Some(src) => src.to_string(),
                    None => match glsl_to_wgsl(&glsl_source) {
                        Ok(wgsl) => wgsl,
                        Err(e) => {
                            for (i, line) in glsl_source.lines().enumerate() {
                                log::debug!("  {:>4}: {}", i + 1, line);
                            }
                            let error_msg = format!("{:#}", e);
                            let display_path = shade_path
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| pkg.config.meta.name.clone());
                            let paths = vec![display_path];
                            self.switch_to_fallback_error(
                                |p, w, h| fallback::render_load_error(&error_msg, p, w, h),
                                &paths,
                            )?;
                            info!(
                                "Shade '{}' shader failed to compile — showing fallback",
                                pkg.config.meta.name,
                            );
                            self.active_package = Some(pkg);
                            return Ok(ShadeLoadOutcome::CompileError(error_msg));
                        }
                    },
                };

                if precompiled_wgsl.is_none() {
                    info!(
                        "GLSL→WGSL translation successful ({} bytes WGSL)",
                        wgsl_source.len()
                    );
                }

                self.rebuild_pipeline_with_frag(&wgsl_source)?;
                self.current_frag_wgsl = wgsl_source;
            }
            None => {
                // No shader — pick a fallback based on available assets.
                // If there are textures (images, video, fonts), sample the first one.
                // Otherwise, show the "no shade" fallback via the error display.
                if !self.textures.is_empty() {
                    info!(
                        "Using texture sampler fallback ({} textures loaded)",
                        self.textures.len()
                    );
                    self.rebuild_pipeline_with_frag(IMAGE_SAMPLER_FRAG_WGSL)?;
                    self.current_frag_wgsl = IMAGE_SAMPLER_FRAG_WGSL.to_string();
                } else {
                    info!("No shader or textures in package — using no-shade fallback");
                    self.show_no_shade_fallback()?;
                }
            }
        }

        info!(
            "Shade package '{}' phase {:?} loaded successfully ({} textures)",
            pkg.config.meta.name,
            phase,
            self.textures.len()
        );
        self.active_package = Some(pkg);
        Ok(ShadeLoadOutcome::Success)
    }

    /// Hot-reload: takes raw Shadertoy-compatible GLSL, translates and loads it.
    pub fn load_glsl_source(&mut self, glsl_source: &str) -> Result<()> {
        info!(
            "Live reload: compiling {} bytes of GLSL...",
            glsl_source.len()
        );
        let wgsl_source =
            glsl_to_wgsl(glsl_source).context("Failed to translate GLSL shader to WGSL")?;
        // glsl_to_wgsl always injects CustomUniforms at set=0 binding=1.
        // Ensure the custom uniform buffer + BGL0 binding exists so the
        // pipeline layout matches the shader's expected bindings.
        if self.custom_uniform_buffer.is_none() {
            let device = self.gpu.device.as_ref().context("GPU not initialised")?;
            let buffer_size = (MAX_CUSTOM_UNIFORMS * std::mem::size_of::<f32>()) as u64;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("kroma-custom-uniforms"),
                size: buffer_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.custom_uniform_buffer = Some(buffer);
            self.rebuild_bind_group_0()?;
        } else if self.gpu.texture_bind_group_layout.is_some() {
            // Ensure pipeline layout includes texture bind groups if present
            self.rebuild_pipeline_layout()?;
        }
        self.rebuild_pipeline_with_frag(&wgsl_source)?;
        self.current_frag_wgsl = wgsl_source;
        info!("Live reload successful");
        Ok(())
    }

    /// Rebuild the render pipeline with a new fragment shader.
    fn rebuild_pipeline_with_frag(&mut self, frag_wgsl: &str) -> Result<()> {
        render_flow::rebuild_pipeline_with_frag(self, frag_wgsl)
    }

    /// Set a custom uniform value from the IPC command.
    pub fn set_custom_uniform(&mut self, name: &str, value: &kroma_shared::ipc::UniformValue) {
        use kroma_shared::ipc::UniformValue;

        self.custom_uniforms.insert(name.to_string(), value.clone());

        // Update the CPU-side data buffer at the mapped index
        if let Some(&idx) = self.custom_uniform_indices.get(name)
            && idx < MAX_CUSTOM_UNIFORMS
        {
            self.custom_uniform_data[idx] = match value {
                UniformValue::Float(v) => *v as f32,
                UniformValue::Bool(b) => {
                    if *b {
                        1.0
                    } else {
                        0.0
                    }
                }
                UniformValue::Int(i) => *i as f32,
            };
        }
        log::debug!("Custom uniform '{}' set to {:?}", name, value);
    }

    /// Initialize custom uniform buffer and mapping from a shade config.
    ///
    /// Called after loading a shade package. Maps uniform names from config
    /// to sequential indices in a storage buffer.
    pub fn init_custom_uniforms(&mut self, uniforms: &IndexMap<String, UniformDef>) -> Result<()> {
        let device = self.gpu.device.as_ref().context("GPU not initialised")?;

        // Map uniform names to indices
        self.custom_uniform_indices.clear();
        self.custom_uniform_data = vec![0.0; MAX_CUSTOM_UNIFORMS];

        for (idx, (name, def)) in uniforms.iter().enumerate() {
            if idx >= MAX_CUSTOM_UNIFORMS {
                warn!(
                    "Maximum {} custom uniform slots reached — ignoring '{}'",
                    MAX_CUSTOM_UNIFORMS, name
                );
                break;
            }

            // Set default value if provided
            if let Some(ref default) = def.default {
                self.custom_uniform_data[idx] = match default {
                    toml::Value::Float(v) => *v as f32,
                    toml::Value::Integer(v) => *v as f32,
                    toml::Value::Boolean(true) => 1.0,
                    toml::Value::Boolean(false) => 0.0,
                    _ => 0.0,
                };
            }

            self.custom_uniform_indices.insert(name.clone(), idx);
        }

        // Create the storage buffer
        let buffer_size = (MAX_CUSTOM_UNIFORMS * std::mem::size_of::<f32>()) as u64;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("kroma-custom-uniforms"),
            size: buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.custom_uniform_buffer = Some(buffer);

        // Rebuild bind group 0 to include both uniform buffer and storage buffer
        self.rebuild_bind_group_0()?;

        info!(
            "Custom uniforms initialized: {} slots mapped",
            self.custom_uniform_indices.len()
        );
        Ok(())
    }

    /// Rebuild bind group 0 to include both the main uniform buffer and custom uniform storage buffer.
    fn rebuild_bind_group_0(&mut self) -> Result<()> {
        let device = self.gpu.device.as_ref().context("GPU not initialised")?;
        let uniform_buf = self
            .gpu
            .uniform_buffer
            .as_ref()
            .context("Uniform buffer missing")?;

        let mut layout_entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }];

        let mut group_entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }];

        if let Some(ref custom_buf) = self.custom_uniform_buffer {
            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });
            group_entries.push(wgpu::BindGroupEntry {
                binding: 1,
                resource: custom_buf.as_entire_binding(),
            });
        }

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kroma-bgl"),
            entries: &layout_entries,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kroma-bg"),
            layout: &layout,
            entries: &group_entries,
        });

        self.gpu.bind_group_layout = Some(layout);
        self.gpu.bind_group = Some(bind_group);

        // Pipeline layout needs rebuilding since BGL changed
        self.rebuild_pipeline_layout()?;

        Ok(())
    }

    /// Upload custom uniform data to the GPU (called each frame).
    pub fn upload_custom_uniforms(&self) {
        if let (Some(buf), Some(queue)) = (&self.custom_uniform_buffer, self.gpu.queue.as_ref()) {
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&self.custom_uniform_data));
        }
    }

    // -------------------------------------------------------------------
    // Texture loading
    // -------------------------------------------------------------------

    /// Create a GPU texture from raw pixel data with a specified format.
    fn create_gpu_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        label: &str,
    ) -> LoadedTexture {
        let bpp = match format {
            wgpu::TextureFormat::Rgba8UnormSrgb => 4u32,
            wgpu::TextureFormat::R32Float => 4u32,
            _ => 4u32,
        };
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpp * width),
                rows_per_image: Some(height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{}_sampler", label)),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        LoadedTexture {
            texture,
            view,
            sampler,
            width,
            height,
            format,
        }
    }

    /// Load all textures referenced by a shade package's config.
    ///
    /// Creates [`TextureSource`] instances for each texture definition
    /// and corresponding GPU textures. The sources manage their own
    /// lifecycles — call [`update_textures`](Renderer::update_textures)
    /// each frame to advance them.
    ///
    /// Returns a (possibly empty) list of **required** textures that failed
    /// to load. Optional textures that fail get a 1×1 transparent placeholder
    /// so binding indices stay consistent.
    fn load_package_textures(
        &mut self,
        pkg: &Arc<LiveShadePackage>,
        textures: &IndexMap<String, TextureDef>,
    ) -> Result<Vec<TextureLoadFailure>> {
        self.lane_id = self.next_lane_id;
        self.next_lane_id += 1;
        self.shader_defs.clear();
        let device = self
            .gpu
            .device
            .as_ref()
            .context("GPU not initialised — cannot load textures")?;
        let queue = self
            .gpu
            .queue
            .as_ref()
            .context("GPU queue not initialised")?;

        // Build a GpuContext for texture sources that need GPU access
        // (shader textures).
        let surface_format = self.active_surface_format();
        let gpu_ctx = GpuContext {
            device: Arc::clone(device),
            queue: Arc::clone(queue),
            surface_format,
        };

        self.gpu.texture_bind_group = None;
        self.gpu.texture_bind_group_layout = None;

        let mut required_failures: Vec<TextureLoadFailure> = Vec::new();

        // Collect texture defs sorted by binding index
        let mut tex_defs: Vec<_> = textures.iter().collect();
        tex_defs.sort_by_key(|(_, def)| def.binding.unwrap_or(u32::MAX));

        for (name, def) in &tex_defs {
            // Create the texture source (all types including AudioSpectrum, Shader)
            match textures::create_texture_source(pkg, def, Some(&gpu_ctx)) {
                Ok(Some(mut source)) => {
                    if def.ty == kroma_shared::types::TextureType::Shader {
                        self.shader_defs.push((
                            self.textures.len(),
                            Arc::clone(pkg),
                            (*def).clone(),
                        ));
                    }
                    let (w, h) = source.dimensions();
                    let gpu_format = source.format().wgpu_format();

                    let mut init_width = 1;
                    let mut init_height = 1;
                    let mut init_data: Vec<u8> =
                        vec![0u8; source.format().bytes_per_pixel() as usize];
                    let mut recycle_init_data = false;

                    match source.update(0.0) {
                        Ok(TextureUpdate::NewFrame {
                            data,
                            width,
                            height,
                        }) if width > 0 && height > 0 => {
                            init_width = width;
                            init_height = height;
                            init_data = data;
                            recycle_init_data = true;
                        }
                        Ok(TextureUpdate::Unchanged)
                        | Ok(TextureUpdate::NewFrame {
                            data: _,
                            width: _,
                            height: _,
                        }) => {}
                        Err(e) => {
                            warn!(
                                "Texture '{}' initial frame prefetch failed (using placeholder): {}",
                                name, e
                            );
                        }
                    }

                    let initial_texture = Self::create_gpu_texture(
                        device,
                        queue,
                        &init_data,
                        init_width,
                        init_height,
                        gpu_format,
                        name,
                    );
                    if recycle_init_data {
                        source.recycle_frame(init_data);
                    }
                    self.textures.push(initial_texture);
                    self.texture_sources.push(Some(source));
                    info!(
                        "Texture '{}' ({}) queued ({}x{})",
                        name,
                        def.ty.as_str(),
                        w,
                        h
                    );
                }
                Ok(None) => {}
                Err(e) => {
                    let source_path = def.source.as_deref().unwrap_or("<unknown>").to_string();

                    if def.optional {
                        // Optional texture: push a transparent 1×1 RGBA placeholder
                        // so binding indices remain consistent.  Image and Video
                        // optional failures are already handled inside
                        // `create_texture_source` (they return a degraded source
                        // with a hot-reload watcher), so this branch only fires
                        // for other types (Font, Slideshow, AudioSpectrum).
                        warn!(
                            "Optional texture '{}' failed to load (using placeholder): {}",
                            name, e
                        );
                        let placeholder_data = vec![0u8; 4]; // transparent black
                        let placeholder = Self::create_gpu_texture(
                            device,
                            queue,
                            &placeholder_data,
                            1,
                            1,
                            wgpu::TextureFormat::Rgba8UnormSrgb,
                            name,
                        );
                        self.textures.push(placeholder);
                        self.texture_sources.push(None);
                    } else {
                        // Required texture: record the failure.
                        warn!("Required texture '{}' failed to load: {}", name, e);
                        required_failures.push(TextureLoadFailure {
                            name: name.to_string(),
                            source: source_path,
                            error: format!("{:#}", e),
                        });
                    }
                }
            }
        }

        // Build bind group if we have textures
        if !self.textures.is_empty() {
            self.build_texture_bind_group()?;
        }

        Ok(required_failures)
    }

    /// Advance all texture sources and upload changed frames to the GPU.
    ///
    /// Called once per frame from the render loop.
    fn update_active_lane_textures(&mut self, dt: f64) -> Result<bool> {
        let queue = match self.gpu.queue.as_ref() {
            Some(q) => q,
            None => return Ok(false),
        };
        let device = match self.gpu.device.as_ref() {
            Some(d) => d,
            None => return Ok(false),
        };

        let mut needs_rebuild = false;
        let mut changed = false;

        for (i, source_opt) in self.texture_sources.iter_mut().enumerate() {
            if self.shader_defs.iter().any(|(index, _, _)| *index == i) {
                continue;
            }
            let Some(source) = source_opt.as_mut() else {
                continue;
            };

            let was_gpu_managed = source.is_gpu_managed();

            // Propagate system uniforms to GPU-managed sources (shader textures)
            if was_gpu_managed {
                source.update_uniforms(&self.uniforms);
            }

            match source.update(dt)? {
                TextureUpdate::Unchanged => {}
                TextureUpdate::NewFrame {
                    data,
                    width,
                    height,
                } => {
                    changed = true;
                    // GPU-managed sources handle their own textures.  Keep the
                    // owned frame buffer available for recycling after any CPU
                    // upload path has borrowed from it.
                    let mut recycle_data = Some(data);
                    if source.is_gpu_managed() {
                        // GPU-managed sources have already updated their own
                        // texture. A NewFrame here means their exposed
                        // view/sampler may have changed, so rebuild bind group.
                        if i < self.textures.len() {
                            self.textures[i].width = width;
                            self.textures[i].height = height;
                        }
                        needs_rebuild = true;
                    } else {
                        if i < self.textures.len() {
                            let data = recycle_data.as_ref().expect("frame data present");
                            let tex_format = self.textures[i].format;
                            let bpp = match tex_format {
                                wgpu::TextureFormat::R32Float => 4u32,
                                _ => 4u32, // Rgba8UnormSrgb
                            };

                            // Check if we need to resize the GPU texture
                            if self.textures[i].width != width || self.textures[i].height != height
                            {
                                // Recreate the GPU texture at the new size
                                let new_tex = Self::create_gpu_texture(
                                    device,
                                    queue,
                                    data,
                                    width,
                                    height,
                                    tex_format,
                                    &format!("texture-{}", i),
                                );
                                self.textures[i] = new_tex;
                                needs_rebuild = true;
                            } else {
                                // Just upload new data to the existing texture
                                queue.write_texture(
                                    wgpu::TexelCopyTextureInfo {
                                        texture: &self.textures[i].texture,
                                        mip_level: 0,
                                        origin: wgpu::Origin3d::ZERO,
                                        aspect: wgpu::TextureAspect::All,
                                    },
                                    data,
                                    wgpu::TexelCopyBufferLayout {
                                        offset: 0,
                                        bytes_per_row: Some(bpp * width),
                                        rows_per_image: Some(height),
                                    },
                                    wgpu::Extent3d {
                                        width,
                                        height,
                                        depth_or_array_layers: 1,
                                    },
                                );
                            }
                        }
                    }

                    if let Some(data) = recycle_data.take() {
                        source.recycle_frame(data);
                    }
                }
            }

            let is_gpu_managed = source.is_gpu_managed();

            // GPU-managed sources render their own offscreen targets
            if is_gpu_managed {
                source.gpu_render()?;
            }

            if was_gpu_managed != is_gpu_managed {
                needs_rebuild = true;
            }
        }

        if needs_rebuild {
            self.build_texture_bind_group()?;
        }

        Ok(changed || needs_rebuild)
    }

    pub fn update_textures(&mut self, dt: f64) -> Result<bool> {
        let mut changed = self.update_active_lane_textures(dt)?;

        if self.transition_state.is_some() {
            let mut transition = self
                .transition_state
                .take()
                .context("Transition state disappeared while updating textures")?;

            let outgoing_lane = self.take_active_lane();
            self.install_active_lane(transition.incoming_lane);
            changed |= self.update_active_lane_textures(dt)?;
            transition.incoming_lane = self.take_active_lane();
            self.install_active_lane(outgoing_lane);

            self.transition_state = Some(transition);
        }

        Ok(changed)
    }

    /// Build the bind group layout and bind group for loaded textures.
    fn build_texture_bind_group(&mut self) -> Result<()> {
        let device = self.gpu.device.as_ref().context("GPU not initialised")?;
        let num_textures = self.textures.len();

        // Build layout entries: each texture gets (texture_view, sampler) pair of bindings
        let mut layout_entries = Vec::new();
        let mut group_entries = Vec::new();

        for i in 0..num_textures {
            let tex_binding = (i * 2) as u32;
            let samp_binding = (i * 2 + 1) as u32;

            // R32Float textures use filterable: true (matches prior audio spectrum behaviour).
            // If this fails on some hardware, switch R32Float to filterable: false + NonFiltering.
            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: tex_binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: samp_binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kroma-texture-bgl"),
            entries: &layout_entries,
        });

        // Now build the actual bind group entries referencing our textures.
        // For GPU-managed sources (shader textures), use their native
        // texture views and samplers directly instead of the LoadedTexture.
        for i in 0..num_textures {
            let tex_binding = (i * 2) as u32;
            let samp_binding = (i * 2 + 1) as u32;

            let (view, sampler) = if let Some(Some(source)) = self.texture_sources.get(i) {
                if source.is_gpu_managed() {
                    if let (Some(v), Some(s)) = (source.gpu_texture_view(), source.gpu_sampler()) {
                        (v as &wgpu::TextureView, s as &wgpu::Sampler)
                    } else {
                        (&self.textures[i].view, &self.textures[i].sampler)
                    }
                } else {
                    (&self.textures[i].view, &self.textures[i].sampler)
                }
            } else {
                (&self.textures[i].view, &self.textures[i].sampler)
            };

            group_entries.push(wgpu::BindGroupEntry {
                binding: tex_binding,
                resource: wgpu::BindingResource::TextureView(view),
            });
            group_entries.push(wgpu::BindGroupEntry {
                binding: samp_binding,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kroma-texture-bg"),
            layout: &layout,
            entries: &group_entries,
        });

        self.gpu.texture_bind_group_layout = Some(layout);
        self.gpu.texture_bind_group = Some(bind_group);

        let entry_count = group_entries.len();
        drop(group_entries);

        // Rebuild pipeline layout to include texture bind group
        self.rebuild_pipeline_layout()?;

        info!(
            "Texture bind group built ({} textures, {} entries)",
            num_textures, entry_count
        );
        Ok(())
    }

    /// Switch the renderer to a fullscreen error-image fallback.
    ///
    /// `render_fn` generates an `Rgba8Unorm` text bitmap given `(paths, width, height)`.
    /// The R channel encodes the text region type (title / subtitle / path);
    /// the GPU shader handles all visual rendering (background, colors, shadow).
    fn switch_to_fallback_error<F>(&mut self, render_fn: F, paths: &[String]) -> Result<()>
    where
        F: FnOnce(&[String], u32, u32) -> Vec<u8>,
    {
        let device = self.gpu.device.as_ref().context("GPU not initialised")?;
        let queue = self
            .gpu
            .queue
            .as_ref()
            .context("GPU queue not initialised")?;

        let (width, height) = self.surface_dimensions();

        let text_bitmap = render_fn(paths, width, height);

        // Retire any per-monitor shader histories belonging to the previous lane.
        self.lane_id = self.next_lane_id;
        self.next_lane_id += 1;
        self.shader_defs.clear();
        // Replace all textures with the single text-bitmap texture.
        self.textures.clear();
        self.texture_sources.clear();

        // Use Rgba8Unorm (linear) so the shader reads exact region codes
        // without sRGB gamma decoding.
        let error_tex = Self::create_gpu_texture(
            device,
            queue,
            &text_bitmap,
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
            "fallback-error-text",
        );
        self.textures.push(error_tex);
        self.texture_sources.push(None);

        // Ensure BGL0 exists (uniform buffer + optional custom uniform buffer).
        if self.gpu.bind_group_layout.is_none() {
            self.rebuild_bind_group_0()?;
        }

        // Rebuild texture bind group for the single text-bitmap texture.
        self.build_texture_bind_group()?;

        // Use the dedicated error fallback shader (GPU-rendered background + text).
        self.rebuild_pipeline_with_frag(FALLBACK_ERROR_FRAG_WGSL)?;
        self.current_frag_wgsl = FALLBACK_ERROR_FRAG_WGSL.to_string();

        info!(
            "Switched to GPU fallback error display ({}x{})",
            width, height
        );
        Ok(())
    }

    /// Show the "no shade loaded" fallback via the same font8x8 mechanism.
    ///
    /// Called on startup when no shade is configured, or when a package has
    /// no shader and no textures.
    pub fn show_no_shade_fallback(&mut self) -> Result<()> {
        self.switch_to_fallback_error(|_paths, w, h| fallback::render_no_shade(w, h), &[])
    }

    /// Show a load-error fallback when a shade package fails to open or load.
    ///
    /// `path` is the shade file that was requested, `details` is the error.
    pub fn switch_to_load_error(&mut self, path: &str, details: &str) -> Result<()> {
        let detail_str = details.to_string();
        self.switch_to_fallback_error(
            move |paths, w, h| fallback::render_load_error(&detail_str, paths, w, h),
            &[path.to_string()],
        )
    }

    /// Get the audio level from the first audio texture source, if any.
    pub fn get_audio_level(&self) -> f32 {
        for source_opt in &self.texture_sources {
            if let Some(source) = source_opt.as_ref() {
                let level = source.audio_level();
                if level > 0.0 || source.texture_type() == "audio_spectrum" {
                    return level;
                }
            }
        }
        0.0
    }

    // -------------------------------------------------------------------
    // Render frame
    // -------------------------------------------------------------------

    /// Rebuild the pipeline layout to include both uniform and texture bind groups.
    fn rebuild_pipeline_layout(&mut self) -> Result<()> {
        render_flow::rebuild_pipeline_layout(self)
    }

    /// True when no target can currently render, including an empty inventory.
    pub fn all_targets_occluded(&self, occluded: &[MonitorId]) -> bool {
        !self
            .gpu
            .targets
            .values()
            .any(|target| target_ready(target, occluded, std::time::Instant::now()))
    }

    /// Polls asynchronous work without waiting, even while rendering is paused.
    pub fn poll_gpu(&self) {
        if let Some(device) = &self.gpu.device {
            let _ = device.poll(wgpu::wgt::PollType::Poll);
        }
    }

    /// Drops GPU surfaces before the backend's native handles are released.
    pub fn clear_targets(&mut self) {
        self.gpu.targets.clear();
    }

    /// Reconciles native handles and sizes, including recovery from zero targets.
    pub fn sync_targets(
        &mut self,
        provider: &dyn SurfaceProvider,
        monitors: &[MonitorConfig],
    ) -> bool {
        let (Some(device), Some(instance), Some(adapter), Some(vert)) = (
            self.gpu.device.as_ref(),
            self.gpu.instance.as_ref(),
            self.gpu.adapter.as_ref(),
            self.gpu.vert_module.as_ref(),
        ) else {
            return false;
        };
        let size = self.surface_dimensions();
        let mut changed = false;
        self.gpu.targets.retain(|name, target| {
            let keep = monitors.iter().any(|m| {
                m.name == *name
                    && m.id == target.id
                    && (provider.is_headless()
                        || provider.create_surface(m.id).ok() == target.window)
            });
            changed |= !keep;
            keep
        });
        for monitor in monitors {
            let Ok((width, height)) = provider.size(monitor.id) else {
                continue;
            };
            if width == 0
                || height == 0
                || width > device.limits().max_texture_dimension_2d
                || height > device.limits().max_texture_dimension_2d
            {
                continue;
            }
            if let Some(target) = self.gpu.targets.get_mut(&monitor.name) {
                if let (Some(surface), Some(config)) = (&target.surface, &mut target.config) {
                    if target.needs_reconfigure || config.width != width || config.height != height
                    {
                        changed = true;
                        config.width = width;
                        config.height = height;
                        surface.configure(device, config);
                        target.needs_reconfigure = false;
                    }
                    continue;
                }
                if provider.is_headless() {
                    continue;
                }
            }
            let created = (|| -> Result<MonitorTarget> {
                let (window, surface, config) = if provider.is_headless() {
                    (None, None, None)
                } else {
                    let window = provider.create_surface(monitor.id)?;
                    // SAFETY: providers retain retired windows until sync_targets returns;
                    // Daemon drops its renderer before its backend on every exit path.
                    let surface = unsafe {
                        instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                            raw_display_handle: self.gpu.raw_display,
                            raw_window_handle: window,
                        })
                    }?;
                    let config = compatible_surface_config(
                        &surface.get_capabilities(adapter),
                        SURFACE_FORMAT,
                        width,
                        height,
                    )
                    .context("Monitor is incompatible with the selected GPU")?;
                    surface.configure(device, &config);
                    (Some(window), Some(surface), Some(config))
                };
                let output = render_flow::create_output_capture_state(
                    device,
                    vert,
                    SURFACE_FORMAT,
                    size.0,
                    size.1,
                );
                let blit = config.as_ref().map(|config| {
                    Self::create_fullscreen_pipeline(
                        device,
                        &output.blit_pipeline_layout,
                        vert,
                        &output.blit_fragment,
                        config.format,
                        "kroma-monitor-present",
                    )
                });
                let globals = std::array::from_fn(|_| {
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("kroma-monitor-globals"),
                        size: std::mem::size_of::<ShaderUniforms>() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    })
                });
                Ok(MonitorTarget {
                    id: monitor.id,
                    window,
                    surface,
                    config,
                    blit,
                    output,
                    globals,
                    transition: None,
                    shader_sources: HashMap::new(),
                    needs_reconfigure: false,
                    retry_after: std::time::Instant::now(),
                })
            })();
            match created {
                Ok(target) => {
                    self.gpu.targets.insert(monitor.name.clone(), target);
                    changed = true;
                }
                Err(error) => warn!("Cannot initialize monitor {}: {error:#}", monitor.name),
            }
        }
        changed
    }

    /// Renders each visible monitor and returns completed captures with their monitor names.
    pub fn render_frame(
        &mut self,
        capture_lighting: &[String],
        occluded: &[MonitorId],
    ) -> Result<Vec<(String, LightingFrame)>> {
        render_flow::render_frame(self, capture_lighting, occluded)
    }
}

/// Translate GLSL fragment shader source to WGSL.
///
/// Uses **shaderc** (the reference Vulkan GLSL compiler) for GLSL → SPIR-V,
/// then **naga** for SPIR-V → WGSL.  This is far more robust than naga's
/// own GLSL frontend, which cannot handle many real-world Shadertoy patterns
/// (mat2-from-vec4, struct arrays, preprocessor macros, etc.).
pub fn glsl_to_wgsl(glsl_source: &str) -> Result<String> {
    shader_translation::glsl_to_wgsl(glsl_source)
}

/// Tiny helper to block on an async wgpu future (wgpu's async is usually
/// instant on native backends).
fn pollster_block<F: std::future::Future>(f: F) -> F::Output {
    shader_translation::pollster_block(f)
}

#[cfg(test)]
mod monitor_tests {
    use super::*;
    use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

    struct OffscreenMonitors(Vec<MonitorConfig>);

    impl SurfaceProvider for OffscreenMonitors {
        fn connect(&mut self) -> Result<()> {
            Ok(())
        }
        fn size(&self, id: MonitorId) -> Result<(u32, u32)> {
            self.0
                .iter()
                .find(|m| m.id == id)
                .map(|m| (m.width, m.height))
                .context("Unknown output")
        }
        fn display_handle(&self) -> Result<RawDisplayHandle> {
            anyhow::bail!("No display")
        }
        fn create_surface(&self, _: MonitorId) -> Result<RawWindowHandle> {
            anyhow::bail!("No window")
        }
        fn list_monitors(&self) -> Result<Vec<MonitorConfig>> {
            Ok(self.0.clone())
        }
        fn dispatch(&mut self) -> Result<()> {
            Ok(())
        }
        fn is_headless(&self) -> bool {
            true
        }
    }

    fn monitor(id: u32, name: &str, width: u32) -> MonitorConfig {
        MonitorConfig {
            id: MonitorId(id),
            name: name.into(),
            width,
            height: 32,
            x: 0,
            y: 0,
            scale: 1.0,
        }
    }

    #[test]
    fn unsupported_surface_capabilities_fail_without_panicking() {
        assert!(
            compatible_surface_config(
                &wgpu::SurfaceCapabilities {
                    formats: vec![],
                    present_modes: vec![],
                    alpha_modes: vec![],
                    usages: wgpu::TextureUsages::empty(),
                },
                SURFACE_FORMAT,
                100,
                100
            )
            .is_none()
        );
        let caps = wgpu::SurfaceCapabilities {
            formats: vec![wgpu::TextureFormat::Rgba8Unorm],
            present_modes: vec![wgpu::PresentMode::Mailbox],
            alpha_modes: vec![wgpu::CompositeAlphaMode::PostMultiplied],
            usages: wgpu::TextureUsages::RENDER_ATTACHMENT,
        };
        let config = compatible_surface_config(&caps, SURFACE_FORMAT, 100, 100).unwrap();
        assert_eq!(config.format, caps.formats[0]);
        assert_eq!(config.present_mode, caps.present_modes[0]);
        assert_eq!(config.alpha_mode, caps.alpha_modes[0]);
        assert!(compatible_surface_config(&caps, SURFACE_FORMAT, 0, 100).is_none());
    }

    #[test]
    fn gpu_targets_hotplug_and_transition_uniforms_are_independent() {
        let mut provider =
            OffscreenMonitors(vec![monitor(7, "left", 64), monitor(42, "right", 96)]);
        let mut renderer = Renderer::new().unwrap();
        if let Err(error) = renderer.init_gpu_with_surface(&GpuPower::Low, &provider) {
            if error.to_string().contains("No GPU adapter") {
                eprintln!("Skipping GPU regression: no Vulkan adapter available");
                return;
            }
            panic!("GPU initialization failed: {error:#}");
        }
        assert!(!renderer.all_targets_occluded(&[MonitorId(7)]));
        assert!(renderer.all_targets_occluded(&[MonitorId(7), MonitorId(42)]));
        assert!(!renderer.all_targets_occluded(&[MonitorId(999)]));
        let left = renderer.gpu.targets["left"].output._final_texture.clone();
        assert_ne!(left, renderer.gpu.targets["right"].output._final_texture);
        assert_ne!(
            renderer.gpu.targets["left"].globals[0],
            renderer.gpu.targets["right"].globals[0]
        );
        provider.0.remove(1);
        renderer.sync_targets(&provider, &provider.0);
        assert_eq!(renderer.gpu.targets["left"].output._final_texture, left);
        provider.0.clear();
        renderer.sync_targets(&provider, &provider.0);
        assert!(renderer.gpu.targets.is_empty());
        assert!(renderer.all_targets_occluded(&[]));
        provider.0 = vec![monitor(99, "left", 128), monitor(101, "right", 96)];
        renderer.sync_targets(&provider, &provider.0);
        assert_eq!(renderer.gpu.targets.len(), 2);
        assert_eq!(renderer.surface_dimensions(), (64, 32));
        assert_ne!(renderer.gpu.targets["left"].output._final_texture, left);

        let shader = |color: &str| {
            format!(
                r#"
            struct Globals {{ time: f32, delta: f32, frame: u32, pad: u32 }};
            @group(0) @binding(0) var<uniform> globals: Globals;
            @fragment fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {{ return {color}; }}
        "#
            )
        };
        renderer
            .rebuild_pipeline_with_frag(&shader("vec4<f32>(globals.time, 0.0, 0.0, 1.0)"))
            .unwrap();
        let device = renderer.gpu.device.as_ref().unwrap().clone();
        let incoming_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(
                shader("vec4<f32>(0.0, globals.time, 0.0, 1.0)").into(),
            ),
        });
        let mut incoming = RendererLane::blank();
        incoming.pipeline = Some(Renderer::create_fullscreen_pipeline(
            &device,
            renderer.gpu.pipeline_layout.as_ref().unwrap(),
            renderer.gpu.vert_module.as_ref().unwrap(),
            &incoming_module,
            SURFACE_FORMAT,
            "test-incoming",
        ));
        incoming.bind_group_layout = renderer.gpu.bind_group_layout.clone();
        renderer.begin_transition_internal("test", r#"
            @group(1) @binding(0) var prev: texture_2d<f32>;
            @group(1) @binding(1) var prev_sampler: sampler;
            @group(1) @binding(2) var next: texture_2d<f32>;
            @group(1) @binding(3) var next_sampler: sampler;
            @fragment fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
                return textureSample(prev, prev_sampler, uv) + textureSample(next, next_sampler, uv);
            }
        "#, incoming, 64, 32).unwrap();
        renderer.transition_lane_uniforms = Some(TransitionLaneUniformState {
            outgoing_elapsed: 0.2,
            outgoing_frame: 1,
            incoming_elapsed: 0.8,
            incoming_frame: 2,
        });
        renderer.uniforms.u_time = 0.5;
        renderer
            .render_frame(&["left".into(), "right".into()], &[MonitorId(101)])
            .unwrap();
        assert!(!renderer.gpu.targets["right"].output.mapping_pending);
        device
            .poll(wgpu::wgt::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .unwrap();
        let frames = renderer
            .render_frame(&[], &[MonitorId(99), MonitorId(101)])
            .unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, "left");
        let pixel = &frames[0].1.rgba()[..4];
        assert!((110..=140).contains(&pixel[0]), "Outgoing lane: {pixel:?}");
        assert!((220..=245).contains(&pixel[1]), "Incoming lane: {pixel:?}");

        // Feedback must advance only on the monitor being rendered.
        renderer.transition_state = None;
        renderer.transition_lane_uniforms = None;
        let config: kroma_shared::types::ShadeConfig = toml::from_str(
            r#"
            [meta]
            name = "feedback regression"
            author = "test"
            [states.active.textures.history]
            type = "shader"
            shader = "feedback.frag"
            width = 8
            height = 8
            [states.active.textures.history.textures.previous]
            type = "shader"
            input = "t-1"
        "#,
        )
        .unwrap();
        let mut package = LiveShadePackage::new_empty(config);
        package.add_asset(
            "feedback.frag".into(),
            br#"#version 450
layout(set = 1, binding = 0) uniform texture2D previous;
layout(set = 1, binding = 1) uniform sampler previous_sampler;
layout(location = 0) in vec2 uv;
layout(location = 0) out vec4 color;
void main() {
    color = vec4(texture(sampler2D(previous, previous_sampler), uv).r + 0.05, 0.0, 0.0, 1.0);
}
"#
            .to_vec(),
        );
        renderer
            .load_phase(Arc::new(package), ShadePhase::Active, None, None)
            .unwrap();
        renderer.render_frame(&[], &[]).unwrap();
        renderer.render_frame(&[], &[MonitorId(101)]).unwrap();
        renderer
            .render_frame(&["left".into(), "right".into()], &[])
            .unwrap();
        device
            .poll(wgpu::wgt::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .unwrap();
        let frames = renderer
            .render_frame(&[], &[MonitorId(99), MonitorId(101)])
            .unwrap();
        assert_eq!(frames.len(), 2);
        let red = |name: &str| frames.iter().find(|(n, _)| n == name).unwrap().1.rgba()[0];
        assert!(
            red("left") > red("right") + 10,
            "Feedback history leaked between monitors"
        );
        assert_ne!(
            renderer.gpu.targets["left"].shader_sources[&renderer.lane_id][0]
                .1
                .gpu_texture_view(),
            renderer.gpu.targets["right"].shader_sources[&renderer.lane_id][0]
                .1
                .gpu_texture_view()
        );
    }
}
