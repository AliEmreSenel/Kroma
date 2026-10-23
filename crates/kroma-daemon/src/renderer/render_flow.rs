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
        blit_pipeline_layout: pipeline_layout,
        blit_fragment: frag_module,
        capture_texture,
        capture_view,
        capture_pipeline,
        readback_buffer,
        completion_tx,
        completion_rx,
        mapping_pending: false,
    }
}

/// Returns the fixed logical rendering format, independent of presentation formats.
pub(super) fn active_surface_format(_state: &Renderer) -> wgpu::TextureFormat {
    SURFACE_FORMAT
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

fn collect_lighting_frame(output: &mut OutputCaptureState) -> Result<Option<LightingFrame>> {
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

/// Creates independent transition attachments for a monitor.
fn transition_targets(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    size: (u32, u32),
) -> MonitorTransitionTargets {
    let make_view = || {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("kroma-monitor-transition"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SURFACE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let prev_view = make_view();
    let next_view = make_view();
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("kroma-monitor-transition-bindings"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&prev_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&next_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    MonitorTransitionTargets {
        prev_view,
        next_view,
        bind_group,
    }
}

fn globals_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    custom: Option<&wgpu::Buffer>,
) -> wgpu::BindGroup {
    let mut entries = vec![wgpu::BindGroupEntry {
        binding: 0,
        resource: globals.as_entire_binding(),
    }];
    if let Some(custom) = custom {
        entries.push(wgpu::BindGroupEntry {
            binding: 1,
            resource: custom.as_entire_binding(),
        });
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("kroma-monitor-uniforms"),
        layout,
        entries: &entries,
    })
}

fn scene_pass(
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    globals: &wgpu::BindGroup,
    textures: Option<&wgpu::BindGroup>,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("kroma-monitor-scene"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
            depth_slice: None,
        })],
        ..Default::default()
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, globals, &[]);
    if let Some(textures) = textures {
        pass.set_bind_group(1, textures, &[]);
    }
    pass.draw(0..3, 0..1);
}

fn capture_output(encoder: &mut wgpu::CommandEncoder, output: &OutputCaptureState) {
    encode_blit_pass(
        encoder,
        &output.capture_view,
        &output.capture_pipeline,
        &output.output_bind_group,
        "kroma-lighting-downsample",
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

/// Keeps shader textures (including feedback and nested shaders) local to a target.
#[allow(clippy::too_many_arguments)]
fn monitor_textures(
    gpu: &GpuContext,
    sources: &mut MonitorShaderSources,
    defs: &[(usize, Arc<LiveShadePackage>, TextureDef)],
    layout: Option<&wgpu::BindGroupLayout>,
    shared: &[LoadedTexture],
    shared_sources: &[Option<Box<dyn TextureSource>>],
    uniforms: &ShaderUniforms,
) -> Result<Option<wgpu::BindGroup>> {
    if defs.is_empty() {
        return Ok(None);
    }
    if sources.is_empty() {
        let mut created = Vec::new();
        for (index, package, def) in defs {
            if let Some(source) = textures::create_texture_source(package, def, Some(gpu))? {
                created.push((*index, source));
            }
        }
        *sources = created;
    }
    for (_, source) in sources.iter_mut() {
        source.update_uniforms(uniforms);
        if let TextureUpdate::NewFrame { data, .. } =
            source.update(f64::from(uniforms.u_delta_time))?
        {
            source.recycle_frame(data);
        }
        source.gpu_render()?;
    }
    let mut entries = Vec::new();
    for (index, texture) in shared.iter().enumerate() {
        let source = sources
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, s)| s.as_ref())
            .or_else(|| shared_sources.get(index).and_then(|s| s.as_deref()));
        let view = source
            .and_then(|s| s.gpu_texture_view())
            .unwrap_or(&texture.view);
        let sampler = source
            .and_then(|s| s.gpu_sampler())
            .unwrap_or(&texture.sampler);
        entries.push(wgpu::BindGroupEntry {
            binding: (index * 2) as u32,
            resource: wgpu::BindingResource::TextureView(view),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: (index * 2 + 1) as u32,
            resource: wgpu::BindingResource::Sampler(sampler),
        });
    }
    Ok(Some(gpu.device.create_bind_group(
        &wgpu::BindGroupDescriptor {
            label: Some("kroma-monitor-textures"),
            layout: layout.context("Missing texture layout")?,
            entries: &entries,
        },
    )))
}

/// Renders every monitor independently using its own attachments and uniforms.
pub(super) fn render_frame(
    state: &mut Renderer,
    capture_lighting: &[String],
    occluded: &[MonitorId],
) -> Result<Vec<(String, LightingFrame)>> {
    let device = state.gpu.device.as_ref().context("GPU not initialized")?;
    let queue = state
        .gpu
        .queue
        .as_ref()
        .context("GPU queue not initialized")?;
    device.poll(PollType::Poll)?;
    state.upload_custom_uniforms();
    let size = state.surface_dimensions();
    let texture_gpu = GpuContext {
        device: Arc::clone(device),
        queue: Arc::clone(queue),
        surface_format: SURFACE_FORMAT,
    };
    let mut completed = Vec::new();
    let mut outgoing = state.uniforms;
    let mut incoming = state.uniforms;
    if let Some(lanes) = state.transition_lane_uniforms {
        outgoing.u_time = lanes.outgoing_elapsed;
        outgoing.u_frame = lanes.outgoing_frame;
        incoming.u_time = lanes.incoming_elapsed;
        incoming.u_frame = lanes.incoming_frame;
    }
    let outgoing_layout = state
        .gpu
        .bind_group_layout
        .as_ref()
        .context("Missing uniform layout")?;
    let outgoing_pipeline = state
        .gpu
        .pipeline
        .as_ref()
        .context("Missing scene pipeline")?;
    if let Some(transition) = &state.transition_state
        && let Some(custom) = &transition.incoming_lane.custom_uniform_buffer
    {
        queue.write_buffer(
            custom,
            0,
            bytemuck::cast_slice(&transition.incoming_lane.custom_uniform_data),
        );
    }
    for (name, target) in &mut state.gpu.targets {
        let frame_result = collect_lighting_frame(&mut target.output)?;
        if let Some(frame) = frame_result {
            completed.push((name.clone(), frame));
        }
        if !target_ready(target, occluded, std::time::Instant::now()) {
            continue;
        }
        // Reconfiguration/recreation is deferred to sync_targets, before acquisition.
        let rendered = (|| -> Result<()> {
            let frame = if let Some(surface) = &target.surface {
                match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame) => Some(frame),
                    wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                        target.needs_reconfigure = true;
                        Some(frame)
                    }
                    wgpu::CurrentSurfaceTexture::Outdated => {
                        target.needs_reconfigure = true;
                        return Ok(());
                    }
                    wgpu::CurrentSurfaceTexture::Lost => {
                        target.surface = None;
                        return Ok(());
                    }
                    wgpu::CurrentSurfaceTexture::Timeout
                    | wgpu::CurrentSurfaceTexture::Occluded => {
                        warn!("Surface acquisition timed out or is occluded on {name}");
                        target.retry_after =
                            std::time::Instant::now() + std::time::Duration::from_millis(100);
                        return Ok(());
                    }
                    wgpu::CurrentSurfaceTexture::Validation => {
                        target.retry_after =
                            std::time::Instant::now() + std::time::Duration::from_secs(1);
                        warn!("Surface validation failure on {name}");
                        return Ok(());
                    }
                }
            } else {
                None
            };
            target.shader_sources.retain(|id, _| {
                *id == state.lane_id
                    || state
                        .transition_state
                        .as_ref()
                        .is_some_and(|t| *id == t.incoming_lane.lane_id)
            });
            let out_textures = monitor_textures(
                &texture_gpu,
                target.shader_sources.entry(state.lane_id).or_default(),
                &state.shader_defs,
                state.gpu.texture_bind_group_layout.as_ref(),
                &state.textures,
                &state.texture_sources,
                &outgoing,
            )?;
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("kroma-monitor-frame"),
            });
            queue.write_buffer(&target.globals[0], 0, bytemuck::bytes_of(&outgoing));
            let out_globals = globals_group(
                device,
                outgoing_layout,
                &target.globals[0],
                state.custom_uniform_buffer.as_ref(),
            );
            if let Some(transition) = &state.transition_state {
                let scratch = target.transition.get_or_insert_with(|| {
                    transition_targets(device, &transition.texture_layout, size)
                });
                scene_pass(
                    &mut encoder,
                    &scratch.prev_view,
                    outgoing_pipeline,
                    &out_globals,
                    out_textures
                        .as_ref()
                        .or(state.gpu.texture_bind_group.as_ref()),
                );
                let lane = &transition.incoming_lane;
                let in_textures = monitor_textures(
                    &texture_gpu,
                    target.shader_sources.entry(lane.lane_id).or_default(),
                    &lane.shader_defs,
                    lane.texture_bind_group_layout.as_ref(),
                    &lane.textures,
                    &lane.texture_sources,
                    &incoming,
                )?;
                queue.write_buffer(&target.globals[1], 0, bytemuck::bytes_of(&incoming));
                let in_globals = globals_group(
                    device,
                    lane.bind_group_layout
                        .as_ref()
                        .context("Missing incoming uniform layout")?,
                    &target.globals[1],
                    lane.custom_uniform_buffer.as_ref(),
                );
                scene_pass(
                    &mut encoder,
                    &scratch.next_view,
                    lane.pipeline
                        .as_ref()
                        .context("Missing incoming pipeline")?,
                    &in_globals,
                    in_textures.as_ref().or(lane.texture_bind_group.as_ref()),
                );
                queue.write_buffer(&target.globals[2], 0, bytemuck::bytes_of(&state.uniforms));
                let composite_globals = globals_group(
                    device,
                    outgoing_layout,
                    &target.globals[2],
                    state.custom_uniform_buffer.as_ref(),
                );
                scene_pass(
                    &mut encoder,
                    &target.output.final_view,
                    &transition.pipeline,
                    &composite_globals,
                    Some(&scratch.bind_group),
                );
            } else {
                target.transition = None;
                scene_pass(
                    &mut encoder,
                    &target.output.final_view,
                    outgoing_pipeline,
                    &out_globals,
                    out_textures
                        .as_ref()
                        .or(state.gpu.texture_bind_group.as_ref()),
                );
            }
            if let (Some(frame), Some(blit)) = (&frame, &target.blit) {
                let view = frame.texture.create_view(&Default::default());
                encode_blit_pass(
                    &mut encoder,
                    &view,
                    blit,
                    &target.output.output_bind_group,
                    "kroma-monitor-present",
                );
            }
            let capture = capture_lighting.contains(name) && !target.output.mapping_pending;
            if capture {
                capture_output(&mut encoder, &target.output);
            }
            queue.submit([encoder.finish()]);
            if let Some(frame) = frame {
                frame.present();
            }
            if capture {
                let tx = target.output.completion_tx.clone();
                target
                    .output
                    .readback_buffer
                    .map_async(wgpu::MapMode::Read, .., move |result| {
                        let _ = tx.send(result);
                    });
                target.output.mapping_pending = true;
            }
            Ok(())
        })();
        if let Err(error) = rendered {
            warn!("Monitor {name} render failed: {error:#}");
            target.retry_after = std::time::Instant::now() + std::time::Duration::from_secs(1);
        }
    }
    Ok(completed)
}
