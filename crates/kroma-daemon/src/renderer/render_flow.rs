use super::*;
use wgpu::wgt::PollType;

/// Creates the intermediate final-output and asynchronous capture resources.
pub(super) fn create_output_capture_state(
    device: &wgpu::Device,
    vert_module: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    width: u32,
    height: u32,
) -> OutputCaptureState {
    let final_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("kroma-final-output"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: surface_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let final_view = final_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("kroma-final-output-sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("kroma-final-output-bgl"),
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
        ],
    });
    let output_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("kroma-final-output-bg"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&final_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("kroma-final-output-pl"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let frag_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("kroma-final-output-frag"),
        source: wgpu::ShaderSource::Wgsl(OUTPUT_BLIT_FRAG_WGSL.into()),
    });
    let surface_pipeline = create_fullscreen_pipeline(
        device,
        &pipeline_layout,
        vert_module,
        &frag_module,
        surface_format,
        "kroma-final-output-pipeline",
    );
    let capture_format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let capture_pipeline = create_fullscreen_pipeline(
        device,
        &pipeline_layout,
        vert_module,
        &frag_module,
        capture_format,
        "kroma-lighting-downsample-pipeline",
    );
    let capture_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("kroma-lighting-capture"),
        size: wgpu::Extent3d {
            width: LIGHTING_CAPTURE_WIDTH,
            height: LIGHTING_CAPTURE_HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: capture_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let capture_view = capture_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let readback_size = u64::from(LIGHTING_CAPTURE_BYTES_PER_ROW * LIGHTING_CAPTURE_HEIGHT);
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("kroma-lighting-readback"),
        size: readback_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let (completion_tx, completion_rx) = std::sync::mpsc::channel();

    OutputCaptureState {
        _final_texture: final_texture,
        final_view,
        output_bind_group,
        surface_pipeline,
        capture_texture,
        capture_view,
        capture_pipeline,
        readback_buffer,
        completion_tx,
        completion_rx,
        mapping_pending: false,
    }
}

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

fn collect_lighting_frame(state: &mut Renderer) -> Result<Option<LightingFrame>> {
    let Some(device) = state.gpu.device.as_ref() else {
        return Ok(None);
    };
    if let Err(error) = device.poll(PollType::Poll) {
        warn!("GPU poll for lighting capture failed: {}", error);
    }

    let Some(output) = state.gpu.output_capture.as_mut() else {
        return Ok(None);
    };
    if !output.mapping_pending {
        return Ok(None);
    }
    let completion = match output.completion_rx.try_recv() {
        Ok(completion) => completion,
        Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(None),
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            output.mapping_pending = false;
            anyhow::bail!("Lighting capture completion channel disconnected");
        }
    };
    output.mapping_pending = false;
    if let Err(error) = completion {
        output.readback_buffer.unmap();
        warn!("Lighting capture buffer mapping failed: {}", error);
        return Ok(None);
    }

    let mapped = output.readback_buffer.slice(..).get_mapped_range();
    let unpadded_row_size = (LIGHTING_CAPTURE_WIDTH * 4) as usize;
    let padded_row_size = LIGHTING_CAPTURE_BYTES_PER_ROW as usize;
    let mut rgba = Vec::with_capacity(unpadded_row_size * LIGHTING_CAPTURE_HEIGHT as usize);
    for row in mapped
        .chunks(padded_row_size)
        .take(LIGHTING_CAPTURE_HEIGHT as usize)
    {
        rgba.extend_from_slice(&row[..unpadded_row_size]);
    }
    drop(mapped);
    output.readback_buffer.unmap();
    LightingFrame::new(LIGHTING_CAPTURE_WIDTH, LIGHTING_CAPTURE_HEIGHT, rgba).map(Some)
}

fn encode_blit_pass(
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    label: &str,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
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
    pass.draw(0..3, 0..1);
}

/// Renders one frame, presents it, and optionally queues a lighting capture.
pub(super) fn render_frame(
    state: &mut Renderer,
    capture_lighting: bool,
) -> Result<Option<LightingFrame>> {
    let completed_lighting_frame = collect_lighting_frame(state)?;
    let Some(queue) = state.gpu.queue.as_ref().cloned() else {
        return Ok(completed_lighting_frame);
    };

    upload_frame_uniforms(state);

    let Some(surface) = state.gpu.surface.as_ref() else {
        return Ok(completed_lighting_frame);
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
            return Ok(completed_lighting_frame);
        }
        wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
            warn!("Surface timeout — skipping frame and polling GPU cleanup");
            queue.submit(std::iter::empty::<wgpu::CommandBuffer>());
            if let Some(device) = state.gpu.device.as_ref()
                && let Err(e) = device.poll(PollType::wait_indefinitely())
            {
                warn!("GPU poll during timeout failed: {}", e);
            }
            return Ok(completed_lighting_frame);
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
        .cloned()
        .context("GPU not initialised — cannot render frame")?;

    if capture_lighting && state.gpu.output_capture.is_none() {
        let config = state
            .gpu
            .surface_config
            .as_ref()
            .context("Surface configuration is unavailable")?;
        let vert_module = state
            .gpu
            .vert_module
            .as_ref()
            .context("Vertex shader disappeared")?;
        state.gpu.output_capture = Some(create_output_capture_state(
            &device,
            vert_module,
            config.format,
            config.width.max(1),
            config.height.max(1),
        ));
    }
    let capture_started = capture_lighting
        && state
            .gpu
            .output_capture
            .as_ref()
            .is_some_and(|output| !output.mapping_pending);
    let render_view = if capture_started {
        state
            .gpu
            .output_capture
            .as_ref()
            .map(|output| output.final_view.clone())
            .context("Final output target is unavailable")?
    } else {
        view.clone()
    };

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

        // Pass 3: blend both live lanes into the shared final output.
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("kroma-transition-composite-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &render_view,
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
        encode_fullscreen_pass(
            state,
            &mut encoder,
            &render_view,
            "kroma-render-pass",
            false,
        )?;
    }

    if capture_started {
        let output = state
            .gpu
            .output_capture
            .as_ref()
            .context("Final output resources disappeared")?;
        encode_blit_pass(
            &mut encoder,
            &view,
            &output.surface_pipeline,
            &output.output_bind_group,
            "kroma-present-pass",
        );

        encode_blit_pass(
            &mut encoder,
            &output.capture_view,
            &output.capture_pipeline,
            &output.output_bind_group,
            "kroma-lighting-downsample-pass",
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &output.capture_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &output.readback_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(LIGHTING_CAPTURE_BYTES_PER_ROW),
                    rows_per_image: Some(LIGHTING_CAPTURE_HEIGHT),
                },
            },
            wgpu::Extent3d {
                width: LIGHTING_CAPTURE_WIDTH,
                height: LIGHTING_CAPTURE_HEIGHT,
                depth_or_array_layers: 1,
            },
        );
    }

    queue.submit(std::iter::once(encoder.finish()));
    frame.present();

    if capture_started {
        let output = state
            .gpu
            .output_capture
            .as_mut()
            .context("Lighting capture resources disappeared")?;
        let completion_tx = output.completion_tx.clone();
        output
            .readback_buffer
            .map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = completion_tx.send(result);
            });
        output.mapping_pending = true;
    }

    Ok(completed_lighting_frame)
}
