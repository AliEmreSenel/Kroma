//! wgpu render pipeline for Kroma.
//!
//! Manages the GPU device, shader module, uniform buffer, and frame
//! rendering. Renders to a real Wayland or X11 surface via wgpu.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use kroma_shared::traits::SurfaceProvider;
use log::{info, warn};

use kroma_shared::shade::LiveShadePackage;
use kroma_shared::types::ShaderUniforms;
use wgpu::wgt::PollType;

use crate::{
    config::GpuPower,
    fallback,
    textures::{self, TextureSource, TextureUpdate},
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

/// Outcome of [`RenderState::load_shade`].
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

    pub active_package: Option<Arc<LiveShadePackage>>,

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
    /// Self-contained texture sources (image, video, font, slideshow, audio).
    /// Parallel to `textures` — `texture_sources[i]` drives `textures[i]`.
    texture_sources: Vec<Option<Box<dyn TextureSource>>>,
    texture_bind_group: Option<wgpu::BindGroup>,
    texture_bind_group_layout: Option<wgpu::BindGroupLayout>,

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
            texture_sources: Vec::new(),
            texture_bind_group: None,
            texture_bind_group_layout: None,
            current_frag_wgsl: INIT_FRAG_WGSL.to_string(),
            buffer_passes: Vec::new(),
        })
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

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });

        let raw_display = surface.display_handle()?;
        let raw_window = surface.create_surface(primary.id)?;

        let surface_target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: raw_display,
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
            self.surface_config = Some(surface_config);
        } else {
            self.surface_config = None;
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
        self.surface = wgpu_surface;
        self.device = Some(device);
        self.queue = Some(queue);
        self.pipeline = Some(pipeline);
        self.uniform_buffer = Some(uniform_buffer);
        self.bind_group = Some(bind_group);
        self.bind_group_layout = Some(bind_group_layout);
        self.pipeline_layout = Some(pipeline_layout);
        self.vert_module = Some(vert_module);

        info!(
            "GPU pipeline initialised with real surface ({}x{})",
            width, height
        );
        Ok(())
    }

    /// Load a shade package into the pipeline.
    ///
    /// If the package has a GLSL shader, translates it to WGSL and builds the
    /// pipeline. If it has image/video assets, loads them as GPU textures.
    /// If there is no shader, uses either a texture sampler or a no-shade fallback.
    ///
    /// On texture or shader errors the renderer switches to a fallback error
    /// image (rendered via `font8x8`) instead of propagating the error.
    pub fn load_shade(
        &mut self,
        pkg: LiveShadePackage,
        shade_path: Option<&str>,
    ) -> Result<ShadeLoadOutcome> {
        let pkg = Arc::new(pkg);

        // Reset texture state
        self.textures.clear();
        self.texture_sources.clear();
        self.buffer_passes.clear();

        // Load all textures from package config
        let tex_failures = self.load_package_textures(&pkg)?;

        // If any *required* textures failed, switch to fallback error display.
        if !tex_failures.is_empty() {
            let paths: Vec<String> = tex_failures
                .iter()
                .map(|f| format!("{}: {} ({})", f.name, f.source, f.error))
                .collect();
            self.switch_to_fallback_error(
                fallback::render_texture_error,
                &paths,
            )?;
            info!(
                "Shade '{}' loaded with texture errors — showing fallback",
                pkg.config.meta.name,
            );
            self.active_package = Some(pkg);
            return Ok(ShadeLoadOutcome::TextureError(tex_failures));
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
                };

                info!(
                    "GLSL→WGSL translation successful ({} bytes WGSL)",
                    wgsl_source.len()
                );
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
            "Shade package '{}' loaded successfully ({} textures)",
            pkg.config.meta.name,
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
    /// lifecycles — call [`update_textures`](RenderState::update_textures)
    /// each frame to advance them.
    ///
    /// Returns a (possibly empty) list of **required** textures that failed
    /// to load. Optional textures that fail get a 1×1 transparent placeholder
    /// so binding indices stay consistent.
    fn load_package_textures(
        &mut self,
        pkg: &Arc<LiveShadePackage>,
    ) -> Result<Vec<TextureLoadFailure>> {
        let device = self
            .device
            .as_ref()
            .context("GPU not initialised — cannot load textures")?;
        let queue = self.queue.as_ref().context("GPU queue not initialised")?;

        self.texture_bind_group = None;
        self.texture_bind_group_layout = None;

        let mut required_failures: Vec<TextureLoadFailure> = Vec::new();

        // Collect texture defs sorted by binding index
        let mut tex_defs: Vec<_> = pkg.config.textures.iter().collect();
        tex_defs.sort_by_key(|(_, def)| def.binding.unwrap_or(u32::MAX));

        for (name, def) in &tex_defs {
            // Create the texture source (all types including AudioSpectrum)
            match textures::create_texture_source(pkg, def) {
                Ok(Some(source)) => {
                    let (w, h) = source.dimensions();
                    let gpu_format = source.format().wgpu_format();
                    // Create a 1×1 placeholder in the correct format
                    let placeholder_data: Vec<u8> =
                        vec![0u8; source.format().bytes_per_pixel() as usize];
                    let placeholder = Self::create_gpu_texture(
                        device,
                        queue,
                        &placeholder_data,
                        1,
                        1,
                        gpu_format,
                        name,
                    );
                    self.textures.push(placeholder);
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
                    let source_path = def
                        .source
                        .as_deref()
                        .unwrap_or("<unknown>")
                        .to_string();

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
                        warn!(
                            "Required texture '{}' failed to load: {}",
                            name, e
                        );
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
    pub fn update_textures(&mut self, dt: f64) -> Result<()> {
        let queue = match self.queue.as_ref() {
            Some(q) => q,
            None => return Ok(()),
        };
        let device = match self.device.as_ref() {
            Some(d) => d,
            None => return Ok(()),
        };

        let mut needs_rebuild = false;

        for (i, source_opt) in self.texture_sources.iter_mut().enumerate() {
            let Some(source) = source_opt.as_mut() else {
                continue;
            };
            match source.update(dt)? {
                TextureUpdate::Unchanged => {}
                TextureUpdate::NewFrame {
                    data,
                    width,
                    height,
                } => {
                    if i >= self.textures.len() {
                        continue;
                    }
                    let tex_format = self.textures[i].format;
                    let bpp = match tex_format {
                        wgpu::TextureFormat::R32Float => 4u32,
                        _ => 4u32, // Rgba8UnormSrgb
                    };

                    // Check if we need to resize the GPU texture
                    if self.textures[i].width != width || self.textures[i].height != height {
                        // Recreate the GPU texture at the new size
                        let new_tex = Self::create_gpu_texture(
                            device,
                            queue,
                            &data,
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
                            &data,
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
        }

        if needs_rebuild {
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

    /// Switch the renderer to a fullscreen error-image fallback.
    ///
    /// `render_fn` generates an `Rgba8Unorm` text bitmap given `(paths, width, height)`.
    /// The R channel encodes the text region type (title / subtitle / path);
    /// the GPU shader handles all visual rendering (background, colors, shadow).
    fn switch_to_fallback_error<F>(
        &mut self,
        render_fn: F,
        paths: &[String],
    ) -> Result<()>
    where
        F: FnOnce(&[String], u32, u32) -> Vec<u8>,
    {
        let device = self.device.as_ref().context("GPU not initialised")?;
        let queue = self.queue.as_ref().context("GPU queue not initialised")?;

        let (width, height) = self
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
        if self.bind_group_layout.is_none() {
            self.rebuild_bind_group_0()?;
        }

        // Rebuild texture bind group for the single text-bitmap texture.
        self.build_texture_bind_group()?;

        // Use the dedicated error fallback shader (GPU-rendered background + text).
        self.rebuild_pipeline_with_frag(FALLBACK_ERROR_FRAG_WGSL)?;
        self.current_frag_wgsl = FALLBACK_ERROR_FRAG_WGSL.to_string();

        info!("Switched to GPU fallback error display ({}x{})", width, height);
        Ok(())
    }

    /// Show the "no shade loaded" fallback via the same font8x8 mechanism.
    ///
    /// Called on startup when no shade is configured, or when a package has
    /// no shader and no textures.
    pub fn show_no_shade_fallback(&mut self) -> Result<()> {
        self.switch_to_fallback_error(
            |_paths, w, h| fallback::render_no_shade(w, h),
            &[],
        )
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

        Ok(())
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
