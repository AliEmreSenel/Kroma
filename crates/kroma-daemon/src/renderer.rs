//! wgpu render pipeline for Kroma.
//!
//! Manages the GPU device, shader module, uniform buffer, and frame
//! rendering. Renders to a real Wayland surface via wgpu.

use anyhow::{Context, Result};
use log::{info, warn};

use kroma_shared::shade::ShadePackage;
use kroma_shared::types::ShaderUniforms;

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
    u_mouse: vec2<f32>,
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

/// The preferred surface texture format.
const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

/// Holds the entire wgpu render state.
pub struct RenderState {
    /// Current shader uniforms (CPU side).
    pub uniforms: ShaderUniforms,

    /// Custom uniforms set via IPC (name → value).
    custom_uniforms: std::collections::HashMap<String, kroma_shared::ipc::UniformValue>,

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

    /// Current fragment shader source (WGSL).
    current_frag_wgsl: String,
}

impl RenderState {
    /// Create a new render state. Does NOT initialise the GPU yet.
    pub fn new() -> Result<Self> {
        Ok(Self {
            uniforms: ShaderUniforms::default(),
            custom_uniforms: std::collections::HashMap::new(),
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
            current_frag_wgsl: DEFAULT_FRAG_WGSL.to_string(),
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
        info!("GPU adapter: {} ({:?})", adapter_info.name, adapter_info.backend);

        let (device, queue) = pollster_block(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("kroma-device"),
                ..Default::default()
            },
            None,
        ))
        .context("Failed to create GPU device")?;

        // Determine the best surface format
        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);

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
                .unwrap_or(surface_caps.alpha_modes[0]),
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
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
            push_constant_ranges: &[],
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
            multiview: None,
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

        info!("GPU pipeline initialised with real surface ({}x{})", width, height);
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

        let (device, queue) = pollster_block(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("kroma-device-headless"),
                ..Default::default()
            },
            None,
        ))
        .context("Failed to create GPU device")?;

        // Create uniform buffer
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("kroma-uniforms"),
            size: std::mem::size_of::<ShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Bind group layout
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

    /// Load a shade package's GLSL fragment shader into the pipeline.
    ///
    /// Translates GLSL → WGSL via naga, recompiles the fragment shader,
    /// and rebuilds the render pipeline.
    pub fn load_shade(&mut self, pkg: &ShadePackage) -> Result<()> {
        let glsl_source = &pkg.shader_source;

        info!("Compiling shade shader ({} bytes GLSL)...", glsl_source.len());

        // Translate GLSL → WGSL via naga
        let wgsl_source = glsl_to_wgsl(glsl_source)
            .context("Failed to translate GLSL shader to WGSL")?;

        info!("GLSL→WGSL translation successful ({} bytes WGSL)", wgsl_source.len());

        // Rebuild the pipeline with the new fragment shader
        self.rebuild_pipeline_with_frag(&wgsl_source)?;
        self.current_frag_wgsl = wgsl_source;

        info!("Shade package shader '{}' loaded successfully", pkg.config.meta.name);
        Ok(())
    }

    /// Load a raw WGSL fragment shader string (used for the default shader or testing).
    pub fn load_wgsl_fragment(&mut self, wgsl: &str) -> Result<()> {
        self.rebuild_pipeline_with_frag(wgsl)?;
        self.current_frag_wgsl = wgsl.to_string();
        Ok(())
    }

    /// Rebuild the render pipeline with a new fragment shader.
    fn rebuild_pipeline_with_frag(&mut self, frag_wgsl: &str) -> Result<()> {
        let device = self.device.as_ref()
            .context("GPU not initialised")?;
        let pipeline_layout = self.pipeline_layout.as_ref()
            .context("Pipeline layout not available")?;
        let vert_module = self.vert_module.as_ref()
            .context("Vertex shader not available")?;

        let format = self.surface_config.as_ref()
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
            multiview: None,
            cache: None,
        });

        self.pipeline = Some(pipeline);
        info!("Render pipeline rebuilt with new fragment shader");
        Ok(())
    }

    /// Set a custom uniform value from the IPC command.
    pub fn set_custom_uniform(&mut self, name: &str, value: &kroma_shared::ipc::UniformValue) {
        self.custom_uniforms.insert(name.to_string(), value.clone());
        log::debug!("Custom uniform '{}' set to {:?}", name, value);
    }

    /// Resize the render surface (e.g., after monitor reconfiguration).
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

        // Get the current surface texture to render to
        let Some(surface) = self.surface.as_ref() else {
            // No surface — headless mode, skip rendering
            return Ok(());
        };

        let frame = match surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                // Reconfigure the surface
                if let (Some(device), Some(config)) = (self.device.as_ref(), self.surface_config.as_ref()) {
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

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let device = self.device.as_ref().unwrap();
        let pipeline = self.pipeline.as_ref();
        let bind_group = self.bind_group.as_ref();

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("kroma-frame"),
        });

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
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            if let (Some(pipeline), Some(bind_group)) = (pipeline, bind_group) {
                render_pass.set_pipeline(pipeline);
                render_pass.set_bind_group(0, bind_group, &[]);
                render_pass.draw(0..3, 0..1); // Fullscreen triangle
            }
        }

        queue.submit(std::iter::once(encoder.finish()));
        frame.present();

        Ok(())
    }

    /// Check if the GPU has been initialised with a real surface.
    pub fn has_surface(&self) -> bool {
        self.surface.is_some()
    }
}

/// Translate GLSL fragment shader source to WGSL using naga.
///
/// This handles Kroma-style GLSL (output of the Shadertoy translator)
/// and produces WGSL that wgpu can compile.
fn glsl_to_wgsl(glsl_source: &str) -> Result<String> {
    use naga::front::glsl::{Frontend, Options};
    use naga::back::wgsl;
    use naga::valid::{Capabilities, ValidationFlags, Validator};

    // Parse GLSL as a fragment shader
    let mut frontend = Frontend::default();
    let options = Options::from(naga::ShaderStage::Fragment);

    let module = frontend
        .parse(&options, glsl_source)
        .map_err(|errors| {
            let err_str = format!("{}", errors);
            log::error!("GLSL parse errors:\n{}", err_str);
            anyhow::anyhow!("GLSL parse errors: {}", err_str)
        })?;

    // Validate the module
    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
    let info = validator
        .validate(&module)
        .map_err(|e| {
            log::error!("Shader validation error: {}", e);
            anyhow::anyhow!("Shader validation error: {}", e)
        })?;

    // Write WGSL output
    let mut wgsl_source = wgsl::write_string(&module, &info, wgsl::WriterFlags::empty())
        .map_err(|e| anyhow::anyhow!("WGSL write error: {}", e))?;

    // Rename the fragment entry point from "main" to "fs_main"
    // to match the pipeline's expected entry point name.
    wgsl_source = wgsl_source.replace("fn main(", "fn fs_main(");

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
