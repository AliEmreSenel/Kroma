//! Shader texture source — renders a GLSL shader to an offscreen texture.
//!
//! A shader texture runs its own fragment shader each frame, producing
//! RGBA8 pixel data that other shaders can sample. It supports the full
//! feature set of the root shader: custom uniforms, nested textures
//! (including other shader textures for recursive composition), and all
//! system uniforms (time, resolution, mouse, CPU, RAM, battery, audio).

use std::sync::Arc;

use anyhow::Result;
use indexmap::IndexMap;
use log::warn;

use kroma_shared::shade::LiveShadePackage;
use kroma_shared::types::{ShaderUniforms, TextureDef, UniformDef};

use super::{GpuContext, TextureFormat, TextureSource, TextureUpdate};

/// Maximum number of custom uniform float slots per shader texture.
const MAX_CUSTOM_UNIFORMS: usize = 32;

/// Vertex shader for fullscreen triangle (same as the main renderer).
const FULLSCREEN_VERT_WGSL: &str = include_str!("../shaders/fullscreen.vert.wgsl");

/// A shader rendered to an offscreen GPU texture.
///
/// Manages its own wgpu render pipeline, uniform buffer, custom uniforms,
/// sub-textures, and offscreen render target. The output texture view and
/// sampler are exposed for zero-copy use by the parent renderer's bind
/// group.
pub struct ShaderTexture {
    /// Shared GPU context.
    gpu: GpuContext,

    /// Output render target texture.
    render_texture: wgpu::Texture,
    /// View for rendering to the offscreen target.
    render_view: wgpu::TextureView,
    /// View for sampling the output (used by parent bind group).
    sample_view: wgpu::TextureView,
    /// Sampler for the output texture.
    sampler: wgpu::Sampler,

    /// Previous frame snapshot used by `input = "t-1"` channels.
    prev_frame_texture: wgpu::Texture,
    prev_frame_view: wgpu::TextureView,
    prev_frame_sampler: wgpu::Sampler,

    /// Output dimensions.
    width: u32,
    height: u32,

    // -- Pipeline state --
    pipeline: wgpu::RenderPipeline,

    // -- Bind group 0: uniforms --
    uniform_buffer: wgpu::Buffer,
    custom_uniform_buffer: wgpu::Buffer,
    bind_group_0: wgpu::BindGroup,

    // -- Bind group 1: sub-textures --
    sub_textures: Vec<SubTexture>,
    texture_bind_group: Option<wgpu::BindGroup>,

    // -- Uniform state --
    uniforms: ShaderUniforms,
    custom_uniform_data: Vec<f32>,
}

/// A loaded sub-texture within a shader texture.
struct SubTexture {
    /// The CPU-side texture source (manages lifecycle, produces frames).
    source: Option<Box<dyn TextureSource>>,
    /// GPU texture (for CPU-managed sources that upload pixel data).
    gpu_texture: Option<wgpu::Texture>,
    gpu_view: wgpu::TextureView,
    gpu_sampler: wgpu::Sampler,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    /// True when this channel should bind the shader's own previous frame.
    uses_prev_frame: bool,
}

impl ShaderTexture {
    /// Create a new shader texture from GLSL source.
    ///
    /// `sub_texture_defs` and `uniform_defs` provide the same configuration
    /// options as the root shader's `textures` and `uniforms` sections.
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        gpu: &GpuContext,
        pkg: Arc<LiveShadePackage>,
        glsl_source: &str,
        width: u32,
        height: u32,
        sub_texture_defs: &IndexMap<String, Box<TextureDef>>,
        uniform_defs: &IndexMap<String, UniformDef>,
        optional: bool,
    ) -> Result<Self> {
        let device = &gpu.device;
        let queue = &gpu.queue;

        let width = width.max(1);
        let height = height.max(1);

        // Use Rgba8UnormSrgb for the offscreen render target so shader
        // textures produce the same color space as the main surface.
        let output_format = wgpu::TextureFormat::Rgba8UnormSrgb;

        // -- Translate GLSL to WGSL --
        let wgsl_source = match crate::renderer::glsl_to_wgsl(glsl_source) {
            Ok(wgsl) => wgsl,
            Err(e) if optional => {
                warn!("Shader texture compile failed (optional, degrading): {}", e);
                return Self::create_placeholder(gpu, width, height, output_format);
            }
            Err(e) => return Err(e.context("Shader texture GLSL compilation failed")),
        };

        // -- Offscreen render target --
        let (render_texture, render_view, sample_view, sampler) =
            Self::create_render_target(device, width, height, output_format);
        let (prev_frame_texture, _prev_frame_render_view, prev_frame_view, prev_frame_sampler) =
            Self::create_render_target(device, width, height, output_format);

        // -- Uniform buffer (ShaderUniforms) --
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shader-tex-uniforms"),
            size: std::mem::size_of::<ShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // -- Custom uniform buffer --
        let buffer_size = (MAX_CUSTOM_UNIFORMS * std::mem::size_of::<f32>()) as u64;
        let custom_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shader-tex-custom-uniforms"),
            size: buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // -- Map custom uniform names to indices --
        let mut custom_uniform_data = vec![0.0f32; MAX_CUSTOM_UNIFORMS];
        for (idx, (_name, def)) in uniform_defs.iter().enumerate() {
            if idx >= MAX_CUSTOM_UNIFORMS {
                warn!(
                    "Shader texture: max {} custom uniforms reached, ignoring '{}'",
                    MAX_CUSTOM_UNIFORMS, _name
                );
                break;
            }
            if let Some(ref default) = def.default {
                custom_uniform_data[idx] = match default {
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
        }

        // -- Bind group 0 (uniforms + custom uniforms) --
        let bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader-tex-bgl0"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let bind_group_0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shader-tex-bg0"),
            layout: &bgl0,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: custom_uniform_buffer.as_entire_binding(),
                },
            ],
        });

        // -- Sub-textures --
        let mut sub_textures = Vec::new();
        let mut tex_bgl_entries = Vec::new();
        let mut tex_bg_entries_data: Vec<(u32, u32)> = Vec::new(); // (tex_binding, samp_binding)

        let mut sorted_defs: Vec<_> = sub_texture_defs.iter().collect();
        sorted_defs.sort_by_key(|(_, def)| def.binding.unwrap_or(u32::MAX));

        for (name, def) in &sorted_defs {
            if Self::is_t_minus_one_input(def) {
                sub_textures.push(Self::create_prev_frame_sub_texture(device, queue, name));
                continue;
            }

            match super::create_texture_source(&pkg, def, Some(gpu)) {
                Ok(Some(source)) => {
                    let sub = Self::create_sub_texture(device, queue, source, name);
                    sub_textures.push(sub);
                }
                Ok(None) => {}
                Err(e) => {
                    if def.optional {
                        warn!(
                            "Shader texture: optional sub-texture '{}' failed (placeholder): {}",
                            name, e
                        );
                        let sub = Self::create_placeholder_sub_texture(device, queue, name);
                        sub_textures.push(sub);
                    } else if optional {
                        warn!(
                            "Shader texture: sub-texture '{}' failed, whole shader optional: {}",
                            name, e
                        );
                        return Self::create_placeholder(gpu, width, height, output_format);
                    } else {
                        return Err(e.context(format!(
                            "Shader texture: required sub-texture '{}' failed to load",
                            name
                        )));
                    }
                }
            }
        }

        // -- Build texture bind group (group 1) --
        let texture_bind_group = if !sub_textures.is_empty() {
            for (i, _sub) in sub_textures.iter().enumerate() {
                let tex_binding = (i * 2) as u32;
                let samp_binding = (i * 2 + 1) as u32;
                tex_bgl_entries.push(wgpu::BindGroupLayoutEntry {
                    binding: tex_binding,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                });
                tex_bgl_entries.push(wgpu::BindGroupLayoutEntry {
                    binding: samp_binding,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                });
                tex_bg_entries_data.push((tex_binding, samp_binding));
            }

            let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("shader-tex-bgl1"),
                entries: &tex_bgl_entries,
            });

            let mut bg_entries = Vec::new();
            for (i, sub) in sub_textures.iter().enumerate() {
                let (tex_b, samp_b) = tex_bg_entries_data[i];
                let (view, sampler) = if sub.uses_prev_frame {
                    (&prev_frame_view, &prev_frame_sampler)
                } else {
                    (&sub.gpu_view, &sub.gpu_sampler)
                };
                bg_entries.push(wgpu::BindGroupEntry {
                    binding: tex_b,
                    resource: wgpu::BindingResource::TextureView(view),
                });
                bg_entries.push(wgpu::BindGroupEntry {
                    binding: samp_b,
                    resource: wgpu::BindingResource::Sampler(sampler),
                });
            }

            let tex_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shader-tex-bg1"),
                layout: &tex_bgl,
                entries: &bg_entries,
            });

            // Build pipeline layout with both bind groups
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("shader-tex-pl"),
                bind_group_layouts: &[Some(&bgl0), Some(&tex_bgl)],
                immediate_size: 0,
            });

            let pipeline =
                Self::create_pipeline(device, &pipeline_layout, &wgsl_source, output_format)?;

            // Store pipeline; we'll set it below
            // Actually, we need to return a struct with the pipeline from here
            // Let's restructure: build pipeline, then construct Self
            return Ok(Self {
                gpu: gpu.clone(),
                render_texture,
                render_view,
                sample_view,
                sampler,
                prev_frame_texture,
                prev_frame_view,
                prev_frame_sampler,
                width,
                height,
                pipeline,
                uniform_buffer,
                custom_uniform_buffer,
                bind_group_0,
                sub_textures,
                texture_bind_group: Some(tex_bg),
                uniforms: ShaderUniforms {
                    u_resolution: [width as f32, height as f32],
                    ..ShaderUniforms::default()
                },
                custom_uniform_data,
            });
        } else {
            None
        };

        // -- Pipeline layout (no sub-textures) --
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shader-tex-pl"),
            bind_group_layouts: &[Some(&bgl0)],
            immediate_size: 0,
        });

        let pipeline =
            Self::create_pipeline(device, &pipeline_layout, &wgsl_source, output_format)?;

        // -- Initial uniform upload --
        let mut initial_uniforms = ShaderUniforms::default();
        initial_uniforms.u_resolution = [width as f32, height as f32];

        Ok(Self {
            gpu: gpu.clone(),
            render_texture,
            render_view,
            sample_view,
            sampler,
            prev_frame_texture,
            prev_frame_view,
            prev_frame_sampler,
            width,
            height,
            pipeline,
            uniform_buffer,
            custom_uniform_buffer,
            bind_group_0,
            sub_textures,
            texture_bind_group,
            uniforms: initial_uniforms,
            custom_uniform_data,
        })
    }

    fn create_render_target(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> (
        wgpu::Texture,
        wgpu::TextureView,
        wgpu::TextureView,
        wgpu::Sampler,
    ) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shader-tex-rt"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let render_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("shader-tex-rt-render"),
            ..Default::default()
        });
        let sample_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("shader-tex-rt-sample"),
            ..Default::default()
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shader-tex-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        (texture, render_view, sample_view, sampler)
    }

    fn create_pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        frag_wgsl: &str,
        format: wgpu::TextureFormat,
    ) -> Result<wgpu::RenderPipeline> {
        let vert_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader-tex-vert"),
            source: wgpu::ShaderSource::Wgsl(FULLSCREEN_VERT_WGSL.into()),
        });

        let frag_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader-tex-frag"),
            source: wgpu::ShaderSource::Wgsl(frag_wgsl.into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shader-tex-pipeline"),
            layout: Some(layout),
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

        Ok(pipeline)
    }

    fn create_sub_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: Box<dyn TextureSource>,
        label: &str,
    ) -> SubTexture {
        if source.is_gpu_managed() {
            // GPU-managed sources provide views/samplers at bind-group rebuild time.
            let placeholder_data = vec![0u8; 4]; // 1x1 transparent
            let format = wgpu::TextureFormat::Rgba8UnormSrgb;
            let (gpu_texture, gpu_view, gpu_sampler) =
                Self::create_sub_gpu_texture(device, queue, &placeholder_data, 1, 1, format, label);

            SubTexture {
                source: Some(source),
                gpu_texture: Some(gpu_texture),
                gpu_view,
                gpu_sampler,
                // Tracks the currently allocated GPU texture size.
                width: 1,
                height: 1,
                format,
                uses_prev_frame: false,
            }
        } else {
            // CPU-managed source — create a GPU texture for uploading frames.
            let format = source.format().wgpu_format();
            let placeholder_data = vec![0u8; source.format().bytes_per_pixel() as usize];
            let (gpu_texture, gpu_view, gpu_sampler) =
                Self::create_sub_gpu_texture(device, queue, &placeholder_data, 1, 1, format, label);

            SubTexture {
                source: Some(source),
                gpu_texture: Some(gpu_texture),
                gpu_view,
                gpu_sampler,
                // Start at placeholder size so the first decoded frame always
                // triggers a proper resize before write_texture.
                width: 1,
                height: 1,
                format,
                uses_prev_frame: false,
            }
        }
    }

    fn create_prev_frame_sub_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
    ) -> SubTexture {
        let placeholder_data = vec![0u8; 4];
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let (gpu_texture, gpu_view, gpu_sampler) =
            Self::create_sub_gpu_texture(device, queue, &placeholder_data, 1, 1, format, label);

        SubTexture {
            source: None,
            gpu_texture: Some(gpu_texture),
            gpu_view,
            gpu_sampler,
            width: 1,
            height: 1,
            format,
            uses_prev_frame: true,
        }
    }

    fn create_placeholder_sub_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
    ) -> SubTexture {
        let placeholder_data = vec![0u8; 4];
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let (gpu_texture, gpu_view, gpu_sampler) =
            Self::create_sub_gpu_texture(device, queue, &placeholder_data, 1, 1, format, label);

        SubTexture {
            source: None,
            gpu_texture: Some(gpu_texture),
            gpu_view,
            gpu_sampler,
            width: 1,
            height: 1,
            format,
            uses_prev_frame: false,
        }
    }

    fn is_t_minus_one_input(def: &TextureDef) -> bool {
        def.input
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case("t-1"))
            .unwrap_or(false)
    }

    fn create_sub_gpu_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[u8],
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        label: &str,
    ) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
        let bpp: u32 = match format {
            wgpu::TextureFormat::R32Float => 4,
            _ => 4,
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

        (texture, view, sampler)
    }

    /// Create a placeholder shader texture (used when optional and load fails).
    fn create_placeholder(
        gpu: &GpuContext,
        width: u32,
        height: u32,
        output_format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let device = &gpu.device;

        let (render_texture, render_view, sample_view, sampler) =
            Self::create_render_target(device, width, height, output_format);
        let (prev_frame_texture, _prev_frame_render_view, prev_frame_view, prev_frame_sampler) =
            Self::create_render_target(device, width, height, output_format);

        // Minimal pass-through shader
        let placeholder_wgsl = r#"
@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}
"#;
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shader-tex-placeholder-uniforms"),
            size: std::mem::size_of::<ShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let buffer_size = (MAX_CUSTOM_UNIFORMS * std::mem::size_of::<f32>()) as u64;
        let custom_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shader-tex-placeholder-custom"),
            size: buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bgl0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader-tex-placeholder-bgl0"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let bind_group_0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shader-tex-placeholder-bg0"),
            layout: &bgl0,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: custom_uniform_buffer.as_entire_binding(),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shader-tex-placeholder-pl"),
            bind_group_layouts: &[Some(&bgl0)],
            immediate_size: 0,
        });

        let vert_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader-tex-placeholder-vert"),
            source: wgpu::ShaderSource::Wgsl(FULLSCREEN_VERT_WGSL.into()),
        });
        let frag_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader-tex-placeholder-frag"),
            source: wgpu::ShaderSource::Wgsl(placeholder_wgsl.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shader-tex-placeholder-pipeline"),
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
                    format: output_format,
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

        Ok(Self {
            gpu: gpu.clone(),
            render_texture,
            render_view,
            sample_view,
            sampler,
            prev_frame_texture,
            prev_frame_view,
            prev_frame_sampler,
            width,
            height,
            pipeline,
            uniform_buffer,
            custom_uniform_buffer,
            bind_group_0,
            sub_textures: Vec::new(),
            texture_bind_group: None,
            uniforms: ShaderUniforms::default(),
            custom_uniform_data: vec![0.0; MAX_CUSTOM_UNIFORMS],
        })
    }

    /// Rebuild the texture bind group after sub-texture GPU resources change.
    fn rebuild_texture_bind_group(&mut self) {
        if self.sub_textures.is_empty() {
            self.texture_bind_group = None;
            return;
        }

        let device = &self.gpu.device;

        let mut layout_entries = Vec::new();
        let mut group_entries = Vec::new();

        for (i, _sub) in self.sub_textures.iter().enumerate() {
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

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader-tex-bgl1-rebuild"),
            entries: &layout_entries,
        });

        for (i, sub) in self.sub_textures.iter().enumerate() {
            let tex_binding = (i * 2) as u32;
            let samp_binding = (i * 2 + 1) as u32;

            if sub.uses_prev_frame {
                group_entries.push(wgpu::BindGroupEntry {
                    binding: tex_binding,
                    resource: wgpu::BindingResource::TextureView(&self.prev_frame_view),
                });
                group_entries.push(wgpu::BindGroupEntry {
                    binding: samp_binding,
                    resource: wgpu::BindingResource::Sampler(&self.prev_frame_sampler),
                });
                continue;
            }

            // For GPU-managed sub-textures, use their native views
            let (view, sampler) = if let Some(ref source) = sub.source {
                if source.is_gpu_managed() {
                    if let (Some(v), Some(s)) = (source.gpu_texture_view(), source.gpu_sampler()) {
                        (v, s)
                    } else {
                        (&sub.gpu_view, &sub.gpu_sampler)
                    }
                } else {
                    (&sub.gpu_view, &sub.gpu_sampler)
                }
            } else {
                (&sub.gpu_view, &sub.gpu_sampler)
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

        self.texture_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shader-tex-bg1-rebuild"),
            layout: &layout,
            entries: &group_entries,
        }));
    }
}

impl TextureSource for ShaderTexture {
    fn update(&mut self, dt: f64) -> Result<TextureUpdate> {
        let queue = &self.gpu.queue;
        let device = &self.gpu.device;

        // Update time uniforms
        self.uniforms.u_time += dt as f32;
        self.uniforms.u_delta_time = dt as f32;
        self.uniforms.u_frame += 1;

        let mut needs_bind_rebuild = false;

        // Update sub-texture sources
        for (i, sub) in self.sub_textures.iter_mut().enumerate() {
            let Some(source) = sub.source.as_mut() else {
                continue;
            };

            let was_gpu_managed = source.is_gpu_managed();

            // Propagate uniforms to GPU-managed sub-textures (recursive)
            source.update_uniforms(&self.uniforms);

            match source.update(dt)? {
                TextureUpdate::Unchanged => {}
                TextureUpdate::NewFrame {
                    data,
                    width,
                    height,
                } => {
                    if source.is_gpu_managed() {
                        // GPU-managed sources handle their own rendering
                        continue;
                    }

                    let bpp = match sub.format {
                        wgpu::TextureFormat::R32Float => 4u32,
                        _ => 4u32,
                    };

                    if sub.width != width || sub.height != height {
                        // Recreate the sub-texture at the new size
                        let (new_tex, new_view, new_sampler) = Self::create_sub_gpu_texture(
                            device,
                            queue,
                            &data,
                            width,
                            height,
                            sub.format,
                            &format!("shader-sub-tex-{}", i),
                        );
                        sub.gpu_texture = Some(new_tex);
                        sub.gpu_view = new_view;
                        sub.gpu_sampler = new_sampler;
                        sub.width = width;
                        sub.height = height;
                        needs_bind_rebuild = true;
                    } else if let Some(ref tex) = sub.gpu_texture {
                        queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: tex,
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

            let is_gpu_managed = source.is_gpu_managed();
            if was_gpu_managed != is_gpu_managed {
                needs_bind_rebuild = true;
            }

            // GPU-managed sub-textures render themselves
            if is_gpu_managed {
                source.gpu_render()?;
            }
        }

        if needs_bind_rebuild {
            self.rebuild_texture_bind_group();
        }

        // We always report Unchanged because we manage our own GPU texture.
        // The parent renderer uses our gpu_texture_view() directly.
        Ok(TextureUpdate::Unchanged)
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn format(&self) -> TextureFormat {
        TextureFormat::Rgba8
    }

    fn texture_type(&self) -> &'static str {
        "shader"
    }

    fn is_gpu_managed(&self) -> bool {
        true
    }

    fn gpu_texture_view(&self) -> Option<&wgpu::TextureView> {
        Some(&self.sample_view)
    }

    fn gpu_sampler(&self) -> Option<&wgpu::Sampler> {
        Some(&self.sampler)
    }

    fn update_uniforms(&mut self, uniforms: &ShaderUniforms) {
        // Copy system uniforms from parent, preserving our resolution
        let res = self.uniforms.u_resolution;
        self.uniforms = *uniforms;
        self.uniforms.u_resolution = res;
    }

    fn gpu_render(&mut self) -> Result<()> {
        let queue = &self.gpu.queue;
        let device = &self.gpu.device;

        // Ensure resolution matches our target
        self.uniforms.u_resolution = [self.width as f32, self.height as f32];

        // Upload uniforms
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&self.uniforms));
        queue.write_buffer(
            &self.custom_uniform_buffer,
            0,
            bytemuck::cast_slice(&self.custom_uniform_data),
        );

        // Encode render pass
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("shader-tex-encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shader-tex-render-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.render_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
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

            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group_0, &[]);
            if let Some(ref tex_bg) = self.texture_bind_group {
                pass.set_bind_group(1, tex_bg, &[]);
            }
            pass.draw(0..3, 0..1);
        }

        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.render_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.prev_frame_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );

        queue.submit(std::iter::once(encoder.finish()));

        Ok(())
    }

    fn audio_level(&self) -> f32 {
        // Propagate audio level from sub-textures
        for sub in &self.sub_textures {
            if let Some(ref source) = sub.source {
                let level = source.audio_level();
                if level > 0.0 || source.texture_type() == "audio_spectrum" {
                    return level;
                }
            }
        }
        0.0
    }
}
