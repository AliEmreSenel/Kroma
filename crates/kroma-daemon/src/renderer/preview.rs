//! Full-color preview capture of the same final canvas used for wallpaper output.

use super::*;
use std::time::Duration;

/// Reusable readback resources allocated only for explicit preview jobs.
pub(crate) struct PreviewCapture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    stride: u32,
}

impl Renderer {
    /// Create a full-color capture target without altering normal lighting capture.
    pub(crate) fn create_preview_capture(&self, width: u32, height: u32) -> Result<PreviewCapture> {
        let device = self.gpu.device.as_ref().context("GPU not initialized")?;
        let limit = device.limits().max_texture_dimension_2d;
        anyhow::ensure!(
            width > 0 && height > 0 && width <= limit && height <= limit,
            "Invalid preview size"
        );
        let stride = (width * 4).div_ceil(256) * 256;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("kroma-preview"),
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
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("kroma-preview-readback"),
            size: u64::from(stride) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(PreviewCapture {
            texture,
            view,
            buffer,
            width,
            height,
            stride,
        })
    }

    /// Read the composited final canvas, including transitions and nested feedback.
    pub(crate) fn capture_preview(&self, capture: &PreviewCapture) -> Result<Vec<u8>> {
        let target = self
            .gpu
            .targets
            .values()
            .next()
            .context("No preview output")?;
        let device = self.gpu.device.as_ref().context("GPU not initialized")?;
        let queue = self
            .gpu
            .queue
            .as_ref()
            .context("GPU queue not initialized")?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("kroma-preview-blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &capture.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&target.output.capture_pipeline);
            pass.set_bind_group(0, &target.output.output_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &capture.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &capture.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(capture.stride),
                    rows_per_image: Some(capture.height),
                },
            },
            wgpu::Extent3d {
                width: capture.width,
                height: capture.height,
                depth_or_array_layers: 1,
            },
        );
        crate::gpu_interop::submit(queue, [encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        capture
            .buffer
            .map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = tx.send(result);
            });
        device.poll(wgpu::wgt::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(30)),
        })?;
        rx.recv_timeout(Duration::from_secs(30))??;
        let mapped = capture.buffer.get_mapped_range(..);
        let row_bytes = (capture.width * 4) as usize;
        let mut rgba = Vec::with_capacity(row_bytes * capture.height as usize);
        for row in mapped
            .chunks(capture.stride as usize)
            .take(capture.height as usize)
        {
            rgba.extend_from_slice(&row[..row_bytes]);
        }
        drop(mapped);
        capture.buffer.unmap();
        Ok(rgba)
    }
}
