//! Synchronize wgpu submissions with native video semaphore submissions.

use std::sync::{Mutex, MutexGuard};

static SUBMISSION: Mutex<()> = Mutex::new(());

/// Submit rendering work without racing native video queue synchronization.
pub(crate) fn submit(
    queue: &wgpu::Queue,
    commands: impl IntoIterator<Item = wgpu::CommandBuffer>,
) -> wgpu::SubmissionIndex {
    with_submission(|| queue.submit(commands))
}

/// Keep a native wait, wgpu signal registration, and its submission together.
pub(crate) fn with_submission<T>(operation: impl FnOnce() -> T) -> T {
    let _guard = lock_submission();
    operation()
}

/// Acquire the same guard for FFmpeg's native queue lock/unlock callbacks.
pub(crate) fn lock_submission() -> MutexGuard<'static, ()> {
    SUBMISSION.lock().unwrap_or_else(|error| error.into_inner())
}

use anyhow::{Context, Result, ensure};
use ash::vk;
use std::ffi::CStr;
type Vulkan = wgpu::hal::api::Vulkan;

/// Queue capabilities provisioned on one specific native rendering device.
pub(crate) struct VideoQueues {
    device: vk::Device,
    _owner: wgpu::Device,
    video: Option<u32>,
    compute: u32,
    graphics: u32,
}

impl VideoQueues {
    /// Check ownership before lending queue indices to another API.
    pub(crate) fn for_device(&self, device: vk::Device) -> Result<(Option<u32>, u32, u32)> {
        ensure!(
            self.device == device,
            "video queues belong to another rendering device"
        );
        Ok((self.video, self.compute, self.graphics))
    }
}

fn video_extensions() -> [&'static CStr; 10] {
    [
        c"VK_KHR_video_queue",
        c"VK_KHR_video_decode_queue",
        c"VK_KHR_video_decode_h264",
        c"VK_KHR_video_decode_h265",
        c"VK_KHR_video_decode_vp9",
        c"VK_KHR_synchronization2",
        c"VK_KHR_external_memory_fd",
        c"VK_EXT_external_memory_dma_buf",
        c"VK_EXT_image_drm_format_modifier",
        c"VK_KHR_external_semaphore_fd",
    ]
}

pub(crate) fn video_family(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Result<u32> {
    // SAFETY: physical belongs to this live Vulkan instance; querying properties
    // does not change device state or access application-owned GPU memory.
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
    families
        .iter()
        .position(|family| {
            family
                .queue_flags
                .contains(vk::QueueFlags::VIDEO_DECODE_KHR)
        })
        .map(|index| index as u32)
        .context("rendering GPU has no Vulkan video decode queue")
}

pub(crate) fn compute_family(
    instance: &ash::Instance,
    physical: vk::PhysicalDevice,
) -> Result<u32> {
    // SAFETY: immutable capability query on the retained physical device.
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
    families
        .iter()
        .position(|family| {
            family.queue_flags.contains(vk::QueueFlags::COMPUTE)
                && !family.queue_flags.contains(vk::QueueFlags::GRAPHICS)
        })
        .or_else(|| {
            families
                .iter()
                .position(|family| family.queue_flags.contains(vk::QueueFlags::COMPUTE))
        })
        .map(|index| index as u32)
        .context("rendering GPU has no compute queue for video interop")
}

/// Create a wgpu device with video import and available decoding queues.
/// No second adapter or cross-GPU transfer is introduced.
pub(crate) fn request_device(
    adapter: &wgpu::Adapter,
    descriptor: &wgpu::DeviceDescriptor<'_>,
) -> Result<(wgpu::Device, wgpu::Queue, VideoQueues)> {
    // SAFETY: HAL access is scoped to device creation; no native handle escapes
    // without ownership retained by wgpu. Features/queues are only added.
    unsafe {
        let hal = adapter
            .as_hal::<Vulkan>()
            .context("renderer is not Vulkan")?;
        let instance = hal.shared_instance().raw_instance();
        let physical = hal.raw_physical_device();
        ensure!(
            hal.shared_instance().instance_api_version() >= vk::API_VERSION_1_3
                && instance
                    .get_physical_device_properties(physical)
                    .api_version
                    >= vk::API_VERSION_1_3,
            "FFmpeg video interop requires Vulkan 1.3"
        );
        let queue_family = video_family(instance, physical).ok();
        let compute_family = compute_family(instance, physical)?;
        let supported = instance.enumerate_device_extension_properties(physical)?;
        let available = |name: &CStr| {
            supported
                .iter()
                .any(|extension| CStr::from_ptr(extension.extension_name.as_ptr()) == name)
        };
        ensure!(
            available(c"VK_KHR_video_decode_queue")
                || [
                    c"VK_KHR_external_memory_fd",
                    c"VK_EXT_external_memory_dma_buf",
                    c"VK_EXT_image_drm_format_modifier",
                    c"VK_KHR_external_semaphore_fd"
                ]
                .into_iter()
                .all(available),
            "GPU lacks native video decoding and DMA-BUF import"
        );
        let extensions: Vec<_> = video_extensions()
            .into_iter()
            .filter(|name| available(name))
            .collect();
        let mut synchronization = vk::PhysicalDeviceSynchronization2Features::default();
        instance.get_physical_device_features2(
            physical,
            &mut vk::PhysicalDeviceFeatures2::default().push_next(&mut synchronization),
        );
        ensure!(
            synchronization.synchronization2 != 0,
            "GPU lacks synchronization2"
        );
        let priorities = [1.0_f32];
        let device = hal.open_with_callback(
            descriptor.required_features,
            &descriptor.required_limits,
            &descriptor.memory_hints,
            Some(Box::new(|args| {
                for extension in extensions {
                    if !args.extensions.contains(&extension) {
                        args.extensions.push(extension);
                    }
                }
                for family in queue_family
                    .into_iter()
                    .chain(std::iter::once(compute_family))
                {
                    if !args
                        .queue_create_infos
                        .iter()
                        .any(|queue| queue.queue_family_index == family)
                    {
                        args.queue_create_infos.push(
                            vk::DeviceQueueCreateInfo::default()
                                .queue_family_index(family)
                                .queue_priorities(&priorities),
                        );
                    }
                }
                *args.create_info = (*args.create_info).push_next(&mut synchronization);
            })),
        )?;
        let native_device = device.device.raw_device().handle();
        let graphics = device.device.queue_family_index();
        let (device, queue) = adapter
            .create_device_from_hal(device, descriptor)
            .map_err(anyhow::Error::from)?;
        let queues = VideoQueues {
            device: native_device,
            _owner: device.clone(),
            video: queue_family,
            compute: compute_family,
            graphics,
        };
        Ok((device, queue, queues))
    }
}
