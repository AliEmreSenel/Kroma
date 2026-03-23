use super::*;
use wgpu::wgt::PollType;

/// Renders to an offscreen target and returns a JPEG-encoded preview frame.
///
/// This path temporarily overrides `u_resolution`, restores frame uniforms
/// afterwards, and converts readback pixels to RGB for JPEG encoding.
pub(super) fn capture_preview_frame(
    state: &mut Renderer,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let device = state
        .gpu
        .device
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No GPU device"))?;
    let queue = state
        .gpu
        .queue
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No GPU queue"))?;

    let w = width.max(1);
    let h = height.max(1);
    let format = state.active_surface_format();

    let (tex, tex_view) =
        Renderer::create_offscreen_render_target(device, w, h, format, "preview-capture");

    let bytes_per_pixel = 4u32;
    let unpadded_bytes_per_row = w * bytes_per_pixel;
    let padded_bytes_per_row = (unpadded_bytes_per_row + 255) & !255;

    let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("preview-readback"),
        size: (padded_bytes_per_row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    state.upload_preview_uniforms(w, h);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("preview-capture-encoder"),
    });

    state.encode_fullscreen_pass(&mut encoder, &tex_view, "preview-render-pass", true)?;

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

    let buffer_slice = output_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = device.poll(PollType::wait_indefinitely())?;

    rx.recv()
        .map_err(|_| anyhow::anyhow!("Buffer map channel closed"))?
        .map_err(|e| anyhow::anyhow!("Buffer map failed: {:?}", e))?;

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
                rgba.push(pixel[2]);
                rgba.push(pixel[1]);
                rgba.push(pixel[0]);
            } else {
                rgba.push(pixel[0]);
                rgba.push(pixel[1]);
                rgba.push(pixel[2]);
            }
            rgba.push(pixel[3]);
        }
    }
    drop(data);
    output_buffer.unmap();

    state.upload_frame_uniforms();

    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for pixel in rgba.chunks_exact(4) {
        rgb.push(pixel[0]);
        rgb.push(pixel[1]);
        rgb.push(pixel[2]);
    }
    let img = image::RgbImage::from_raw(w, h, rgb)
        .ok_or_else(|| anyhow::anyhow!("Failed to create image from pixels"))?;
    let mut jpeg_bytes = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut jpeg_bytes);
    img.write_to(&mut cursor, image::ImageFormat::Jpeg)
        .context("JPEG encode failed")?;

    Ok(jpeg_bytes)
}
