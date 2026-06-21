//! Slideshow texture source.
//!
//! Cycles through a list of child texture sources on a timer. Each slide is
//! a full texture definition, so a slideshow can contain image/video/shader/
//! audio/font/slideshow entries. During transition windows, both current and
//! next child sources are kept live for blending.

use std::sync::Arc;

use anyhow::{Context, Result};
use log::{info, warn};

use kroma_shared::shade::LiveShadePackage;
use kroma_shared::transition::{TransitionScope, parse_transition_spec, resolve_transition_spec};
use kroma_shared::types::{ShaderUniforms, TextureDef};

use super::{GpuContext, TextureFormat, TextureSource, TextureUpdate};

const FULLSCREEN_VERT_WGSL: &str = include_str!("../shaders/fullscreen.vert.wgsl");

const TRANSITION_COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const TRANSITION_CUSTOM_UNIFORM_SLOTS: usize = 32;

struct UploadTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    width: u32,
    height: u32,
}

struct SlideshowTransitionRuntime {
    uniform_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    output_sampler: wgpu::Sampler,
    output_width: u32,
    output_height: u32,
    prev_upload: Option<UploadTexture>,
    next_upload: Option<UploadTexture>,
    uniforms: ShaderUniforms,
}

impl SlideshowTransitionRuntime {
    fn new(gpu: &GpuContext, wgsl_source: &str, width: u32, height: u32) -> Result<Self> {
        let device = &gpu.device;
        let queue = &gpu.queue;
        let w = width.max(1);
        let h = height.max(1);

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("slideshow-transition-uniforms"),
            size: std::mem::size_of::<ShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let custom_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("slideshow-transition-custom-uniforms"),
            size: (TRANSITION_CUSTOM_UNIFORM_SLOTS * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let zero_custom = [0.0f32; TRANSITION_CUSTOM_UNIFORM_SLOTS];
        queue.write_buffer(
            &custom_uniform_buffer,
            0,
            bytemuck::cast_slice(&zero_custom),
        );

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("slideshow-transition-globals-bgl"),
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

        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("slideshow-transition-globals-bg"),
            layout: &globals_layout,
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

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("slideshow-transition-texture-bgl"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("slideshow-transition-pipeline-layout"),
            bind_group_layouts: &[Some(&globals_layout), Some(&texture_bind_group_layout)],
            immediate_size: 0,
        });

        let vert_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("slideshow-transition-vert"),
            source: wgpu::ShaderSource::Wgsl(FULLSCREEN_VERT_WGSL.into()),
        });
        let frag_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("slideshow-transition-frag"),
            source: wgpu::ShaderSource::Wgsl(wgsl_source.into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("slideshow-transition-pipeline"),
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
                    format: TRANSITION_COLOR_FORMAT,
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

        let (output_texture, output_view) = Self::create_output_target(device, w, h);
        let output_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("slideshow-transition-output-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Ok(Self {
            uniform_buffer,
            globals_bind_group,
            texture_bind_group_layout,
            pipeline,
            output_texture,
            output_view,
            output_sampler,
            output_width: w,
            output_height: h,
            prev_upload: None,
            next_upload: None,
            uniforms: ShaderUniforms::default(),
        })
    }

    fn create_output_target(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("slideshow-transition-output"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TRANSITION_COLOR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    fn ensure_output_size(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let w = width.max(1);
        let h = height.max(1);
        if self.output_width == w && self.output_height == h {
            return;
        }
        let (tex, view) = Self::create_output_target(device, w, h);
        self.output_texture = tex;
        self.output_view = view;
        self.output_width = w;
        self.output_height = h;
    }

    fn write_uniforms(&self, queue: &wgpu::Queue) {
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(std::slice::from_ref(&self.uniforms)),
        );
    }
}

// ---------------------------------------------------------------------------
// Slide entry — lightweight metadata kept for every slide
// ---------------------------------------------------------------------------

/// Metadata for a single slide.
struct SlideEntry {
    def: TextureDef,
}

// ---------------------------------------------------------------------------
// Active child — the currently loaded texture
// ---------------------------------------------------------------------------

/// Load a slide source on demand from the shade package / filesystem.
fn load_slide_child(
    entry: &SlideEntry,
    pkg: &Arc<LiveShadePackage>,
    hot_reload: bool,
    gpu: Option<&GpuContext>,
) -> Result<Box<dyn TextureSource>> {
    let mut def = entry.def.clone();
    def.hot_reload = def.hot_reload || hot_reload;

    let source = super::create_texture_source(pkg, &def, gpu)
        .with_context(|| {
            format!(
                "Failed to create slideshow source of type '{}'",
                def.ty.as_str()
            )
        })?
        .context("Slideshow source did not produce a texture source")?;

    Ok(source)
}

// ---------------------------------------------------------------------------
// SlideshowTexture
// ---------------------------------------------------------------------------

/// A slideshow texture that cycles through child textures on a timer.
///
/// Normally only the currently-active child is loaded. During transition
/// windows, the upcoming child is loaded early and both children are updated
/// until the handoff point.
pub struct SlideshowTexture {
    /// The shade package (mmap-backed), used to read image data on demand.
    pkg: Arc<LiveShadePackage>,
    /// Lightweight metadata for every slide.
    entries: Vec<SlideEntry>,
    /// Index of the currently active slide.
    current: usize,
    /// The loaded child for the current slide.
    current_child: Option<Box<dyn TextureSource>>,
    /// The loaded child for the upcoming slide while transitioning.
    next_child: Option<Box<dyn TextureSource>>,
    /// Index of the upcoming slide while transitioning.
    next_index: Option<usize>,
    /// Most recent CPU frame from current child.
    current_frame_cache: Option<(Vec<u8>, u32, u32)>,
    /// Most recent CPU frame from next child.
    next_frame_cache: Option<(Vec<u8>, u32, u32)>,
    /// Timer accumulator (seconds).
    timer: f64,
    /// Seconds between slides.
    interval: f64,
    /// Optional transition spec string for slide switches.
    transition_spec: Option<String>,
    /// Effective transition duration for slide switches.
    transition_duration: Option<f64>,
    /// Whether this slideshow can currently blend frames.
    transition_blend_enabled: bool,
    /// Transition shader runtime for slideshow edges.
    transition_runtime: Option<SlideshowTransitionRuntime>,
    /// Whether external slide sources should auto-reload from filesystem changes.
    hot_reload: bool,
    /// Shared GPU context for slides that are GPU-managed (shader textures).
    gpu: Option<GpuContext>,
}

impl SlideshowTexture {
    fn update_cached_frame(
        child: &mut Box<dyn TextureSource>,
        cache: &mut Option<(Vec<u8>, u32, u32)>,
        dt: f64,
    ) -> Result<TextureUpdate> {
        let update = child.update(dt)?;
        if let TextureUpdate::NewFrame {
            data,
            width,
            height,
        } = &update
        {
            *cache = Some((data.clone(), *width, *height));
        }
        Ok(update)
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

    fn resolve_transition_wgsl(pkg: &LiveShadePackage, spec: &str) -> Result<(String, f64)> {
        let parsed = parse_transition_spec(spec)
            .with_context(|| format!("Invalid slideshow transition spec '{}'", spec))?;
        let builtins = crate::transitions::builtin_transition_defs();
        let resolved =
            resolve_transition_spec(&parsed, &builtins, None, Some(&pkg.config.transitions))
                .with_context(|| format!("Unresolved slideshow transition spec '{}'", spec))?;

        let wgsl = match resolved.scope {
            TransitionScope::Kroma => crate::transitions::builtin_transition_wgsl(resolved.id)
                .with_context(|| format!("Unknown builtin transition '{}'", resolved.id))?
                .to_string(),
            TransitionScope::Incoming => {
                let bytes = pkg
                    .read_asset(&resolved.definition.shader)
                    .with_context(|| {
                        format!(
                            "Missing transition shader asset '{}' for '{}'.",
                            resolved.definition.shader, spec
                        )
                    })?;
                let source = String::from_utf8(bytes).with_context(|| {
                    format!(
                        "Transition shader '{}' is not valid UTF-8",
                        resolved.definition.shader
                    )
                })?;
                if resolved.definition.shader.ends_with(".wgsl") {
                    source
                } else {
                    let preprocessed = Self::preprocess_transition_glsl(&source);
                    crate::renderer::glsl_to_wgsl(&preprocessed).with_context(|| {
                        format!(
                            "Failed to compile slideshow transition shader '{}'",
                            resolved.definition.shader
                        )
                    })?
                }
            }
            TransitionScope::Outgoing => {
                anyhow::bail!("Outgoing scope is unavailable for slideshow transitions")
            }
        };

        Ok((wgsl, resolved.duration))
    }

    fn ensure_upload_texture(
        upload: &mut Option<UploadTexture>,
        gpu: &GpuContext,
        data: &[u8],
        width: u32,
        height: u32,
        label: &str,
    ) {
        let device = &gpu.device;
        let queue = &gpu.queue;
        let w = width.max(1);
        let h = height.max(1);

        if upload.as_ref().map(|t| (t.width, t.height)) != Some((w, h)) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TRANSITION_COLOR_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(&format!("{}-sampler", label)),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            *upload = Some(UploadTexture {
                _texture: texture,
                view,
                sampler,
                width: w,
                height: h,
            });
        }

        if let Some(upload) = upload.as_ref() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &upload._texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * w),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    fn resolve_transition_input(
        source: Option<&mut Box<dyn TextureSource>>,
        cache: &Option<(Vec<u8>, u32, u32)>,
        upload: &mut Option<UploadTexture>,
        gpu: &GpuContext,
        label: &str,
    ) -> Option<(wgpu::TextureView, wgpu::Sampler)> {
        if let Some(source) = source
            && source.is_gpu_managed()
            && let (Some(v), Some(s)) = (source.gpu_texture_view(), source.gpu_sampler())
        {
            return Some((v.clone(), s.clone()));
        }

        let (data, width, height) = cache.as_ref()?;
        Self::ensure_upload_texture(upload, gpu, data, *width, *height, label);
        upload
            .as_ref()
            .map(|upload| (upload.view.clone(), upload.sampler.clone()))
    }

    fn render_transition_frame(&mut self, t: f32) -> Result<()> {
        let Some(runtime) = self.transition_runtime.as_mut() else {
            return Ok(());
        };
        let Some(gpu) = self.gpu.as_ref() else {
            return Ok(());
        };

        let prev_dims = self
            .current_child
            .as_ref()
            .map(|s| s.dimensions())
            .or_else(|| self.current_frame_cache.as_ref().map(|(_, w, h)| (*w, *h)));
        let next_dims = self
            .next_child
            .as_ref()
            .map(|s| s.dimensions())
            .or_else(|| self.next_frame_cache.as_ref().map(|(_, w, h)| (*w, *h)));
        let (pw, ph) = prev_dims.unwrap_or((1, 1));
        let (nw, nh) = next_dims.unwrap_or((1, 1));
        if pw != nw || ph != nh {
            return Ok(());
        }

        let prev = Self::resolve_transition_input(
            self.current_child.as_mut(),
            &self.current_frame_cache,
            &mut runtime.prev_upload,
            gpu,
            "slideshow-transition-prev-upload",
        );
        let next = Self::resolve_transition_input(
            self.next_child.as_mut(),
            &self.next_frame_cache,
            &mut runtime.next_upload,
            gpu,
            "slideshow-transition-next-upload",
        );
        let (prev_view, prev_sampler) = match prev {
            Some(v) => v,
            None => return Ok(()),
        };
        let (next_view, next_sampler) = match next {
            Some(v) => v,
            None => return Ok(()),
        };

        runtime.ensure_output_size(&gpu.device, pw, ph);
        runtime.uniforms.u_transition_t = t.clamp(0.0, 1.0);
        runtime.uniforms.u_resolution = [pw as f32, ph as f32];
        runtime.write_uniforms(&gpu.queue);

        let texture_bg = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("slideshow-transition-texture-bg"),
            layout: &runtime.texture_bind_group_layout,
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

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("slideshow-transition-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("slideshow-transition-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &runtime.output_view,
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
            pass.set_pipeline(&runtime.pipeline);
            pass.set_bind_group(0, &runtime.globals_bind_group, &[]);
            pass.set_bind_group(1, &texture_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        gpu.queue.submit(std::iter::once(encoder.finish()));
        Ok(())
    }

    /// Build a slideshow from a list of slide source definitions.
    ///
    /// Only resolves video sources (embedded stream or disk path). Image data
    /// is NOT read at this point — it is decompressed from the mmap on
    /// demand when the slide becomes active.
    ///
    /// When `optional` is `true` and construction fails (e.g. no valid
    /// sources, or the first child cannot be loaded), the texture degrades
    /// to an empty slideshow that emits [`TextureUpdate::Unchanged`].
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        pkg: Arc<LiveShadePackage>,
        sources: &[TextureDef],
        interval: f64,
        transition_spec: Option<&str>,
        shuffle: bool,
        hot_reload: bool,
        optional: bool,
        gpu: Option<&GpuContext>,
    ) -> Result<Self> {
        let pkg_fallback = if optional {
            Some(Arc::clone(&pkg))
        } else {
            None
        };
        let inner = || -> Result<Self> {
            anyhow::ensure!(
                !sources.is_empty(),
                "Slideshow requires at least one source"
            );

            let mut entries = Vec::with_capacity(sources.len());

            for slide in sources {
                let entry = SlideEntry { def: slide.clone() };
                entries.push(entry);
            }

            // Shuffle if requested
            if shuffle && entries.len() > 1 {
                let seed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as usize;
                for i in (1..entries.len()).rev() {
                    let j = (seed.wrapping_mul(i).wrapping_add(7)) % (i + 1);
                    entries.swap(i, j);
                }
            }

            // Load only the first child eagerly
            let first_child = load_slide_child(&entries[0], &pkg, hot_reload, gpu)
                .context("Failed to load first slideshow child")?;

            let (transition_spec, transition_duration, transition_runtime) = if let Some(spec_raw) =
                transition_spec
            {
                let (transition_wgsl, duration) = Self::resolve_transition_wgsl(&pkg, spec_raw)?;
                if duration > interval {
                    anyhow::bail!(
                        "Slideshow transition duration {} exceeds interval {}",
                        duration,
                        interval
                    );
                }
                let gpu = gpu.context(
                    "Slideshow transitions require GPU context for shader-based compositing",
                )?;
                let (initial_w, initial_h) = first_child.dimensions();
                let runtime =
                    SlideshowTransitionRuntime::new(gpu, &transition_wgsl, initial_w, initial_h)?;
                (Some(spec_raw.to_string()), Some(duration), Some(runtime))
            } else {
                (None, None, None)
            };

            info!(
                "SlideshowTexture loaded: {} slides, {:.1}s interval, shuffle={}",
                entries.len(),
                interval,
                shuffle
            );

            Ok(Self {
                pkg,
                entries,
                current: 0,
                current_child: Some(first_child),
                next_child: None,
                next_index: None,
                current_frame_cache: None,
                next_frame_cache: None,
                timer: 0.0,
                interval,
                transition_spec,
                transition_duration,
                transition_blend_enabled: transition_runtime.is_some(),
                transition_runtime,
                hot_reload,
                gpu: gpu.cloned(),
            })
        };

        match inner() {
            Ok(tex) => Ok(tex),
            Err(e) if optional => {
                warn!(
                    "Optional slideshow failed to load (using placeholder): {}",
                    e
                );
                Ok(Self {
                    pkg: pkg_fallback.expect("optional=true but no fallback pkg"),
                    entries: Vec::new(),
                    current: 0,
                    current_child: None,
                    next_child: None,
                    next_index: None,
                    current_frame_cache: None,
                    next_frame_cache: None,
                    timer: 0.0,
                    interval,
                    transition_spec: transition_spec.map(|s| s.to_string()),
                    transition_duration: None,
                    transition_blend_enabled: false,
                    transition_runtime: None,
                    hot_reload,
                    gpu: gpu.cloned(),
                })
            }
            Err(e) => Err(e),
        }
    }
}

impl TextureSource for SlideshowTexture {
    fn update(&mut self, dt: f64) -> Result<TextureUpdate> {
        if self.entries.is_empty() {
            return Ok(TextureUpdate::Unchanged);
        }

        // Advance timer and either start/advance an in-window transition or perform hard swap.
        self.timer += dt;
        let duration = self.transition_duration.unwrap_or(0.0);
        let transition_start = (self.interval - duration).max(0.0);

        if self.entries.len() > 1
            && self.next_child.is_none()
            && duration > 0.0
            && self.timer >= transition_start
        {
            let next = (self.current + 1) % self.entries.len();
            match load_slide_child(
                &self.entries[next],
                &self.pkg,
                self.hot_reload,
                self.gpu.as_ref(),
            ) {
                Ok(child) => {
                    self.next_child = Some(child);
                    self.next_index = Some(next);
                    self.next_frame_cache = None;
                    self.transition_blend_enabled = self.transition_runtime.is_some();
                    if let Some(spec) = self.transition_spec.as_deref() {
                        log::debug!(
                            "Slideshow transition '{}' started to slide {} (duration {:.3}s)",
                            spec,
                            next + 1,
                            duration
                        );
                    }
                }
                Err(e) => {
                    log::warn!(
                        "Failed to start slideshow transition to child {}: {}",
                        next,
                        e
                    );
                }
            }
        }

        if self.next_child.is_some() {
            if let Some(current) = self.current_child.as_mut() {
                let _ = Self::update_cached_frame(current, &mut self.current_frame_cache, dt)?;
            }
            if let Some(next) = self.next_child.as_mut() {
                let _ = Self::update_cached_frame(next, &mut self.next_frame_cache, dt)?;
            }

            if self.timer >= self.interval {
                self.timer %= self.interval;
                self.current = self.next_index.take().unwrap_or(self.current);
                self.current_child = self.next_child.take();
                self.current_frame_cache = self.next_frame_cache.take();
                self.transition_blend_enabled = true;
                return Ok(match &self.current_frame_cache {
                    Some((data, w, h)) => TextureUpdate::NewFrame {
                        data: data.clone(),
                        width: *w,
                        height: *h,
                    },
                    None => TextureUpdate::Unchanged,
                });
            }

            if self.transition_blend_enabled && duration > 0.0 {
                let t = ((self.timer - transition_start) / duration).clamp(0.0, 1.0) as f32;
                self.render_transition_frame(t)?;
            }

            return Ok(TextureUpdate::Unchanged);
        }

        // No active transition.
        if self.timer >= self.interval && self.entries.len() > 1 {
            self.timer %= self.interval;
            let next = (self.current + 1) % self.entries.len();

            info!(
                "Slideshow: advancing to slide {} of {}",
                next + 1,
                self.entries.len()
            );

            match load_slide_child(
                &self.entries[next],
                &self.pkg,
                self.hot_reload,
                self.gpu.as_ref(),
            ) {
                Ok(child) => {
                    self.current_child = Some(child);
                    self.current = next;
                    self.current_frame_cache = None;
                    if let Some(current) = self.current_child.as_mut() {
                        let update =
                            Self::update_cached_frame(current, &mut self.current_frame_cache, 0.0)?;
                        return Ok(update);
                    }
                }
                Err(e) => {
                    log::warn!("Failed to load slideshow child {}: {}", next, e);
                }
            }
        }

        match self.current_child.as_mut() {
            Some(child) => Self::update_cached_frame(child, &mut self.current_frame_cache, dt),
            None => Ok(TextureUpdate::Unchanged),
        }
    }

    fn dimensions(&self) -> (u32, u32) {
        if self.next_child.is_some()
            && self.transition_blend_enabled
            && let Some(runtime) = self.transition_runtime.as_ref()
        {
            return (runtime.output_width, runtime.output_height);
        }
        match &self.current_child {
            Some(child) => child.dimensions(),
            None => (1, 1),
        }
    }

    fn format(&self) -> TextureFormat {
        if self.next_child.is_some() && self.transition_blend_enabled {
            return TextureFormat::Rgba8;
        }
        match &self.current_child {
            Some(child) => child.format(),
            None => TextureFormat::Rgba8,
        }
    }

    fn audio_level(&self) -> f32 {
        match &self.current_child {
            Some(child) => child.audio_level(),
            None => 0.0,
        }
    }

    fn texture_type(&self) -> &'static str {
        "slideshow"
    }

    fn is_gpu_managed(&self) -> bool {
        if self.next_child.is_some() && self.transition_blend_enabled {
            return self.transition_runtime.is_some();
        }
        self.current_child
            .as_ref()
            .map(|child| child.is_gpu_managed())
            .unwrap_or(false)
    }

    fn gpu_texture_view(&self) -> Option<&wgpu::TextureView> {
        if self.next_child.is_some() && self.transition_blend_enabled {
            return self
                .transition_runtime
                .as_ref()
                .map(|runtime| &runtime.output_view);
        }
        self.current_child
            .as_ref()
            .and_then(|child| child.gpu_texture_view())
    }

    fn gpu_sampler(&self) -> Option<&wgpu::Sampler> {
        if self.next_child.is_some() && self.transition_blend_enabled {
            return self
                .transition_runtime
                .as_ref()
                .map(|runtime| &runtime.output_sampler);
        }
        self.current_child
            .as_ref()
            .and_then(|child| child.gpu_sampler())
    }

    fn update_uniforms(&mut self, uniforms: &kroma_shared::types::ShaderUniforms) {
        if let Some(child) = self.current_child.as_mut() {
            child.update_uniforms(uniforms);
        }
        if let Some(child) = self.next_child.as_mut() {
            child.update_uniforms(uniforms);
        }
        if let Some(runtime) = self.transition_runtime.as_mut() {
            runtime.uniforms = *uniforms;
        }
    }

    fn gpu_render(&mut self) -> Result<()> {
        if let Some(child) = self.current_child.as_mut()
            && child.is_gpu_managed()
        {
            child.gpu_render()?;
        }
        if let Some(child) = self.next_child.as_mut()
            && child.is_gpu_managed()
        {
            child.gpu_render()?;
        }

        if self.next_child.is_some() && self.transition_blend_enabled {
            let duration = self.transition_duration.unwrap_or(0.0);
            if duration > 0.0 {
                let transition_start = (self.interval - duration).max(0.0);
                let t = ((self.timer - transition_start) / duration).clamp(0.0, 1.0) as f32;
                self.render_transition_frame(t)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::SlideshowTexture;
    use std::sync::Arc;

    use indexmap::IndexMap;
    use kroma_shared::{
        shade::LiveShadePackage,
        types::{ShadeConfig, TextureDef, TextureFilter, TextureType, TextureWrap},
    };

    fn test_pkg() -> Arc<LiveShadePackage> {
        let config: ShadeConfig = toml::from_str(
            r#"
[meta]
name = "Slideshow Test"
author = "Kroma"

[states.active]
length = 0.0
"#,
        )
        .expect("valid config");
        Arc::new(LiveShadePackage::new_empty(config))
    }

    fn noise_slide() -> TextureDef {
        TextureDef {
            ty: TextureType::Noise,
            source: None,
            seed: Some(42),
            input: None,
            sources: Vec::new(),
            looping: true,
            filter: TextureFilter::default(),
            wrap: TextureWrap::default(),
            binding: None,
            font_size: None,
            interval: None,
            transition: None,
            shuffle: false,
            fft_bands: None,
            hot_reload: false,
            optional: false,
            shader: None,
            width: Some(4),
            height: Some(4),
            textures: IndexMap::new(),
            uniforms: IndexMap::new(),
        }
    }

    #[test]
    fn slideshow_rejects_transition_duration_over_interval() {
        let pkg = test_pkg();
        let slides = vec![noise_slide()];
        match SlideshowTexture::load(
            pkg,
            &slides,
            1.0,
            Some("kroma.fade:2.0"),
            false,
            false,
            false,
            None,
        ) {
            Ok(_) => panic!("duration > interval must fail"),
            Err(err) => {
                assert!(
                    err.to_string().contains("exceeds interval"),
                    "unexpected error: {err}"
                );
            }
        }
    }
}
