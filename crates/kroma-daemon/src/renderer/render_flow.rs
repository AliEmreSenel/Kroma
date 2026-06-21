use super::*;
use wgpu::wgt::PollType;

/// Returns the active render target format, falling back to a safe default.
pub(super) fn active_surface_format(state: &Renderer) -> wgpu::TextureFormat {
    state
        .gpu
        .surface_config
        .as_ref()
        .map(|c| c.format)
        .unwrap_or(SURFACE_FORMAT)
}

/// Builds the fullscreen pipeline used by the main frame and preview paths.
pub(super) fn create_fullscreen_pipeline(
    device: &wgpu::Device,
    pipeline_layout: &wgpu::PipelineLayout,
    vert_module: &wgpu::ShaderModule,
    frag_module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(pipeline_layout),
        vertex: wgpu::VertexState {
            module: vert_module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: frag_module,
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
    })
}

/// Encodes one fullscreen pass that binds globals and optional textures.
///
/// When `require_pipeline` is true, missing pipeline/bindings are treated as
/// errors; otherwise the pass becomes a no-op.
pub(super) fn encode_fullscreen_pass(
    state: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    target_view: &wgpu::TextureView,
    label: &str,
    require_pipeline: bool,
) -> Result<()> {
    let pipeline = state.gpu.pipeline.as_ref();
    let bind_group = state.gpu.bind_group.as_ref();

    if require_pipeline {
        let _ = pipeline.context("No render pipeline")?;
        let _ = bind_group.context("No bind group")?;
    }

    let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target_view,
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

    if let (Some(pipeline), Some(bind_group)) = (pipeline, bind_group) {
        render_pass.set_pipeline(pipeline);
        render_pass.set_bind_group(0, bind_group, &[]);
        if let Some(ref tex_bg) = state.gpu.texture_bind_group {
            render_pass.set_bind_group(1, tex_bg, &[]);
        }
        render_pass.draw(0..3, 0..1);
    }

    Ok(())
}

/// Uploads a uniform block into the global uniform buffer, if initialized.
pub(super) fn write_uniform_buffer(state: &Renderer, uniforms: &ShaderUniforms) {
    if let (Some(queue), Some(buf)) = (state.gpu.queue.as_ref(), state.gpu.uniform_buffer.as_ref())
    {
        queue.write_buffer(buf, 0, bytemuck::bytes_of(uniforms));
    }
}

/// Uploads the per-frame uniforms and dynamic custom uniform buffer.
pub(super) fn upload_frame_uniforms(state: &Renderer) {
    state.write_uniform_buffer(&state.uniforms);
    state.upload_custom_uniforms();
}

/// Rebuilds the main pipeline with the provided fragment WGSL source.
pub(super) fn rebuild_pipeline_with_frag(state: &mut Renderer, frag_wgsl: &str) -> Result<()> {
    let device = state.gpu.device.as_ref().context("GPU not initialised")?;
    let pipeline_layout = state
        .gpu
        .pipeline_layout
        .as_ref()
        .context("Pipeline layout not available")?;
    let vert_module = state
        .gpu
        .vert_module
        .as_ref()
        .context("Vertex shader not available")?;

    let format = active_surface_format(state);

    let frag_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("kroma-frag-custom"),
        source: wgpu::ShaderSource::Wgsl(frag_wgsl.into()),
    });

    let pipeline = create_fullscreen_pipeline(
        device,
        pipeline_layout,
        vert_module,
        &frag_module,
        format,
        "kroma-pipeline-custom",
    );

    state.gpu.pipeline = Some(pipeline);
    info!("Render pipeline rebuilt with new fragment shader");
    Ok(())
}

/// Recreates pipeline layout according to currently active bind group layouts.
pub(super) fn rebuild_pipeline_layout(state: &mut Renderer) -> Result<()> {
    let device = state.gpu.device.as_ref().context("GPU not initialised")?;
    let bgl0 = state
        .gpu
        .bind_group_layout
        .as_ref()
        .context("Uniform BGL missing")?;

    let layouts: Vec<Option<&wgpu::BindGroupLayout>> =
        if let Some(ref tex_bgl) = state.gpu.texture_bind_group_layout {
            vec![Some(bgl0), Some(tex_bgl)]
        } else {
            vec![Some(bgl0)]
        };

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("kroma-pl"),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });

    state.gpu.pipeline_layout = Some(pipeline_layout);

    Ok(())
}

/// Renders one frame to the surface, handling recoverable surface errors.
pub(super) fn render_frame(state: &mut Renderer) -> Result<()> {
    let Some(queue) = state.gpu.queue.as_ref().cloned() else {
        return Ok(());
    };

    upload_frame_uniforms(state);

    let Some(surface) = state.gpu.surface.as_ref() else {
        return Ok(());
    };

    let frame = match surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame)
        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
        wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
            if let (Some(device), Some(config)) =
                (state.gpu.device.as_ref(), state.gpu.surface_config.as_ref())
            {
                surface.configure(device, config);
            }
            return Ok(());
        }
        wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
            warn!("Surface timeout — skipping frame and polling GPU cleanup");
            queue.submit(std::iter::empty::<wgpu::CommandBuffer>());
            if let Some(device) = state.gpu.device.as_ref()
                && let Err(e) = device.poll(PollType::wait_indefinitely())
            {
                warn!("GPU poll during timeout failed: {}", e);
            }
            return Ok(());
        }
        wgpu::CurrentSurfaceTexture::Validation => {
            return Err(anyhow::anyhow!(
                "Surface validation error while acquiring frame"
            ));
        }
    };

    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    let device = state
        .gpu
        .device
        .as_ref()
        .context("GPU not initialised — cannot render frame")?;

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("kroma-frame"),
    });

    if state.transition_state.is_some() {
        let mut transition = state
            .transition_state
            .take()
            .context("Transition state disappeared during frame render")?;

        if let Some(lane) = state.transition_lane_uniforms {
            let mut lane_uniforms = state.uniforms;
            lane_uniforms.u_time = lane.outgoing_elapsed;
            lane_uniforms.u_frame = lane.outgoing_frame;
            write_uniform_buffer(state, &lane_uniforms);
            state.upload_custom_uniforms();
        }

        // Pass 1: render outgoing lane into prev target.
        encode_fullscreen_pass(
            state,
            &mut encoder,
            &transition.prev_view,
            "kroma-transition-prev-pass",
            true,
        )?;

        // Pass 2: render incoming lane using the exact same lane pipeline path.
        if let Some(lane) = state.transition_lane_uniforms {
            let mut lane_uniforms = state.uniforms;
            lane_uniforms.u_time = lane.incoming_elapsed;
            lane_uniforms.u_frame = lane.incoming_frame;
            write_uniform_buffer(state, &lane_uniforms);
            state.upload_custom_uniforms();
        }

        let outgoing_lane = state.take_active_lane();
        state.install_active_lane(transition.incoming_lane);
        encode_fullscreen_pass(
            state,
            &mut encoder,
            &transition.next_view,
            "kroma-transition-next-pass",
            true,
        )?;
        transition.incoming_lane = state.take_active_lane();
        state.install_active_lane(outgoing_lane);

        // Composite pass uses global transition uniforms.
        write_uniform_buffer(state, &state.uniforms);
        state.upload_custom_uniforms();

        // Pass 2: blend previous snapshot with live next pass into the real surface.
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("kroma-transition-composite-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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

        if let Some(bg0) = state.gpu.bind_group.as_ref() {
            pass.set_pipeline(&transition.pipeline);
            pass.set_bind_group(0, bg0, &[]);
            pass.set_bind_group(1, &transition.texture_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        state.transition_state = Some(transition);
    } else {
        encode_fullscreen_pass(state, &mut encoder, &view, "kroma-render-pass", false)?;
    }

    queue.submit(std::iter::once(encoder.finish()));
    frame.present();

    Ok(())
}
