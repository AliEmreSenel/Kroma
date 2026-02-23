//! wgpu render pipeline for Kroma.
//!
//! Manages the GPU device, shader module, uniform buffer, and frame
//! rendering. Renders to a real Wayland or X11 surface via wgpu.

use std::collections::HashMap;
use std::ptr::NonNull;

use anyhow::{Context, Result};
use log::{info, warn};

use kroma_shared::shade::LiveShadePackage;
use kroma_shared::types::{ShaderUniforms, TextureDef};
use wgpu::wgt::PollType;
use wgpu::{FilterMode, MipmapFilterMode};

use crate::audio::SPECTRUM_BANDS;

/// Default fullscreen triangle vertex shader (WGSL).
///
/// Draws a full-screen triangle with UVs — the fragment shader does the rest.
const FULLSCREEN_VERT_WGSL: &str = r#"
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    // Full-screen triangle trick: 3 vertices, no buffer needed.
    var out: VertexOutput;
    let x = f32(i32(vertex_index) / 2) * 4.0 - 1.0;
    let y = f32(i32(vertex_index) % 2) * 4.0 - 1.0;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) / 2.0, 1.0 - (y + 1.0) / 2.0);
    return out;
}
"#;

/// Default fragment shader (WGSL) — gradient that reacts to time.
const DEFAULT_FRAG_WGSL: &str = r#"
struct Globals {
    u_time: f32,
    u_delta_time: f32,
    u_frame: u32,
    _pad0: u32,
    u_resolution: vec2<f32>,
    _pad1: vec2<f32>,
    u_mouse: vec4<f32>,
    u_cpu: f32,
    u_ram: f32,
    u_battery: f32,
    u_audio_level: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let t = globals.u_time;
    let r = 0.5 + 0.5 * sin(t + uv.x * 6.2831);
    let g = 0.5 + 0.5 * sin(t * 1.3 + uv.y * 6.2831);
    let b = 0.5 + 0.5 * sin(t * 0.7 + (uv.x + uv.y) * 3.1416);
    return vec4<f32>(r, g, b, 1.0);
}
"#;

/// Default fragment shader for image/video mode — just samples texture 0.
const IMAGE_SAMPLER_FRAG_WGSL: &str = r#"
struct Globals {
    u_time: f32,
    u_delta_time: f32,
    u_frame: u32,
    _pad0: u32,
    u_resolution: vec2<f32>,
    _pad1: vec2<f32>,
    u_mouse: vec4<f32>,
    u_cpu: f32,
    u_ram: f32,
    u_battery: f32,
    u_audio_level: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var t_texture0: texture_2d<f32>;
@group(1) @binding(1) var s_texture0: sampler;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(t_texture0, s_texture0, uv);
}
"#;

/// The preferred surface texture format.
const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

/// Maximum number of texture channels (beyond this we ignore extra ones).
const MAX_TEXTURE_SLOTS: usize = 16;

/// Maximum number of custom uniform float slots.
const MAX_CUSTOM_UNIFORMS: usize = 32;

/// A loaded GPU texture with its sampler.
struct LoadedTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    width: u32,
    height: u32,
}

/// Audio spectrum texture: 512×1 R32Float, updated each frame.
struct AudioSpectrumTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
}

/// Slideshow state: tracks which image is displayed and when to switch.
struct SlideshowState {
    /// Configuration for all slides.
    defs: Vec<TextureDef>,
    /// Display order indices.
    order: Vec<usize>,
    /// Current index into `order`.
    current: usize,
    timer: f64,
    interval: f64,
}

pub enum SlideshowEvent {
    None,
    SwappedToImage,
    SwappedToVideo { source: String },
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

/// Holds the entire wgpu render state.
pub struct RenderState {
    /// Current shader uniforms (CPU side).
    pub uniforms: ShaderUniforms,

    pub active_package: Option<LiveShadePackage>,

    /// Custom uniforms set via IPC (name → value).
    custom_uniforms: HashMap<String, kroma_shared::ipc::UniformValue>,
    /// Mapping from custom uniform name → index in the storage buffer.
    custom_uniform_indices: HashMap<String, usize>,
    /// CPU-side custom uniform data (uploaded to GPU each frame).
    custom_uniform_data: Vec<f32>,
    /// GPU storage buffer for custom uniform values.
    custom_uniform_buffer: Option<wgpu::Buffer>,

    // wgpu resources
    instance: Option<wgpu::Instance>,
    surface: Option<wgpu::Surface<'static>>,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    pipeline: Option<wgpu::RenderPipeline>,
    uniform_buffer: Option<wgpu::Buffer>,
    bind_group: Option<wgpu::BindGroup>,
    bind_group_layout: Option<wgpu::BindGroupLayout>,
    pipeline_layout: Option<wgpu::PipelineLayout>,
    vert_module: Option<wgpu::ShaderModule>,
    surface_config: Option<wgpu::SurfaceConfiguration>,

    // Texture resources (bind group 1)
    textures: Vec<LoadedTexture>,
    texture_bind_group: Option<wgpu::BindGroup>,
    texture_bind_group_layout: Option<wgpu::BindGroupLayout>,
    audio_spectrum: Option<AudioSpectrumTexture>,
    slideshow: Option<SlideshowState>,

    /// Current fragment shader source (WGSL).
    current_frag_wgsl: String,

    /// Multi-pass buffer states (Shadertoy-style Buffer A/B/C/D).
    buffer_passes: Vec<BufferPassState>,
}

impl RenderState {
    /// Create a new render state. Does NOT initialise the GPU yet.
    pub fn new() -> Result<Self> {
        Ok(Self {
            active_package: None,
            uniforms: ShaderUniforms::default(),
            custom_uniforms: std::collections::HashMap::new(),
            custom_uniform_indices: std::collections::HashMap::new(),
            custom_uniform_data: vec![0.0; MAX_CUSTOM_UNIFORMS],
            custom_uniform_buffer: None,
            instance: None,
            surface: None,
            device: None,
            queue: None,
            pipeline: None,
            uniform_buffer: None,
            bind_group: None,
            bind_group_layout: None,
            pipeline_layout: None,
            vert_module: None,
            surface_config: None,
            textures: Vec::new(),
            texture_bind_group: None,
            texture_bind_group_layout: None,
            audio_spectrum: None,
            slideshow: None,
            current_frag_wgsl: DEFAULT_FRAG_WGSL.to_string(),
            buffer_passes: Vec::new(),
        })
    }

    /// Initialise wgpu with a real Wayland surface for rendering.
    ///
    /// # Arguments
    /// * `display_ptr` - Raw `wl_display*` pointer
    /// * `surface_ptr` - Raw `wl_surface*` pointer
    /// * `width` / `height` - Surface dimensions
    ///
    /// # Safety
    /// The display and surface pointers must be valid Wayland objects.
    pub unsafe fn init_gpu_with_surface(
        &mut self,
        display_ptr: std::ptr::NonNull<std::ffi::c_void>,
        surface_ptr: std::ptr::NonNull<std::ffi::c_void>,
        width: u32,
        height: u32,
    ) -> Result<()> {
        info!("Creating wgpu instance (Vulkan backend)...");
        info!("  display_ptr = {:?}", display_ptr);
        info!("  surface_ptr = {:?}", surface_ptr);
        info!("  dimensions  = {}x{}", width, height);

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });

        // Create a wgpu surface from the raw Wayland handles
        let raw_display = raw_window_handle::RawDisplayHandle::Wayland(
            raw_window_handle::WaylandDisplayHandle::new(display_ptr),
        );
        let raw_window = raw_window_handle::RawWindowHandle::Wayland(
            raw_window_handle::WaylandWindowHandle::new(surface_ptr),
        );

        let surface_target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: raw_display,
            raw_window_handle: raw_window,
        };

        let surface = instance
            .create_surface_unsafe(surface_target)
            .context("Failed to create wgpu surface from Wayland handles")?;

        // Request adapter compatible with the surface
        let adapter = pollster_block(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("No GPU adapter compatible with the Wayland surface")?;

        let adapter_info = adapter.get_info();
        info!(
            "GPU adapter: {} ({:?})",
            adapter_info.name, adapter_info.backend
        );

        let (device, queue) = pollster_block(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("kroma-device"),
            ..Default::default()
        }))
        .context("Failed to create GPU device")?;

        // Determine the best surface format
        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(
                *surface_caps
                    .formats
                    .first()
                    .context("No supported surface formats found")?,
            );

        info!("Surface format: {:?}", format);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo, // Vsync
            alpha_mode: surface_caps
                .alpha_modes
                .iter()
                .find(|m| **m == wgpu::CompositeAlphaMode::Opaque)
                .copied()
                .unwrap_or_else(|| {
                    surface_caps
                        .alpha_modes
                        .first()
                        .copied()
                        .unwrap_or(wgpu::CompositeAlphaMode::Auto)
                }),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        surface.configure(&device, &surface_config);

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
            bind_group_layouts: &[&bind_group_layout],
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

        // Render pipeline
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("kroma-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vert_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &frag_module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        self.instance = Some(instance);
        self.surface = Some(surface);
        self.device = Some(device);
        self.queue = Some(queue);
        self.pipeline = Some(pipeline);
        self.uniform_buffer = Some(uniform_buffer);
        self.bind_group = Some(bind_group);
        self.bind_group_layout = Some(bind_group_layout);
        self.pipeline_layout = Some(pipeline_layout);
        self.vert_module = Some(vert_module);
        self.surface_config = Some(surface_config);

        info!(
            "GPU pipeline initialised with real surface ({}x{})",
            width, height
        );
        Ok(())
    }

    /// Initialise the wgpu device with an X11 window (for KDE X11, XFCE, etc.).
    ///
    /// Uses the Xlib display handle obtained by opening a parallel Xlib
    /// connection (x11rb is pure-Rust XCB, but wgpu's Vulkan backend needs
    /// either Xlib or Xcb display pointers).
    pub unsafe fn init_gpu_with_x11(
        &mut self,
        window_id: u32,
        screen_num: i32,
        width: u32,
        height: u32,
    ) -> Result<()> {
        info!("Creating wgpu instance for X11...");
        info!("  window_id   = 0x{:x}", window_id);
        info!("  screen_num  = {}", screen_num);
        info!("  dimensions  = {}x{}", width, height);

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });

        // Open a parallel Xlib connection for wgpu
        // We use dlopen to avoid a hard link dependency on libX11
        let libx11 = unsafe { libloading::Library::new("libX11.so.6") }
            .or_else(|_| unsafe { libloading::Library::new("libX11.so") })
            .context("Failed to load libX11 — is X11 installed?")?;
        let x_open_display: libloading::Symbol<
            unsafe extern "C" fn(*const std::ffi::c_char) -> *mut std::ffi::c_void,
        > = unsafe { libx11.get(b"XOpenDisplay") }.context("XOpenDisplay not found in libX11")?;
        let display_ptr = x_open_display(std::ptr::null());
        if display_ptr.is_null() {
            anyhow::bail!("Failed to open X11 display via Xlib");
        }
        let display_nn = NonNull::new(display_ptr).context("Xlib display pointer is null")?;
        // Keep libx11 alive for the lifetime of the process (leak it)
        std::mem::forget(libx11);

        let raw_display = raw_window_handle::RawDisplayHandle::Xlib(
            raw_window_handle::XlibDisplayHandle::new(Some(display_nn), screen_num),
        );
        let raw_window = raw_window_handle::RawWindowHandle::Xlib(
            raw_window_handle::XlibWindowHandle::new(window_id as std::ffi::c_ulong),
        );

        let surface_target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: raw_display,
            raw_window_handle: raw_window,
        };

        let surface = instance
            .create_surface_unsafe(surface_target)
            .context("Failed to create wgpu surface from X11 handles")?;

        // The rest mirrors the Wayland init
        let adapter = pollster_block(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("No GPU adapter compatible with the X11 surface")?;

        let adapter_info = adapter.get_info();
        info!(
            "GPU adapter (X11): {} ({:?})",
            adapter_info.name, adapter_info.backend
        );

        let (device, queue) = pollster_block(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("kroma-device"),
            ..Default::default()
        }))
        .context("Failed to create GPU device")?;

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(
                *surface_caps
                    .formats
                    .first()
                    .context("No supported surface formats found (X11)")?,
            );

        info!("Surface format (X11): {:?}", format);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: surface_caps
                .alpha_modes
                .iter()
                .find(|m| **m == wgpu::CompositeAlphaMode::Opaque)
                .copied()
                .unwrap_or_else(|| {
                    surface_caps
                        .alpha_modes
                        .first()
                        .copied()
                        .unwrap_or(wgpu::CompositeAlphaMode::Auto)
                }),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        surface.configure(&device, &surface_config);

        // Create uniform buffer + bind group + pipeline (shared code)
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("kroma-uniforms"),
            size: std::mem::size_of::<ShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("kroma-pl"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let vert_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kroma-vert"),
            source: wgpu::ShaderSource::Wgsl(FULLSCREEN_VERT_WGSL.into()),
        });

        let frag_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kroma-frag"),
            source: wgpu::ShaderSource::Wgsl(self.current_frag_wgsl.clone().into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("kroma-pipeline-x11"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vert_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &frag_module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        self.instance = Some(instance);
        self.surface = Some(surface);
        self.device = Some(device);
        self.queue = Some(queue);
        self.pipeline = Some(pipeline);
        self.uniform_buffer = Some(uniform_buffer);
        self.bind_group = Some(bind_group);
        self.bind_group_layout = Some(bind_group_layout);
        self.pipeline_layout = Some(pipeline_layout);
        self.vert_module = Some(vert_module);
        self.surface_config = Some(surface_config);

        info!(
            "GPU pipeline initialised with X11 surface ({}x{})",
            width, height
        );
        Ok(())
    }

    /// Initialise the wgpu device in headless mode (no surface).
    ///
    /// Used for testing or when no display is available.
    pub fn init_gpu_headless(&mut self) -> Result<()> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });

        let adapter = pollster_block(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            ..Default::default()
        }))
        .context("No suitable GPU adapter found")?;

        info!("GPU adapter (headless): {}", adapter.get_info().name);

        let (device, queue) = pollster_block(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("kroma-device-headless"),
            ..Default::default()
        }))
        .context("Failed to create GPU device")?;

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

        self.instance = Some(instance);
        self.device = Some(device);
        self.queue = Some(queue);
        self.uniform_buffer = Some(uniform_buffer);
        self.bind_group_layout = Some(bind_group_layout);
        self.bind_group = Some(bind_group);

        info!("GPU pipeline initialised (headless mode)");
        Ok(())
    }

    /// Load a shade package into the pipeline.
    ///
    /// If the package has a GLSL shader, translates it to WGSL and builds the
    /// pipeline. If it has image/video assets, loads them as GPU textures.
    /// If there is no shader (image/video mode), uses a default sampler shader.
    pub fn load_shade(&mut self, pkg: LiveShadePackage) -> Result<()> {
        // Reset slideshow state
        self.slideshow = None;
        self.textures.clear();
        // Clear any existing buffer passes from the previous shader
        self.buffer_passes.clear();

        let is_slide = pkg.config.slideshow.is_some();

        // Set up slideshow if configured (interval > 0) and we have 2+ textures.
        // Works regardless of mode — any package can cycle through its textures.
        if is_slide {
            let mut tex_defs: Vec<TextureDef> = pkg.config.textures.values().cloned().collect();
            tex_defs.sort_by(|a, b| a.source.cmp(&b.source));

            let config = pkg.config.slideshow.as_ref().unwrap();
            let count = tex_defs.len();
            let mut order: Vec<usize> = (0..count).collect();
            if config.shuffle {
                let seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as usize;
                for i in (1..count).rev() {
                    let j = (seed.wrapping_mul(i).wrapping_add(7)) % (i + 1);
                    order.swap(i, j);
                }
            }

            self.slideshow = Some(SlideshowState {
                defs: tex_defs,
                order,
                current: 0,
                timer: 0.0,
                interval: config.interval,
            });
            let device = self
                .device
                .as_ref()
                .context("GPU not initialised — cannot load textures")?;
            let queue = self.queue.as_ref().context("GPU queue not initialised")?;

            let placeholder = Self::create_texture_from_raw_rgba(
                device,
                queue,
                &[0, 0, 0, 255],
                1,
                1,
                "placeholde-slide",
            );
            self.textures.push(placeholder);
            self.load_package_textures(&pkg, false)?;

            info!(
                "Slideshow initialized: {} textures, {:.1}s interval",
                count, config.interval
            );
        } else {
            // Load all textures from package assets (images, fonts, etc.)
            self.load_package_textures(&pkg, true)?;
        }

        // Initialize custom uniforms (creates storage buffer + rebuilds BGL0).
        // Must happen BEFORE shader compilation so GLSL referencing binding=1 works.
        self.init_custom_uniforms(&pkg.config)?;

        match &pkg.shader_source {
            Some(glsl_source) => {
                info!(
                    "Compiling shade shader ({} bytes GLSL)...",
                    glsl_source.len()
                );

                let wgsl_source = match glsl_to_wgsl(glsl_source) {
                    Ok(wgsl) => wgsl,
                    Err(e) => {
                        for (i, line) in glsl_source.lines().enumerate() {
                            log::debug!("  {:>4}: {}", i + 1, line);
                        }
                        return Err(e.context("Failed to translate GLSL shader to WGSL"));
                    }
                };

                info!(
                    "GLSL→WGSL translation successful ({} bytes WGSL)",
                    wgsl_source.len()
                );
                self.rebuild_pipeline_with_frag(&wgsl_source)?;
                self.current_frag_wgsl = wgsl_source;
            }
            None => {
                // No shader — pick a sensible default based on what assets are available.
                // If there are textures (images, video, fonts), sample the first one.
                // Otherwise, show a gradient.
                let default_wgsl = if !self.textures.is_empty() || self.slideshow.is_some() {
                    info!(
                        "Using default texture sampler shader ({} textures loaded)",
                        self.textures.len()
                    );
                    IMAGE_SAMPLER_FRAG_WGSL.to_string()
                } else {
                    info!("No textures or shader — using default gradient");
                    DEFAULT_FRAG_WGSL.to_string()
                };
                self.rebuild_pipeline_with_frag(&default_wgsl)?;
                self.current_frag_wgsl = default_wgsl;
            }
        }

        info!(
            "Shade package '{}' loaded successfully ({} textures)",
            pkg.config.meta.name,
            self.textures.len()
        );
        self.active_package = Some(pkg);
        Ok(())
    }

    /// Load a raw WGSL fragment shader string (used for the default shader or testing).
    #[allow(dead_code)]
    pub fn load_wgsl_fragment(&mut self, wgsl: &str) -> Result<()> {
        self.rebuild_pipeline_with_frag(wgsl)?;
        self.current_frag_wgsl = wgsl.to_string();
        Ok(())
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
            let device = self.device.as_ref().context("GPU not initialised")?;
            let buffer_size = (MAX_CUSTOM_UNIFORMS * std::mem::size_of::<f32>()) as u64;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("kroma-custom-uniforms"),
                size: buffer_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.custom_uniform_buffer = Some(buffer);
            self.rebuild_bind_group_0()?;
        } else if self.texture_bind_group_layout.is_some() {
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
        let device = self.device.as_ref().context("GPU not initialised")?;
        let pipeline_layout = self
            .pipeline_layout
            .as_ref()
            .context("Pipeline layout not available")?;
        let vert_module = self
            .vert_module
            .as_ref()
            .context("Vertex shader not available")?;

        let format = self
            .surface_config
            .as_ref()
            .map(|c| c.format)
            .unwrap_or(SURFACE_FORMAT);

        let frag_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kroma-frag-custom"),
            source: wgpu::ShaderSource::Wgsl(frag_wgsl.into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("kroma-pipeline-custom"),
            layout: Some(pipeline_layout),
            vertex: wgpu::VertexState {
                module: vert_module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &frag_module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        self.pipeline = Some(pipeline);
        info!("Render pipeline rebuilt with new fragment shader");
        Ok(())
    }

    /// Set a custom uniform value from the IPC command.
    pub fn set_custom_uniform(&mut self, name: &str, value: &kroma_shared::ipc::UniformValue) {
        use kroma_shared::ipc::UniformValue;

        self.custom_uniforms.insert(name.to_string(), value.clone());

        // Update the CPU-side data buffer at the mapped index
        if let Some(&idx) = self.custom_uniform_indices.get(name) {
            if idx < MAX_CUSTOM_UNIFORMS {
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
        }
        log::debug!("Custom uniform '{}' set to {:?}", name, value);
    }

    /// Initialize custom uniform buffer and mapping from a shade config.
    ///
    /// Called after loading a shade package. Maps uniform names from config
    /// to sequential indices in a storage buffer.
    pub fn init_custom_uniforms(
        &mut self,
        config: &kroma_shared::types::ShadeConfig,
    ) -> Result<()> {
        let device = self.device.as_ref().context("GPU not initialised")?;

        // Map uniform names to indices
        self.custom_uniform_indices.clear();
        self.custom_uniform_data = vec![0.0; MAX_CUSTOM_UNIFORMS];

        for (idx, (name, def)) in config.uniforms.iter().enumerate() {
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
        let device = self.device.as_ref().context("GPU not initialised")?;
        let uniform_buf = self
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

        self.bind_group_layout = Some(layout);
        self.bind_group = Some(bind_group);

        // Pipeline layout needs rebuilding since BGL changed
        self.rebuild_pipeline_layout()?;

        Ok(())
    }

    /// Upload custom uniform data to the GPU (called each frame).
    pub fn upload_custom_uniforms(&self) {
        if let (Some(buf), Some(queue)) = (&self.custom_uniform_buffer, self.queue.as_ref()) {
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&self.custom_uniform_data));
        }
    }

    // -------------------------------------------------------------------
    // Texture loading
    // -------------------------------------------------------------------

    /// Decode image bytes (PNG/JPEG/WebP/GIF) into an RGBA8 wgpu texture.
    fn create_texture_from_bytes(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bytes: &[u8],
        label: &str,
        filter: &kroma_shared::types::TextureFilter,
        wrap: &kroma_shared::types::TextureWrap,
    ) -> Result<LoadedTexture> {
        let img = image::load_from_memory(bytes)
            .with_context(|| format!("Failed to decode image: {}", label))?;
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();

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
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let wgpu_filter = match filter {
            kroma_shared::types::TextureFilter::Linear => wgpu::FilterMode::Linear,
            kroma_shared::types::TextureFilter::Nearest => wgpu::FilterMode::Nearest,
        };
        let wgpu_wrap = match wrap {
            kroma_shared::types::TextureWrap::Repeat => wgpu::AddressMode::Repeat,
            kroma_shared::types::TextureWrap::Clamp => wgpu::AddressMode::ClampToEdge,
            kroma_shared::types::TextureWrap::Mirror => wgpu::AddressMode::MirrorRepeat,
        };

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{}_sampler", label)),
            address_mode_u: wgpu_wrap,
            address_mode_v: wgpu_wrap,
            address_mode_w: wgpu_wrap,
            mag_filter: wgpu_filter,
            min_filter: wgpu_filter,
            mipmap_filter: MipmapFilterMode::Linear,
            ..Default::default()
        });

        info!("Loaded texture '{}' ({}x{})", label, width, height);
        Ok(LoadedTexture {
            texture,
            view,
            sampler,
            width,
            height,
        })
    }

    /// Create a GPU texture from raw RGBA8 data (no image decoding needed).
    fn create_texture_from_raw_rgba(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rgba: &[u8],
        width: u32,
        height: u32,
        label: &str,
    ) -> LoadedTexture {
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
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
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
        }
    }

    fn load_texture(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        pkg: &LiveShadePackage,
        def: &TextureDef,
    ) -> Option<LoadedTexture> {
        if self.textures.len() >= MAX_TEXTURE_SLOTS {
            warn!(
                "Maximum {} texture slots reached — ignoring '{}'",
                MAX_TEXTURE_SLOTS, name
            );
            return None;
        }

        if def.ty == "audio_spectrum" {
            return None;
        }

        if def.ty == "video" {
            let placeholder =
                Self::create_texture_from_raw_rgba(device, queue, &[0, 0, 0, 255], 1, 1, name);
            return Some(placeholder);
        }

        let source = match &def.source {
            Some(s) => s.clone(),
            None => {
                warn!("Texture '{}' has no source — skipping", name);
                return None;
            }
        };

        if let Some(source) = &def.source {
            // Try reading from mmap
            if let Some(bytes) = pkg.read_asset(source) {
                match Self::create_texture_from_bytes(
                    device,
                    queue,
                    &bytes,
                    name,
                    &def.filter,
                    &def.wrap,
                ) {
                    Ok(tex) => return Some(tex),
                    Err(e) => warn!("Failed to decode image: {}", e),
                }
            } else {
                warn!(
                    "Asset '{}' not found in package for texture '{}'",
                    source, name
                );
            }
        } else {
            warn!(
                "Asset '{}' not found in package for texture '{}'",
                source, name
            );
        }
        None
    }

    /// Load all textures referenced by a shade package's config.
    fn load_package_textures(&mut self, pkg: &LiveShadePackage, load_texs: bool) -> Result<()> {
        let device = self
            .device
            .as_ref()
            .context("GPU not initialised — cannot load textures")?;
        let queue = self.queue.as_ref().context("GPU queue not initialised")?;

        self.audio_spectrum = None; // Important: Clear old spectrum
        self.texture_bind_group = None;
        self.texture_bind_group_layout = None;

        // Collect texture defs sorted by binding index
        let mut tex_defs: Vec<_> = pkg.config.textures.iter().collect();
        tex_defs.sort_by_key(|(_, def)| def.binding.unwrap_or(u32::MAX));

        if load_texs {
            for (name, def) in &tex_defs {
                if let Some(tex) = self.load_texture(device, queue, name, pkg, def) {
                    self.textures.push(tex)
                }
            }
        }

        // Load font atlas textures
        {
            use crate::font;
            for (name, font_def) in &pkg.config.fonts {
                if self.textures.len() >= MAX_TEXTURE_SLOTS {
                    warn!("Maximum texture slots reached — ignoring font '{}'", name);
                    break;
                }

                // Try reading from mmap
                if let Some(bytes) = pkg.read_asset(&font_def.source) {
                    match font::rasterize_font_atlas(&bytes, font_def.size) {
                        Ok(atlas) => {
                            // Upload the atlas RGBA texture
                            let tex = Self::create_texture_from_raw_rgba(
                                device,
                                queue,
                                &atlas.rgba_data,
                                atlas.width,
                                atlas.height,
                                &format!("font-{}", name),
                            );
                            self.textures.push(tex);
                            info!(
                                "Font atlas '{}' loaded as texture ({}x{})",
                                name, atlas.width, atlas.height
                            );
                        }
                        Err(e) => warn!("Failed to rasterize font '{}': {}", name, e),
                    }
                } else {
                    warn!(
                        "Font source '{}' not found in package for '{}'",
                        font_def.source, name
                    );
                }
            }
        }

        // 2. Initialize Audio Spectrum if requested
        if pkg
            .config
            .textures
            .iter()
            .any(|tex| tex.1.ty == "audio_spectrum")
        {
            self.create_audio_spectrum_texture()?;
        }

        // 3. Build Bind Group (If we have images OR audio)
        if !self.textures.is_empty() || self.audio_spectrum.is_some() {
            self.build_texture_bind_group()?;
        }

        Ok(())
    }

    /// Build the bind group layout and bind group for loaded textures.
    fn build_texture_bind_group(&mut self) -> Result<()> {
        let device = self.device.as_ref().context("GPU not initialised")?;
        let num_textures = self.textures.len();

        // Build layout entries: each texture gets (texture_view, sampler) pair of bindings
        let mut layout_entries = Vec::new();
        let mut group_entries = Vec::new();

        for i in 0..num_textures {
            let tex_binding = (i * 2) as u32;
            let samp_binding = (i * 2 + 1) as u32;

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

        // Add audio spectrum texture at the end if available
        if self.audio_spectrum.is_some() {
            let tex_binding = (num_textures * 2) as u32;
            let samp_binding = tex_binding + 1;

            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: tex_binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: samp_binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                count: None,
            });
        }

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kroma-texture-bgl"),
            entries: &layout_entries,
        });

        // Now build the actual bind group entries referencing our textures
        for i in 0..num_textures {
            let tex_binding = (i * 2) as u32;
            let samp_binding = (i * 2 + 1) as u32;
            group_entries.push(wgpu::BindGroupEntry {
                binding: tex_binding,
                resource: wgpu::BindingResource::TextureView(&self.textures[i].view),
            });
            group_entries.push(wgpu::BindGroupEntry {
                binding: samp_binding,
                resource: wgpu::BindingResource::Sampler(&self.textures[i].sampler),
            });
        }

        if let Some(ref audio) = self.audio_spectrum {
            let tex_binding = (num_textures * 2) as u32;
            let samp_binding = tex_binding + 1;
            group_entries.push(wgpu::BindGroupEntry {
                binding: tex_binding,
                resource: wgpu::BindingResource::TextureView(&audio.view),
            });
            group_entries.push(wgpu::BindGroupEntry {
                binding: samp_binding,
                resource: wgpu::BindingResource::Sampler(&audio.sampler),
            });
        }

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kroma-texture-bg"),
            layout: &layout,
            entries: &group_entries,
        });

        self.texture_bind_group_layout = Some(layout);
        self.texture_bind_group = Some(bind_group);

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

    /// Create the audio spectrum texture (512×1 R32Float).
    pub fn create_audio_spectrum_texture(&mut self) -> Result<()> {
        let device = self.device.as_ref().context("GPU not initialised")?;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("kroma-audio-spectrum"),
            size: wgpu::Extent3d {
                width: 512,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("kroma-audio-spectrum-sampler"),
            mag_filter: FilterMode::Nearest,
            min_filter: FilterMode::Nearest,
            mipmap_filter: MipmapFilterMode::Nearest,
            ..Default::default()
        });

        self.audio_spectrum = Some(AudioSpectrumTexture {
            texture,
            view,
            sampler,
        });

        info!("Audio spectrum texture created (512x1 R32Float)");
        Ok(())
    }

    /// Upload audio spectrum data to the GPU texture.
    pub fn update_audio_spectrum(&mut self, spectrum: &[f32]) {
        if let (Some(audio), Some(queue)) = (&self.audio_spectrum, self.queue.as_ref()) {
            // Ensure exactly 512 values
            let mut padded = [0.0f32; SPECTRUM_BANDS];
            let len = spectrum.len().min(SPECTRUM_BANDS);
            padded[..len].copy_from_slice(&spectrum[..len]);

            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &audio.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&padded),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * 512),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 512,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    // -------------------------------------------------------------------
    // Slideshow
    // -------------------------------------------------------------------

    /// Swap the active display texture to a specific slideshow index.
    ///
    /// Rebuilds bind group so the GPU sees the new texture.
    fn swap_slideshow_texture(&mut self, def_idx: usize) -> Result<SlideshowEvent> {
        let Some(ref slideshow) = self.slideshow else {
            return Ok(SlideshowEvent::None);
        };
        // We need the package to read the asset
        let Some(ref pkg) = self.active_package else {
            return Ok(SlideshowEvent::None);
        };

        let def = slideshow.defs[def_idx].clone();

        // We need to create a new bind group pointing to this texture.
        // The simplest approach: override self.textures with a view into the slideshow.
        // Since we can't clone GPU textures, we rebuild the bind group directly.
        let device = match self.device.as_ref() {
            Some(d) => d,
            None => return Ok(SlideshowEvent::None),
        };

        let source = def.source.clone().unwrap_or_default();

        // [CHANGED] Read from mmap

        let Some(tex) = self.load_texture(device, self.queue.as_ref().unwrap(), &source, pkg, &def)
        else {
            return Ok(SlideshowEvent::None);
        };

        let mut layout_entries = vec![
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
        ];

        let mut group_entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&tex.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&tex.sampler),
            },
        ];

        // Preserve audio spectrum texture in the bind group if present
        if let Some(ref audio) = self.audio_spectrum {
            let tex_binding = 2u32;
            let samp_binding = 3u32;
            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: tex_binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
            layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: samp_binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                count: None,
            });
            group_entries.push(wgpu::BindGroupEntry {
                binding: tex_binding,
                resource: wgpu::BindingResource::TextureView(&audio.view),
            });
            group_entries.push(wgpu::BindGroupEntry {
                binding: samp_binding,
                resource: wgpu::BindingResource::Sampler(&audio.sampler),
            });
        }

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kroma-slideshow-bgl"),
            entries: &layout_entries,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kroma-slideshow-bg"),
            layout: &layout,
            entries: &group_entries,
        });

        self.texture_bind_group_layout = Some(layout);
        self.texture_bind_group = Some(bind_group);

        // Rebuild pipeline layout to include the new texture bind group
        if let Err(e) = self.rebuild_pipeline_layout() {
            warn!("Failed to rebuild pipeline layout for slideshow: {}", e);
        };

        if def.ty == "video" {
            Ok(SlideshowEvent::SwappedToVideo { source })
        } else {
            Ok(SlideshowEvent::SwappedToImage)
        }
    }

    /// Advance slideshow timer and switch textures when needed.
    ///
    /// Called once per frame from the render loop with the frame's delta time.
    pub fn update_slideshow(&mut self, dt: f64) -> Result<SlideshowEvent> {
        let should_advance = if let Some(ref mut slideshow) = self.slideshow {
            if slideshow.order.is_empty() {
                false
            } else {
                slideshow.timer += dt;
                if slideshow.timer >= slideshow.interval {
                    slideshow.timer -= slideshow.interval;
                    slideshow.current = (slideshow.current + 1) % slideshow.order.len();
                    true
                } else {
                    false
                }
            }
        } else {
            false
        };

        if let (true, Some(slideshow)) = (should_advance, &self.slideshow) {
            let idx = slideshow.current;
            let total = slideshow.defs.len();
            info!("Slideshow: advancing to image {} of {}", idx + 1, total);
            return self.swap_slideshow_texture(idx);
        };
        Ok(SlideshowEvent::None)
    }

    // -------------------------------------------------------------------
    // Video texture
    // -------------------------------------------------------------------

    /// Create or resize the video frame texture.
    ///
    /// Called when a video is first loaded or when the video dimensions change.
    pub fn create_video_texture(&mut self, width: u32, height: u32, index: usize) -> Result<()> {
        let device = self.device.as_ref().context("GPU not initialised")?;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("kroma-video-{}", index)),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("kroma-video-sampler-{}", index)),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let new_tex = LoadedTexture {
            texture,
            view,
            sampler,
            width,
            height,
        };

        // Put it in the main textures array (slot 0) and rebuild bind group
        if index < self.textures.len() {
            self.textures[index] = new_tex;
        } else {
            // Should not happen if placeholders were created correctly
            warn!(
                "Attempted to create video at index {} but only {} textures exist",
                index,
                self.textures.len()
            );
            self.textures.push(new_tex);
        }
        self.build_texture_bind_group()?;

        // Also store a reference dimension in video_texture
        // (we use self.textures[0] for the actual GPU resources)
        info!(
            "Video texture created at index {} ({}x{})",
            index, width, height
        );
        Ok(())
    }

    /// Upload a raw RGBA frame to the video texture.
    ///
    /// Called once per frame from the render loop when a video is playing.
    pub fn update_video_frame(&mut self, rgba_data: &[u8], width: u32, height: u32, index: usize) {
        // Check if we need to resize
        let needs_resize = self.textures.is_empty()
            || self.textures[index].width != width
            || self.textures[index].height != height;

        if needs_resize {
            if let Err(e) = self.create_video_texture(width, height, index) {
                warn!("Failed to create/resize video texture: {}", e);
                return;
            }
        }

        if let Some(queue) = self.queue.as_ref() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures[index].texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                rgba_data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * width),
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

    /// Rebuild the pipeline layout to include both uniform and texture bind groups.
    fn rebuild_pipeline_layout(&mut self) -> Result<()> {
        let device = self.device.as_ref().context("GPU not initialised")?;
        let bgl0 = self
            .bind_group_layout
            .as_ref()
            .context("Uniform BGL missing")?;

        let layouts: Vec<&wgpu::BindGroupLayout> =
            if let Some(ref tex_bgl) = self.texture_bind_group_layout {
                vec![bgl0, tex_bgl]
            } else {
                vec![bgl0]
            };

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("kroma-pl"),
            bind_group_layouts: &layouts,
            immediate_size: 0,
        });

        self.pipeline_layout = Some(pipeline_layout);

        // Only rebuild render pipeline if we have a vertex shader module
        // (headless mode may not have one yet)
        if self.vert_module.is_some() {
            self.rebuild_pipeline_with_frag(&self.current_frag_wgsl.clone())?;
        }

        Ok(())
    }

    /// Resize the render surface (e.g., after monitor reconfiguration).
    #[allow(dead_code)]
    pub fn resize(&mut self, width: u32, height: u32) {
        if let (Some(surface), Some(device), Some(config)) = (
            self.surface.as_ref(),
            self.device.as_ref(),
            self.surface_config.as_mut(),
        ) {
            config.width = width.max(1);
            config.height = height.max(1);
            surface.configure(device, config);
            self.uniforms.u_resolution = [width as f32, height as f32];
            info!("Surface resized to {}x{}", width, height);
        }
    }

    /// Render a single frame to the surface.
    pub fn render_frame(&mut self) -> Result<()> {
        let Some(queue) = self.queue.as_ref() else {
            return Ok(());
        };

        // Upload uniforms
        if let Some(buf) = self.uniform_buffer.as_ref() {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&self.uniforms));
        }

        // Upload custom uniforms
        self.upload_custom_uniforms();

        // Get the current surface texture to render to
        let Some(surface) = self.surface.as_ref() else {
            // No surface — headless mode, skip rendering
            return Ok(());
        };

        let frame = match surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                // Reconfigure the surface
                if let (Some(device), Some(config)) =
                    (self.device.as_ref(), self.surface_config.as_ref())
                {
                    surface.configure(device, config);
                }
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => {
                warn!("Surface timeout — skipping frame");
                return Ok(());
            }
            Err(e) => {
                return Err(anyhow::anyhow!("Surface error: {}", e));
            }
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let device = self
            .device
            .as_ref()
            .context("GPU not initialised — cannot render frame")?;
        let pipeline = self.pipeline.as_ref();
        let bind_group = self.bind_group.as_ref();

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("kroma-frame"),
        });

        // TODO: render buffer passes here (multi-pass Buffer A/B/C/D)

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("kroma-render-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if let (Some(pipeline), Some(bind_group)) = (pipeline, bind_group) {
                render_pass.set_pipeline(pipeline);
                render_pass.set_bind_group(0, bind_group, &[]);
                // Also bind textures if available (group 1)
                if let Some(ref tex_bg) = self.texture_bind_group {
                    render_pass.set_bind_group(1, tex_bg, &[]);
                }
                render_pass.draw(0..3, 0..1); // Fullscreen triangle
            }
        }

        queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        Ok(())
    }

    /// Render the current shader to an offscreen texture and return JPEG bytes.
    ///
    /// This is used for the live preview stream — renders at a small resolution
    /// and returns base64-encoded JPEG data.
    pub fn capture_preview_frame(&mut self, width: u32, height: u32) -> Result<Vec<u8>> {
        let device = self
            .device
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No GPU device"))?;
        let queue = self
            .queue
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No GPU queue"))?;
        let pipeline = self
            .pipeline
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No render pipeline"))?;
        let bind_group = self
            .bind_group
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No bind group"))?;

        let w = width.max(1);
        let h = height.max(1);

        // Use the same format as the active pipeline to avoid format mismatch
        let format = self
            .surface_config
            .as_ref()
            .map(|c| c.format)
            .unwrap_or(SURFACE_FORMAT);

        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview-capture"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let tex_view = tex.create_view(&wgpu::TextureViewDescriptor::default());

        // Bytes per row must be aligned to 256 for buffer copy
        let bytes_per_pixel = 4u32;
        let unpadded_bytes_per_row = w * bytes_per_pixel;
        let padded_bytes_per_row = (unpadded_bytes_per_row + 255) & !255;

        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview-readback"),
            size: (padded_bytes_per_row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Upload uniforms with preview resolution
        let mut preview_uniforms = self.uniforms;
        preview_uniforms.u_resolution = [w as f32, h as f32];
        if let Some(buf) = self.uniform_buffer.as_ref() {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&preview_uniforms));
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("preview-capture-encoder"),
        });

        // Render pass
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview-render-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &tex_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            if let Some(ref tex_bg) = self.texture_bind_group {
                pass.set_bind_group(1, tex_bg, &[]);
            }
            pass.draw(0..3, 0..1);
        }

        // Copy texture to buffer
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &output_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );

        queue.submit(std::iter::once(encoder.finish()));

        // Map the buffer and read back pixels
        let buffer_slice = output_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        let _ = device.poll(PollType::wait_indefinitely())?;

        rx.recv()
            .map_err(|_| anyhow::anyhow!("Buffer map channel closed"))?
            .map_err(|e| anyhow::anyhow!("Buffer map failed: {:?}", e))?;

        // Copy pixel data, removing row padding and converting BGRA → RGBA
        let data = buffer_slice.get_mapped_range();
        let mut rgba = Vec::with_capacity((w * h * bytes_per_pixel) as usize);
        let is_bgra = matches!(
            format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        for row in 0..h {
            let start = (row * padded_bytes_per_row) as usize;
            let end = start + unpadded_bytes_per_row as usize;
            let row_data = &data[start..end];
            for pixel in row_data.chunks_exact(4) {
                if is_bgra {
                    rgba.push(pixel[2]); // R (was B)
                    rgba.push(pixel[1]); // G
                    rgba.push(pixel[0]); // B (was R)
                } else {
                    rgba.push(pixel[0]); // R
                    rgba.push(pixel[1]); // G
                    rgba.push(pixel[2]); // B
                }
                rgba.push(pixel[3]); // A
            }
        }
        drop(data);
        output_buffer.unmap();

        // Restore original resolution in uniform buffer
        if let Some(buf) = self.uniform_buffer.as_ref() {
            queue.write_buffer(buf, 0, bytemuck::bytes_of(&self.uniforms));
        }

        // Encode as JPEG — convert RGBA to RGB first (JPEG doesn't support alpha)
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for pixel in rgba.chunks_exact(4) {
            rgb.push(pixel[0]); // R
            rgb.push(pixel[1]); // G
            rgb.push(pixel[2]); // B
        }
        let img = image::RgbImage::from_raw(w, h, rgb)
            .ok_or_else(|| anyhow::anyhow!("Failed to create image from pixels"))?;
        let mut jpeg_bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut jpeg_bytes);
        img.write_to(&mut cursor, image::ImageFormat::Jpeg)
            .context("JPEG encode failed")?;

        Ok(jpeg_bytes)
    }

    /// Check if the GPU has been initialised with a real surface.
    #[allow(dead_code)]
    pub fn has_surface(&self) -> bool {
        self.surface.is_some()
    }
}

/// Translate GLSL fragment shader source to WGSL.
///
/// Uses **shaderc** (the reference Vulkan GLSL compiler) for GLSL → SPIR-V,
/// then **naga** for SPIR-V → WGSL.  This is far more robust than naga's
/// own GLSL frontend, which cannot handle many real-world Shadertoy patterns
/// (mat2-from-vec4, struct arrays, preprocessor macros, etc.).
fn glsl_to_wgsl(glsl_source: &str) -> Result<String> {
    use naga::back::wgsl;
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    // Inject the custom uniform storage buffer declaration if not already present.
    // This lets hand-written .shade shaders reference custom_data[N] without
    // needing to include the declaration manually.
    let glsl_source = if !glsl_source.contains("CustomUniforms") {
        // Insert after the Globals uniform block if present, otherwise after #version
        if let Some(pos) = glsl_source.find("layout(location = 0) out vec4") {
            // Insert before the output declaration
            let (before, after) = glsl_source.split_at(pos);
            format!(
                "{}// Custom uniform storage buffer — access via custom_data[index]\nlayout(set = 0, binding = 1) readonly buffer CustomUniforms {{\n    float custom_data[32];\n}};\n\n{}",
                before, after
            )
        } else {
            // Fallback: prepend after #version line
            let mut lines = glsl_source.lines();
            let first_line = lines.next().unwrap_or("");
            if first_line.starts_with("#version") {
                format!(
                    "{}\n\n// Custom uniform storage buffer — access via custom_data[index]\nlayout(set = 0, binding = 1) readonly buffer CustomUniforms {{\n    float custom_data[32];\n}};\n\n{}",
                    first_line,
                    lines.collect::<Vec<_>>().join("\n")
                )
            } else {
                // No version directive — just prepend
                format!(
                    "// Custom uniform storage buffer — access via custom_data[index]\nlayout(set = 0, binding = 1) readonly buffer CustomUniforms {{\n    float custom_data[32];\n}};\n\n{}",
                    glsl_source
                )
            }
        }
    } else {
        glsl_source.to_string()
    };

    // --- Step 1: GLSL → SPIR-V via shaderc -----------------------------------
    let compiler = shaderc::Compiler::new()
        .map_err(|_| anyhow::anyhow!("Failed to create shaderc compiler"))?;
    let mut options = shaderc::CompileOptions::new()
        .map_err(|_| anyhow::anyhow!("Failed to create shaderc compile options"))?;
    options.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_0 as u32,
    );
    options.set_source_language(shaderc::SourceLanguage::GLSL);
    options.set_target_spirv(shaderc::SpirvVersion::V1_0);
    // Auto-set bindings for naga compatibility
    options.set_auto_bind_uniforms(false);

    let binary = compiler
        .compile_into_spirv(
            &glsl_source,
            shaderc::ShaderKind::Fragment,
            "shader.frag",
            "main",
            Some(&options),
        )
        .map_err(|e| {
            log::error!("shaderc GLSL compile error:\n{}", e);
            anyhow::anyhow!("shaderc GLSL compile error: {}", e)
        })?;

    if binary.get_num_warnings() > 0 {
        log::warn!("shaderc warnings:\n{}", binary.get_warning_messages());
    }

    let spirv_bytes = binary.as_binary();

    // --- Step 2: SPIR-V → naga Module ----------------------------------------
    let spv_options = naga::front::spv::Options {
        adjust_coordinate_space: false,
        strict_capabilities: false,
        block_ctx_dump_prefix: None,
    };

    let module = naga::front::spv::parse_u8_slice(bytemuck::cast_slice(spirv_bytes), &spv_options)
        .map_err(|e| {
            log::error!("SPIR-V parse error: {}", e);
            anyhow::anyhow!("SPIR-V parse error: {}", e)
        })?;

    // --- Step 3: Validate & write WGSL ---------------------------------------
    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
    let info = validator.validate(&module).map_err(|e| {
        log::error!("Shader validation error: {}", e);
        anyhow::anyhow!("Shader validation error: {}", e)
    })?;

    let mut wgsl_source = wgsl::write_string(&module, &info, wgsl::WriterFlags::empty())
        .map_err(|e| anyhow::anyhow!("WGSL write error: {}", e))?;

    // Rename the fragment entry point from "main" to "fs_main"
    // Only rename the entry point `fn main(`, not any helper function
    // containing "main" in its name. Replace just the first occurrence.
    if let Some(pos) = wgsl_source.find("fn main(") {
        wgsl_source.replace_range(pos..pos + 8, "fn fs_main(");
    }

    Ok(wgsl_source)
}

/// Tiny helper to block on an async wgpu future (wgpu's async is usually
/// instant on native backends).
fn pollster_block<F: std::future::Future>(f: F) -> F::Output {
    futures_lite_block_on(f)
}

/// Minimal single-threaded executor for wgpu futures.
fn futures_lite_block_on<F: std::future::Future>(f: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    fn raw_waker() -> RawWaker {
        fn no_op(_: *const ()) {}
        fn clone(_: *const ()) -> RawWaker {
            raw_waker()
        }
        let vtable = &RawWakerVTable::new(clone, no_op, no_op, no_op);
        RawWaker::new(std::ptr::null(), vtable)
    }

    let waker = unsafe { Waker::from_raw(raw_waker()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);

    loop {
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(val) => return val,
            Poll::Pending => {
                std::hint::spin_loop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glsl_to_wgsl_simple_fragment() {
        let glsl = r#"#version 450
layout(location = 0) out vec4 fragColor;

void main() {
    fragColor = vec4(1.0, 0.0, 0.0, 1.0);
}
"#;
        let result = glsl_to_wgsl(glsl);
        assert!(result.is_ok(), "GLSL→WGSL failed: {:?}", result.err());
        let wgsl = result.unwrap();
        assert!(!wgsl.is_empty());
    }
}
