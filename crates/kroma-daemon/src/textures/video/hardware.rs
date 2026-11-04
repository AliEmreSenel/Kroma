//! Vulkan video interop on the renderer's actual physical and logical device.

use std::cell::RefCell;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use ash::vk::{self, Handle};
use ffmpeg_next::ffi;

use crate::textures::GpuContext;

type Vulkan = wgpu::hal::api::Vulkan;

thread_local! {
    static FFMPEG_QUEUE_GUARD: RefCell<(usize, Option<std::sync::MutexGuard<'static, ()>>)> = const { RefCell::new((0, None)) };
}

// FFmpeg may create several device contexts borrowing the same native queues.
// Its per-context default mutexes would not protect those queues from each
// other or from wgpu. Use the renderer's submission guard for every context.
unsafe extern "C" fn lock_queue(_context: *mut ffi::AVHWDeviceContext, _family: u32, _index: u32) {
    FFMPEG_QUEUE_GUARD.with(|held| {
        let mut held = held.borrow_mut();
        if held.0 == 0 {
            held.1 = Some(crate::gpu_interop::lock_submission());
        }
        held.0 += 1;
    });
}

unsafe extern "C" fn unlock_queue(
    _context: *mut ffi::AVHWDeviceContext,
    _family: u32,
    _index: u32,
) {
    FFMPEG_QUEUE_GUARD.with(|held| {
        let mut held = held.borrow_mut();
        held.0 = held.0.saturating_sub(1);
        if held.0 == 0 {
            held.1 = None;
        }
    });
}

/// FFmpeg device reference borrowing the renderer's Vulkan device.
/// The retained wgpu owner outlives all FFmpeg objects using these handles.
pub(crate) struct Device {
    reference: *mut ffi::AVBufferRef,
    vaapi: *mut ffi::AVBufferRef,
    mapped_frames: *mut ffi::AVBufferRef,
    _owner: Arc<wgpu::Device>,
}

// FFmpeg frames/codecs may retain the native device after the Rust decoder
// wrapper is dropped. Keep every borrowed pointer and its GPU owner alive
// until FFmpeg releases the last native device reference.
struct DeviceLease {
    _extensions: Vec<*const std::ffi::c_char>,
    _instance_extensions: Vec<*const std::ffi::c_char>,
    _features: Box<vk::PhysicalDeviceSynchronization2Features<'static>>,
    _timeline: Box<vk::PhysicalDeviceTimelineSemaphoreFeatures<'static>>,
    _owner: Arc<wgpu::Device>,
}

unsafe extern "C" fn free_device_lease(context: *mut ffi::AVHWDeviceContext) {
    // SAFETY: FFmpeg calls this once, after native device uninitialization and
    // after the last reference is gone. Only this callback owns the box.
    unsafe {
        let lease = (*context).user_opaque.cast::<DeviceLease>();
        (*context).user_opaque = std::ptr::null_mut();
        if !lease.is_null() {
            drop(Box::from_raw(lease));
        }
    }
}

unsafe extern "C" fn free_borrowed_frames(_opaque: *mut std::ffi::c_void, _data: *mut u8) {
    // The real AVHWFramesContext owns this allocation. Its destructor drains
    // FFmpeg's import submissions, releasing these borrowed references before
    // freeing the context itself.
}

impl Device {
    /// Initialize FFmpeg on the already-selected renderer device and video queue.
    pub(crate) fn new(gpu: &GpuContext) -> Result<Self> {
        Self::shared(gpu, true)
    }

    fn shared(gpu: &GpuContext, native_video: bool) -> Result<Self> {
        let owner = Arc::clone(&gpu.device);
        let queues = gpu
            .video_queues
            .as_ref()
            .context("GPU video interop was not configured")?;
        // SAFETY: all native pointers are obtained from a live wgpu device and
        // retained by this owner until after av_buffer_unref. Extension arrays
        // and feature chains stay at stable addresses for the device lifetime.
        unsafe {
            let hal = owner.as_hal::<Vulkan>().context("renderer is not Vulkan")?;
            let (family, compute, graphics) = queues.for_device(hal.raw_device().handle())?;
            ensure!(
                !native_video || family.is_some(),
                "rendering GPU has no native Vulkan video decode queue"
            );
            let shared = hal.shared_instance();
            let instance = shared.raw_instance();
            let physical = hal.raw_physical_device();
            let extensions: Vec<_> = hal
                .enabled_device_extensions()
                .iter()
                .map(|name| name.as_ptr())
                .collect();
            let instance_extensions: Vec<_> = shared
                .extensions()
                .iter()
                .map(|name| name.as_ptr())
                .collect();
            let mut features = Box::new(
                vk::PhysicalDeviceSynchronization2Features::default().synchronization2(true),
            );
            let mut timeline = Box::new(
                vk::PhysicalDeviceTimelineSemaphoreFeatures::default().timeline_semaphore(true),
            );
            features.p_next =
                (&mut *timeline as *mut vk::PhysicalDeviceTimelineSemaphoreFeatures<'_>).cast();
            let reference =
                ffi::av_hwdevice_ctx_alloc(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN);
            ensure!(
                !reference.is_null(),
                "could not allocate FFmpeg Vulkan context"
            );
            let context = (*reference).data.cast::<ffi::AVHWDeviceContext>();
            let vulkan = (*context).hwctx.cast::<ffi::AVVulkanDeviceContext>();
            (*vulkan).inst = instance.handle().as_raw() as ffi::VkInstance;
            (*vulkan).phys_dev = physical.as_raw() as ffi::VkPhysicalDevice;
            (*vulkan).act_dev = hal.raw_device().handle().as_raw() as ffi::VkDevice;
            (*vulkan).get_proc_addr = std::mem::transmute::<
                vk::PFN_vkGetInstanceProcAddr,
                ffi::PFN_vkGetInstanceProcAddr,
            >(
                shared.entry().static_fn().get_instance_proc_addr
            );
            (*vulkan).device_features.sType =
                vk::StructureType::PHYSICAL_DEVICE_FEATURES_2.as_raw() as _;
            (*vulkan).device_features.pNext =
                (&mut *features as *mut vk::PhysicalDeviceSynchronization2Features<'_>).cast();
            (*vulkan).enabled_dev_extensions = extensions.as_ptr();
            (*vulkan).nb_enabled_dev_extensions = extensions.len() as i32;
            (*vulkan).enabled_inst_extensions = instance_extensions.as_ptr();
            (*vulkan).nb_enabled_inst_extensions = instance_extensions.len() as i32;
            let mut index = 0;
            if let Some(family) = family {
                (*vulkan).qf[index].idx = family as i32;
                (*vulkan).qf[index].num = 1;
                (*vulkan).qf[index].flags = vk::QueueFlags::VIDEO_DECODE_KHR.as_raw();
                for (extension, bit) in [
                    (c"VK_KHR_video_decode_h264", 1),
                    (c"VK_KHR_video_decode_h265", 2),
                    (c"VK_KHR_video_decode_vp9", 8),
                ] {
                    if hal.enabled_device_extensions().contains(&extension) {
                        (*vulkan).qf[index].video_caps |= bit;
                    }
                }
                index += 1;
            }
            (*vulkan).qf[index].idx = compute as i32;
            (*vulkan).qf[index].num = 1;
            (*vulkan).qf[index].flags =
                (vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER).as_raw();
            // Include the rendering family in concurrent-sharing image creation,
            // while reserving FFmpeg compute/transfer submissions for its own
            // queue. The decoder never submits graphics work.
            if compute != graphics {
                index += 1;
                (*vulkan).qf[index].idx = graphics as i32;
                (*vulkan).qf[index].num = 1;
                (*vulkan).qf[index].flags = vk::QueueFlags::GRAPHICS.as_raw();
            } else {
                (*vulkan).qf[index].flags |= vk::QueueFlags::GRAPHICS.as_raw();
            }
            (*vulkan).nb_qf = (index + 1) as i32;
            (*vulkan).lock_queue = Some(lock_queue);
            (*vulkan).unlock_queue = Some(unlock_queue);
            (*context).user_opaque = Box::into_raw(Box::new(DeviceLease {
                _extensions: extensions,
                _instance_extensions: instance_extensions,
                _features: features,
                _timeline: timeline,
                _owner: Arc::clone(&owner),
            }))
            .cast();
            (*context).free = Some(free_device_lease);
            let result = ffi::av_hwdevice_ctx_init(reference);
            drop(hal);
            let device = Self {
                reference,
                vaapi: std::ptr::null_mut(),
                mapped_frames: std::ptr::null_mut(),
                _owner: owner,
            };
            ensure!(
                result >= 0,
                "FFmpeg could not share rendering device: {}",
                ffmpeg_next::Error::from(result)
            );
            Ok(device)
        }
    }

    /// Decode through VA-API on the same DRM render node, importing via DMA-BUF.
    pub(crate) fn new_vaapi(gpu: &GpuContext) -> Result<Self> {
        let mut device = Self::shared(gpu, false)?;
        // SAFETY: immutable physical-device query; the DRM node is derived from
        // this exact adapter rather than a hard-coded card or display server.
        unsafe {
            let hal = device
                ._owner
                .as_hal::<Vulkan>()
                .context("renderer is not Vulkan")?;
            for extension in [
                c"VK_KHR_external_memory_fd",
                c"VK_EXT_external_memory_dma_buf",
                c"VK_EXT_image_drm_format_modifier",
                c"VK_KHR_external_semaphore_fd",
            ] {
                ensure!(
                    hal.enabled_device_extensions().contains(&extension),
                    "GPU lacks {} for DMA-BUF video",
                    extension.to_string_lossy()
                );
            }
            let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
            hal.shared_instance()
                .raw_instance()
                .get_physical_device_properties2(
                    hal.raw_physical_device(),
                    &mut vk::PhysicalDeviceProperties2::default().push_next(&mut drm),
                );
            ensure!(
                drm.has_render != 0,
                "rendering GPU exposes no DRM render node"
            );
            let node = std::ffi::CString::new(format!("/dev/dri/renderD{}", drm.render_minor))?;
            let result = ffi::av_hwdevice_ctx_create(
                &mut device.vaapi,
                ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI,
                node.as_ptr(),
                std::ptr::null_mut(),
                0,
            );
            ensure!(
                result >= 0,
                "VA-API initialization on {} failed: {}",
                node.to_string_lossy(),
                ffmpeg_next::Error::from(result)
            );
        }
        Ok(device)
    }

    /// Identify the active decoder and transfer path for diagnostics.
    pub(crate) fn name(&self) -> &'static str {
        if self.vaapi.is_null() {
            "Vulkan"
        } else {
            "VA-API with same-device DMA-BUF import"
        }
    }

    /// Import a VA-API surface into the renderer's Vulkan device without readback.
    pub(crate) fn map_frame(&mut self, frame: &mut ffmpeg_next::frame::Video) -> Result<()> {
        if self.vaapi.is_null() {
            return Ok(());
        }
        ensure!(
            frame.format() == ffmpeg_next::format::Pixel::VAAPI,
            "VA-API decoder returned a software frame"
        );
        // SAFETY: contexts and mapped frames retain their owning FFmpeg device
        // references. av_hwframe_map imports DMA-BUFs and their synchronization
        // fences; it neither downloads nor copies decoded pixels into CPU RAM.
        unsafe {
            ensure!(
                !(*frame.as_ptr()).hw_frames_ctx.is_null(),
                "VA-API frame has no hardware context"
            );
            if self.mapped_frames.is_null() {
                let source = (*(*frame.as_ptr()).hw_frames_ctx)
                    .data
                    .cast::<ffi::AVHWFramesContext>();
                self.mapped_frames = ffi::av_hwframe_ctx_alloc(self.reference);
                ensure!(
                    !self.mapped_frames.is_null(),
                    "could not allocate mapped video context"
                );
                let context = (*self.mapped_frames).data.cast::<ffi::AVHWFramesContext>();
                (*context).format = ffi::AVPixelFormat::AV_PIX_FMT_VULKAN;
                (*context).sw_format = (*source).sw_format;
                (*context).width = (*source).width;
                (*context).height = (*source).height;
                let result = ffi::av_hwframe_ctx_init(self.mapped_frames);
                ensure!(
                    result >= 0,
                    "initialize DMA-BUF import context: {}",
                    ffmpeg_next::Error::from(result)
                );
            }
            let mut drm = ffmpeg_next::frame::Video::empty();
            (*drm.as_mut_ptr()).format = ffi::AVPixelFormat::AV_PIX_FMT_DRM_PRIME as i32;
            let flags = ffi::AV_HWFRAME_MAP_READ as i32 | ffi::AV_HWFRAME_MAP_DIRECT as i32;
            let result = ffi::av_hwframe_map(drm.as_mut_ptr(), frame.as_ptr(), flags);
            ensure!(
                result >= 0,
                "export VA-API frame as DMA-BUF: {}",
                ffmpeg_next::Error::from(result)
            );
            let mut mapped = ffmpeg_next::frame::Video::empty();
            (*mapped.as_mut_ptr()).format = ffi::AVPixelFormat::AV_PIX_FMT_VULKAN as i32;
            let mut owner = ffmpeg_next::frame::Video::empty();
            (*owner.as_mut_ptr()).hw_frames_ctx = ffi::av_buffer_ref(self.mapped_frames);
            ensure!(
                !(*owner.as_ptr()).hw_frames_ctx.is_null(),
                "could not retain mapped video context"
            );
            // FFmpeg's import execution pool retains the map descriptor, which
            // normally owns this same frames context: context -> pool -> map ->
            // context. Give the descriptor a borrowed context reference instead.
            // The returned AVFrame receives the real owning reference below,
            // so external frames still keep the context alive. On final release,
            // frames_uninit drains the pool while its context is still valid.
            (*mapped.as_mut_ptr()).hw_frames_ctx = ffi::av_buffer_create(
                (*self.mapped_frames).data,
                (*self.mapped_frames).size,
                Some(free_borrowed_frames),
                std::ptr::null_mut(),
                0,
            );
            ensure!(
                !(*mapped.as_ptr()).hw_frames_ctx.is_null(),
                "could not borrow mapped video context"
            );
            let result = ffi::av_hwframe_map(mapped.as_mut_ptr(), drm.as_ptr(), flags);
            ensure!(
                result >= 0,
                "import VA-API frame into renderer: {}",
                ffmpeg_next::Error::from(result)
            );
            std::mem::swap(
                &mut (*mapped.as_mut_ptr()).hw_frames_ctx,
                &mut (*owner.as_mut_ptr()).hw_frames_ctx,
            );
            let result = ffi::av_frame_copy_props(mapped.as_mut_ptr(), frame.as_ptr());
            ensure!(
                result >= 0,
                "preserve mapped frame properties: {}",
                ffmpeg_next::Error::from(result)
            );
            *frame = mapped;
        }
        Ok(())
    }

    /// Attach the hardware device before opening a codec; refuse silent fallback.
    pub(crate) fn configure(&self, context: &mut ffmpeg_next::codec::Context) -> Result<()> {
        // SAFETY: context is exclusively borrowed and not yet opened. FFmpeg
        // owns the new buffer reference and invokes the non-panicking callback.
        unsafe {
            ensure!(
                ffi::avcodec_is_open(context.as_mut_ptr()) == 0,
                "hardware device must be attached before opening the decoder"
            );
            let reference = ffi::av_buffer_ref(if self.vaapi.is_null() {
                self.reference
            } else {
                self.vaapi
            });
            ensure!(
                !reference.is_null(),
                "could not reference Vulkan decode device"
            );
            ffi::av_buffer_unref(&mut (*context.as_mut_ptr()).hw_device_ctx);
            (*context.as_mut_ptr()).hw_device_ctx = reference;
            // Hardware performs the frame decode. CPU frame threading adds
            // decoder surfaces and buffering without helping this paced worker.
            (*context.as_mut_ptr()).thread_count = 1;
            (*context.as_mut_ptr()).get_format = if self.vaapi.is_null() {
                Some(hardware_format)
            } else {
                Some(vaapi_format)
            };
        }
        Ok(())
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: this owns exactly one FFmpeg buffer reference; the Vulkan
        // device remains owned by _owner during FFmpeg destruction.
        unsafe {
            ffi::av_buffer_unref(&mut self.mapped_frames);
            ffi::av_buffer_unref(&mut self.vaapi);
            ffi::av_buffer_unref(&mut self.reference);
        };
    }
}

unsafe extern "C" fn hardware_format(
    _context: *mut ffi::AVCodecContext,
    formats: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    let mut format = formats;
    // SAFETY: FFmpeg provides a sentinel-terminated format array for this call.
    unsafe {
        while *format != ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *format == ffi::AVPixelFormat::AV_PIX_FMT_VULKAN {
                return *format;
            }
            format = format.add(1);
        }
    }
    ffi::AVPixelFormat::AV_PIX_FMT_NONE
}

unsafe extern "C" fn vaapi_format(
    _context: *mut ffi::AVCodecContext,
    formats: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    let mut format = formats;
    // SAFETY: FFmpeg provides a sentinel-terminated format array for this call.
    unsafe {
        while *format != ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *format == ffi::AVPixelFormat::AV_PIX_FMT_VAAPI {
                return *format;
            }
            format = format.add(1);
        }
    }
    ffi::AVPixelFormat::AV_PIX_FMT_NONE
}

/// Validate the first negotiated frame before publishing a hardware source.
pub(crate) fn validate_frame(frame: &ffmpeg_next::frame::Video) -> Result<()> {
    ensure!(
        frame.color_range() != ffmpeg_next::color::Range::JPEG,
        "GPU video conversion currently requires limited-range video"
    );
    ensure!(
        frame.format() == ffmpeg_next::format::Pixel::VULKAN,
        "decoder did not produce Vulkan frames"
    );
    // SAFETY: only FFmpeg-produced frames with the negotiated Vulkan format are
    // inspected; their retained buffer references own these metadata pointers.
    unsafe {
        let raw = frame.as_ptr();
        ensure!(
            !(*raw).hw_frames_ctx.is_null() && !(*raw).data[0].is_null(),
            "missing hardware frame context"
        );
        let context = (*(*raw).hw_frames_ctx)
            .data
            .cast::<ffi::AVHWFramesContext>();
        ensure!(
            (*context).sw_format == ffi::AVPixelFormat::AV_PIX_FMT_NV12,
            "GPU conversion currently requires 8-bit NV12"
        );
        let image = (*raw).data[0].cast::<ffi::AVVkFrame>();
        ensure!(
            (*image).img[0] != 0 && (*image).sem[0] != 0,
            "decoder returned an incomplete GPU image"
        );
        ensure!(
            (*image).img[2] == 0,
            "GPU conversion supports at most two NV12 plane images"
        );
        let count = if (*image).img[1] == 0 { 1 } else { 2 };
        for index in 0..count {
            ensure!(
                (*image).sem[index] != 0,
                "decoder plane has no synchronization semaphore"
            );
            ensure!(
                (*image).queue_family[index] == vk::QUEUE_FAMILY_IGNORED,
                "decoder image does not use concurrent queue-family sharing"
            );
        }
        ensure!(
            count == 1 || (*image).sem[0] != (*image).sem[1],
            "decoder plane semaphores must be independent"
        );
        ensure!(
            (*raw).crop_left == 0 && (*raw).crop_top == 0,
            "GPU conversion does not support a cropped frame origin"
        );
    }
    Ok(())
}

/// Copy decoder planes and convert to the persistent RGBA shader input on GPU.
/// No decoded pixel data crosses CPU memory in this path.
pub(crate) struct Converter {
    gpu: GpuContext,
    planes: [wgpu::Texture; 2],
    pipeline: wgpu::RenderPipeline,
    binding: wgpu::BindGroup,
    output: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl Converter {
    /// Create reusable plane textures and a GPU color-conversion pipeline.
    pub(crate) fn new(gpu: GpuContext, output: &wgpu::Texture, width: u32, height: u32) -> Self {
        let plane = |format, width, height| {
            gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("video-decoder-plane"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
        };
        let planes = [
            plane(wgpu::TextureFormat::R8Unorm, width, height),
            plane(
                wgpu::TextureFormat::Rg8Unorm,
                width.div_ceil(2),
                height.div_ceil(2),
            ),
        ];
        let views = planes
            .each_ref()
            .map(|plane| plane.create_view(&Default::default()));
        // Initialize once through wgpu so its resource tracker knows that later
        // native copies fully replace valid plane contents. No CPU zero upload.
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        for view in &views {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("video-plane-initialize"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        crate::gpu_interop::submit(&gpu.queue, [encoder.finish()]);
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("video-nv12-to-rgba"),
                source: wgpu::ShaderSource::Wgsl(include_str!("nv12.wgsl").into()),
            });
        let pipeline = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("video-nv12-to-rgba"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
        let binding = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("video-nv12-planes"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&views[1]),
                },
            ],
        });
        Self {
            gpu,
            planes,
            pipeline,
            binding,
            output: output.create_view(&wgpu::TextureViewDescriptor {
                format: Some(wgpu::TextureFormat::Rgba8Unorm),
                ..Default::default()
            }),
            width,
            height,
        }
    }

    /// Submit native plane copies and color conversion, waiting only on GPU work.
    pub(crate) fn convert(&self, frame: &mut ffmpeg_next::frame::Video) -> Result<()> {
        validate_frame(frame)?;
        ensure!(
            frame.width() == self.width && frame.height() == self.height,
            "hardware frame dimensions changed"
        );
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        encoder.transition_resources(
            std::iter::empty(),
            self.planes.iter().map(|texture| wgpu::TextureTransition {
                texture,
                selector: None,
                state: wgpu::TextureUses::COPY_DST,
            }),
        );
        let prepare = encoder.finish();
        // Raw and wgpu commands require distinct command encoders. Submit them
        // together so wgpu's transitions bracket the native plane copies.
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        // SAFETY: all handles belong to the same retained Vulkan device. The
        // decoder frame stays owned until its signal completes. Native copies
        // are bracketed by wgpu resource transitions, and the source image's
        // layout is restored. All queue submissions use the shared submit lock.
        unsafe {
            let hal = self
                .gpu
                .device
                .as_hal::<Vulkan>()
                .context("renderer is not Vulkan")?;
            let device = hal.raw_device();
            let context = (*(*frame.as_ptr()).hw_frames_ctx)
                .data
                .cast::<ffi::AVHWFramesContext>();
            let frames = (*context).hwctx.cast::<ffi::AVVulkanFramesContext>();
            let image = (*frame.as_ptr()).data[0].cast::<ffi::AVVkFrame>();
            // FFmpeg's lock covers metadata reads through queue submission.
            let lock = (*frames)
                .lock_frame
                .context("Vulkan frame lock is missing")?;
            let unlock = (*frames)
                .unlock_frame
                .context("Vulkan frame unlock is missing")?;
            lock(context, image);
            let result = (|| -> Result<(Vec<vk::Semaphore>, Vec<u64>)> {
                let count = if (*image).img[1] == 0 { 1 } else { 2 };
                let semaphores: Vec<_> = (&(*image).sem)[..count]
                    .iter()
                    .map(|value| vk::Semaphore::from_raw(*value))
                    .collect();
                let values = (&(*image).sem_value)[..count].to_vec();
                let signals: Vec<_> = values
                    .iter()
                    .map(|value| value.checked_add(1).context("video timeline overflow"))
                    .collect::<Result<_>>()?;
                let sources: Vec<_> = (&(*image).img)[..count]
                    .iter()
                    .map(|value| vk::Image::from_raw(*value))
                    .collect();
                let layouts: Vec<_> = (&(*image).layout)[..count]
                    .iter()
                    .map(|value| vk::ImageLayout::from_raw(*value))
                    .collect();
                let destinations: Vec<_> = self
                    .planes
                    .iter()
                    .map(|texture| {
                        texture
                            .as_hal::<Vulkan>()
                            .map(|texture| texture.raw_handle())
                            .context("plane is not Vulkan")
                    })
                    .collect::<Result<_>>()?;
                encoder.as_hal_mut::<Vulkan, _, _>(|raw| {
                    let raw = raw.expect("Vulkan encoder").raw_handle();
                    let range = vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1);
                    let before: Vec<_> = sources
                        .iter()
                        .zip(&layouts)
                        .map(|(source, layout)| {
                            vk::ImageMemoryBarrier::default()
                                .old_layout(*layout)
                                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                                .image(*source)
                                .subresource_range(range)
                        })
                        .collect();
                    device.cmd_pipeline_barrier(
                        raw,
                        vk::PipelineStageFlags::TOP_OF_PIPE,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &before,
                    );
                    for (index, destination) in destinations.iter().enumerate() {
                        let (width, height, aspect) = if index == 0 {
                            (self.width, self.height, vk::ImageAspectFlags::PLANE_0)
                        } else {
                            (
                                self.width.div_ceil(2),
                                self.height.div_ceil(2),
                                vk::ImageAspectFlags::PLANE_1,
                            )
                        };
                        let aspect = if count == 1 {
                            aspect
                        } else {
                            vk::ImageAspectFlags::COLOR
                        };
                        let source = sources[if count == 1 { 0 } else { index }];
                        let region = vk::ImageCopy::default()
                            .src_subresource(
                                vk::ImageSubresourceLayers::default()
                                    .aspect_mask(aspect)
                                    .layer_count(1),
                            )
                            .dst_subresource(
                                vk::ImageSubresourceLayers::default()
                                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                                    .layer_count(1),
                            )
                            .extent(vk::Extent3D {
                                width,
                                height,
                                depth: 1,
                            });
                        device.cmd_copy_image(
                            raw,
                            source,
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            *destination,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            &[region],
                        );
                    }
                    let after: Vec<_> = sources
                        .iter()
                        .zip(&layouts)
                        .map(|(source, layout)| {
                            vk::ImageMemoryBarrier::default()
                                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                                .new_layout(*layout)
                                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                                .image(*source)
                                .subresource_range(range)
                        })
                        .collect();
                    device.cmd_pipeline_barrier(
                        raw,
                        vk::PipelineStageFlags::TRANSFER,
                        vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                        vk::DependencyFlags::empty(),
                        &[],
                        &[],
                        &after,
                    );
                });
                let copy = encoder.finish();
                let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("video-gpu-color-conversion"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.output,
                            resolve_target: None,
                            depth_slice: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &self.binding, &[]);
                    pass.draw(0..3, 0..1);
                }
                let command = encoder.finish();
                crate::gpu_interop::with_submission(|| -> Result<()> {
                    let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; count];
                    let mut timeline =
                        vk::TimelineSemaphoreSubmitInfo::default().wait_semaphore_values(&values);
                    let wait = vk::SubmitInfo::default()
                        .push_next(&mut timeline)
                        .wait_semaphores(&semaphores)
                        .wait_dst_stage_mask(&stages);
                    device.queue_submit(hal.raw_queue(), &[wait], vk::Fence::null())?;
                    let queue = self
                        .gpu
                        .queue
                        .as_hal::<Vulkan>()
                        .context("queue is not Vulkan")?;
                    for (semaphore, signal) in semaphores.iter().zip(&signals) {
                        queue.add_signal_semaphore(*semaphore, Some(*signal));
                    }
                    self.gpu.queue.submit([prepare, copy, command]);
                    Ok(())
                })?;
                (&mut (*image).sem_value)[..count].copy_from_slice(&signals);
                Ok((semaphores, signals))
            })();
            unlock(context, image);
            let (semaphores, signals) = result?;
            // Bounded backpressure: do not let FFmpeg reuse a decoder surface
            // until the GPU conversion finished. This never maps pixel memory.
            device.wait_semaphores(
                &vk::SemaphoreWaitInfo::default()
                    .semaphores(&semaphores)
                    .values(&signals),
                5_000_000_000,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a Vulkan video-capable GPU and KROMA_PROFILE_VIDEO"]
    fn decode_on_rendering_device() {
        ffmpeg_next::init().unwrap();
        ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Warning);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = crate::renderer::pollster_block(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                ..Default::default()
            },
        ))
        .unwrap();
        eprintln!("shared video GPU: {:?}", adapter.get_info());
        let (device, queue, queues) =
            crate::gpu_interop::request_device(&adapter, &wgpu::DeviceDescriptor::default())
                .unwrap();
        let gpu = GpuContext {
            device: Arc::new(device),
            queue: Arc::new(queue),
            video_queues: Some(Arc::new(queues)),
            video_decode: crate::config::VideoDecodeMode::Auto,
            surface_format: wgpu::TextureFormat::Rgba8UnormSrgb,
        };
        let mut hardware = if std::env::var("KROMA_TEST_BACKEND").as_deref() == Ok("vaapi") {
            Device::new_vaapi(&gpu).unwrap()
        } else {
            Device::new(&gpu).unwrap()
        };
        let mut input =
            ffmpeg_next::format::input(&std::env::var("KROMA_PROFILE_VIDEO").unwrap()).unwrap();
        let stream = input
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .unwrap();
        let index = stream.index();
        let mut context =
            ffmpeg_next::codec::Context::from_parameters(stream.parameters()).unwrap();
        hardware.configure(&mut context).unwrap();
        let mut decoder = context.decoder().video().unwrap();
        let mut frame = ffmpeg_next::frame::Video::empty();
        for (stream, packet) in input.packets() {
            if stream.index() != index {
                continue;
            }
            decoder.send_packet(&packet).unwrap();
            if decoder.receive_frame(&mut frame).is_ok() {
                hardware.map_frame(&mut frame).unwrap();
                assert_eq!(frame.format(), ffmpeg_next::format::Pixel::VULKAN);
                // SAFETY: the preceding mapping produced a retained Vulkan frame.
                unsafe {
                    let image = (*frame.as_ptr()).data[0].cast::<ffi::AVVkFrame>();
                    eprintln!(
                        "mapped images {:?}, queue {:?}, layout {:?}",
                        (*image).img,
                        (*image).queue_family,
                        (*image).layout
                    );
                }
                eprintln!(
                    "hardware frame {}x{} on renderer VkDevice",
                    frame.width(),
                    frame.height()
                );
                check_converted_frame(&gpu, &mut frame);
                drop(frame);
                drop(decoder);
                if !hardware.mapped_frames.is_null() {
                    // SAFETY: the hardware device still owns the context. No
                    // external frame remains; import dependencies must not own it.
                    unsafe {
                        assert_eq!(
                            ffi::av_buffer_get_ref_count(hardware.mapped_frames),
                            1,
                            "DMA-BUF import context must not retain itself"
                        );
                    }
                }
                return;
            }
        }
        panic!("no hardware frame decoded");
    }

    fn check_converted_frame(gpu: &GpuContext, frame: &mut ffmpeg_next::frame::Video) {
        use kroma_shared::traits::VideoDecoder;
        let path = std::env::var("KROMA_PROFILE_VIDEO").unwrap();
        let mut software =
            super::super::FfmpegVideoDecoder::load(std::path::Path::new(&path)).unwrap();
        let expected = software.next_frame().unwrap().to_vec();
        let width = frame.width();
        let height = frame.height();
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hardware-video-check"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[wgpu::TextureFormat::Rgba8Unorm],
        });
        Converter::new(gpu.clone(), &target, width, height)
            .convert(frame)
            .unwrap();
        let padded = (width * 4).div_ceil(256) * 256;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        crate::gpu_interop::submit(&gpu.queue, [encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = buffer.get_mapped_range(..);
        let mut actual = Vec::new();
        super::super::copy_rgba_rows(
            &mapped,
            padded as usize,
            width as usize,
            height as usize,
            &mut actual,
        )
        .unwrap();
        let mut max = 0u8;
        let mut total = 0u64;
        let mut mismatches = 0;
        for (a, b) in actual.iter().zip(&expected) {
            let difference = a.abs_diff(*b);
            max = max.max(difference);
            total += u64::from(difference);
            mismatches += usize::from(difference != 0);
        }
        eprintln!(
            "GPU_COLOR_CHECK {}",
            serde_json::json!({"path":path,"width":width,"height":height,
            "bytes":actual.len(),"max_difference":max,"mean_difference":total as f64 / actual.len() as f64,
            "different_components":mismatches})
        );
        if let Ok(directory) = std::env::var("KROMA_GPU_CHECK_OUTPUT") {
            let directory = std::path::Path::new(&directory);
            std::fs::write(directory.join("expected.rgba"), &expected).unwrap();
            std::fs::write(directory.join("actual.rgba"), &actual).unwrap();
        }
        assert_eq!(
            actual, expected,
            "GPU decode/conversion must preserve the software renderer's RGBA bytes"
        );
    }
}
