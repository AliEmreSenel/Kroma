//! wgpu render pipeline for Kroma.
//!
//! Manages the GPU device, shader module, uniform buffer, and frame
//! rendering. Renders to a real Wayland or X11 surface via wgpu.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use kroma_shared::traits::SurfaceProvider;
use log::{info, warn};

use kroma_shared::shade::LiveShadePackage;
use kroma_shared::types::{ShadePhase, ShaderUniforms, TextureDef, UniformDef};

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

/// The preferred surface texture format.
const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

/// Maximum number of custom uniform float slots.
const MAX_CUSTOM_UNIFORMS: usize = 32;

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
    texture_bind_group: wgpu::BindGroup,
    _prev_texture: wgpu::Texture,
    prev_view: wgpu::TextureView,
    _next_texture: wgpu::Texture,
    next_view: wgpu::TextureView,
    _width: u32,
    _height: u32,
    incoming_lane: RendererLane,
}

#[derive(Clone, Copy)]
struct TransitionLaneUniformState {
    outgoing_elapsed: f32,
    outgoing_frame: u32,
    incoming_elapsed: f32,
    incoming_frame: u32,
}

struct RendererLane {
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

/// GPU-bound resources owned by the renderer.
///
/// This struct groups low-level wgpu handles away from orchestration state,
/// which keeps [`Renderer`] easier to reason about.
#[derive(Default)]
pub(crate) struct GpuResources {
    instance: Option<wgpu::Instance>,
    surface: Option<wgpu::Surface<'static>>,
    device: Option<Arc<wgpu::Device>>,
    queue: Option<Arc<wgpu::Queue>>,
    pipeline: Option<wgpu::RenderPipeline>,
    uniform_buffer: Option<wgpu::Buffer>,
    bind_group: Option<wgpu::BindGroup>,
    bind_group_layout: Option<wgpu::BindGroupLayout>,
    pipeline_layout: Option<wgpu::PipelineLayout>,
    vert_module: Option<wgpu::ShaderModule>,
    surface_config: Option<wgpu::SurfaceConfiguration>,
    texture_bind_group: Option<wgpu::BindGroup>,
    texture_bind_group_layout: Option<wgpu::BindGroupLayout>,
}

/// Orchestrates rendering, shader loading, uniforms, and texture updates.
pub struct Renderer {
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

    pub fn surface_dimensions(&self) -> (u32, u32) {
        self.gpu
            .surface_config
            .as_ref()
            .map(|c| (c.width.max(1), c.height.max(1)))
            .unwrap_or((
                self.uniforms.u_resolution[0].max(1.0) as u32,
                self.uniforms.u_resolution[1].max(1.0) as u32,
            ))
    }

    fn take_active_lane(&mut self) -> RendererLane {
        RendererLane {
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
        let w = width.max(1);
        let h = height.max(1);

        let prev_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("kroma-transition-prev-target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let prev_view = prev_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let prev_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("kroma-transition-prev-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let next_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("kroma-transition-dynamic-target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let next_view = next_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let next_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("kroma-transition-next-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

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

        let transition_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kroma-transition-bg"),
            layout: &texture_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&prev_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&prev_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&next_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&next_sampler),
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

        self.transition_spec = Some(spec.to_string());
        self.transition_progress = 0.0;
        self.transition_state = Some(TransitionRenderState {
            _spec: spec.to_string(),
            pipeline,
            texture_bind_group: transition_bg,
            _prev_texture: prev_texture,
            prev_view,
            _next_texture: next_texture,
            next_view,
            _width: w,
            _height: h,
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

    /// Writes one uniform block into GPU uniform memory.
    fn write_uniform_buffer(&self, uniforms: &ShaderUniforms) {
        render_flow::write_uniform_buffer(self, uniforms)
    }

    pub fn init_gpu_with_surface(
        &mut self,
        gpu_pref: &GpuPower,
        surface: &dyn SurfaceProvider,
    ) -> Result<()> {
        let primary = surface
            .list_monitors()?
            .first()
            .cloned()
            .unwrap_or_else(|| kroma_shared::types::MonitorConfig {
                id: kroma_shared::types::MonitorId(0),
                name: "default".into(),
                width: 1920,
                height: 1080,
                x: 0,
                y: 0,
                scale: 1.0,
            });

        let (width, height) = surface
            .size(primary.id)
            .unwrap_or((primary.width, primary.height));

        info!("Initializing WGPU (Vulkan)... Headless detected.");

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let raw_display = surface.display_handle()?;
        let raw_window = surface.create_surface(primary.id)?;

        let surface_target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(raw_display),
            raw_window_handle: raw_window,
        };

        let wgpu_surface = unsafe { instance.create_surface_unsafe(surface_target) }.ok();

        let adapter = pollster_block(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: gpu_pref.into(),
            compatible_surface: wgpu_surface.as_ref(),
            force_fallback_adapter: false,
        }))
        .context("No GPU adapter found")?;

        let (device, queue) = pollster_block(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::FLOAT32_FILTERABLE,
            label: Some("kroma-device"),
            ..Default::default()
        }))
        .context("Failed to create GPU device")?;
        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let mut format = wgpu::TextureFormat::Rgba8UnormSrgb;

        if let Some(ref s) = wgpu_surface {
            let surface_caps = s.get_capabilities(&adapter);
            format = surface_caps
                .formats
                .iter()
                .find(|f| f.is_srgb())
                .copied()
                .unwrap_or(surface_caps.formats[0]);

            let surface_config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: width.max(1),
                height: height.max(1),
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            };
            s.configure(&device, &surface_config);
            self.gpu.surface_config = Some(surface_config);
        } else {
            self.gpu.surface_config = None;
        }
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
        self.gpu.surface = wgpu_surface;
        self.gpu.device = Some(device);
        self.gpu.queue = Some(queue);
        self.gpu.pipeline = Some(pipeline);
        self.gpu.uniform_buffer = Some(uniform_buffer);
        self.gpu.bind_group = Some(bind_group);
        self.gpu.bind_group_layout = Some(bind_group_layout);
        self.gpu.pipeline_layout = Some(pipeline_layout);
        self.gpu.vert_module = Some(vert_module);

        info!(
            "GPU pipeline initialised with real surface ({}x{})",
            width, height
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
                    toml::Value::Boolean(v) => {
                        if *v {
                            1.0
                        } else {
                            0.0
                        }
                    }
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
    fn update_active_lane_textures(&mut self, dt: f64) -> Result<()> {
        let queue = match self.gpu.queue.as_ref() {
            Some(q) => q,
            None => return Ok(()),
        };
        let device = match self.gpu.device.as_ref() {
            Some(d) => d,
            None => return Ok(()),
        };

        let mut needs_rebuild = false;

        for (i, source_opt) in self.texture_sources.iter_mut().enumerate() {
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

        Ok(())
    }

    pub fn update_textures(&mut self, dt: f64) -> Result<()> {
        self.update_active_lane_textures(dt)?;

        if self.transition_state.is_some() {
            let mut transition = self
                .transition_state
                .take()
                .context("Transition state disappeared while updating textures")?;

            let outgoing_lane = self.take_active_lane();
            self.install_active_lane(transition.incoming_lane);
            self.update_active_lane_textures(dt)?;
            transition.incoming_lane = self.take_active_lane();
            self.install_active_lane(outgoing_lane);

            self.transition_state = Some(transition);
        }

        Ok(())
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

        let (width, height) = self
            .gpu
            .surface_config
            .as_ref()
            .map(|c| (c.width, c.height))
            .unwrap_or((1920, 1080));

        let text_bitmap = render_fn(paths, width, height);

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

    /// Render a single frame to the surface.
    pub fn render_frame(&mut self) -> Result<()> {
        render_flow::render_frame(self)
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
