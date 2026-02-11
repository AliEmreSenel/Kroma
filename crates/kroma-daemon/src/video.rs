//! Video decoder implementations for Kroma.
//!
//! Uses FFmpeg via `ffmpeg-next` for real video decoding.

#![allow(dead_code)]

use std::path::Path;

use anyhow::Result;

use kroma_shared::traits::VideoDecoder;

// ---------------------------------------------------------------------------
// FFmpeg video decoder
// ---------------------------------------------------------------------------

pub struct FfmpegVideoDecoder {
    input: ffmpeg_next::format::context::Input,
    decoder: ffmpeg_next::decoder::Video,
    video_stream_index: usize,
    scaler: ffmpeg_next::software::scaling::Context,
    width: u32,
    height: u32,
    frame_buffer: Vec<u8>,
    duration: f64,
    time_base: f64,
}

// SAFETY: FfmpegVideoDecoder is only used on the main render thread.
// The raw pointers in ffmpeg types are not shared across threads.
unsafe impl Send for FfmpegVideoDecoder {}

impl FfmpegVideoDecoder {
    /// Try to decode one video frame from the input stream.
    fn decode_next_packet(&mut self) -> Option<()> {
        loop {
            // Try to receive a decoded frame first
            let mut decoded = ffmpeg_next::util::frame::Video::empty();
            if self.decoder.receive_frame(&mut decoded).is_ok() {
                return self.scale_frame(&decoded);
            }

            // Send next packet to the decoder
            let mut found_video = false;
            for (stream, packet) in self.input.packets() {
                if stream.index() == self.video_stream_index {
                    if self.decoder.send_packet(&packet).is_err() {
                        continue;
                    }
                    found_video = true;
                    break;
                }
            }

            if !found_video {
                // End of stream — flush decoder
                let _ = self.decoder.send_eof();
                let mut decoded = ffmpeg_next::util::frame::Video::empty();
                if self.decoder.receive_frame(&mut decoded).is_ok() {
                    return self.scale_frame(&decoded);
                }
                return None;
            }
        }
    }

    /// Scale a decoded frame to RGBA and store in `frame_buffer`.
    fn scale_frame(&mut self, decoded: &ffmpeg_next::util::frame::Video) -> Option<()> {
        let mut rgba_frame = ffmpeg_next::util::frame::Video::empty();
        self.scaler.run(decoded, &mut rgba_frame).ok()?;

        let data = rgba_frame.data(0);
        let stride = rgba_frame.stride(0);
        let w = self.width as usize;
        let h = self.height as usize;

        self.frame_buffer.resize(w * h * 4, 0);
        for y in 0..h {
            let src_start = y * stride;
            let dst_start = y * w * 4;
            let row_bytes = w * 4;
            if src_start + row_bytes <= data.len()
                && dst_start + row_bytes <= self.frame_buffer.len()
            {
                self.frame_buffer[dst_start..dst_start + row_bytes]
                    .copy_from_slice(&data[src_start..src_start + row_bytes]);
            }
        }
        Some(())
    }
}

impl VideoDecoder for FfmpegVideoDecoder {
    fn load(path: &Path) -> Result<Self>
    where
        Self: Sized,
    {
        ffmpeg_next::init()
            .map_err(|e| anyhow::anyhow!("Failed to initialize FFmpeg: {}", e))?;

        let input = ffmpeg_next::format::input(&path)
            .map_err(|e| anyhow::anyhow!("Failed to open video '{}': {}", path.display(), e))?;

        let video_stream = input
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .ok_or_else(|| anyhow::anyhow!("No video stream found in '{}'", path.display()))?;

        let video_stream_index = video_stream.index();
        let time_base = f64::from(video_stream.time_base());
        let duration = video_stream.duration() as f64 * time_base;

        let context_decoder =
            ffmpeg_next::codec::context::Context::from_parameters(video_stream.parameters())?;
        let decoder = context_decoder.decoder().video()?;

        let width = decoder.width();
        let height = decoder.height();

        let scaler = ffmpeg_next::software::scaling::Context::get(
            decoder.format(),
            width,
            height,
            ffmpeg_next::format::Pixel::RGBA,
            width,
            height,
            ffmpeg_next::software::scaling::Flags::BILINEAR,
        )?;

        log::info!(
            "FFmpeg video decoder: {}x{}, {:.1}s duration, stream {}",
            width, height, duration, video_stream_index
        );

        Ok(Self {
            input,
            decoder,
            video_stream_index,
            scaler,
            width,
            height,
            frame_buffer: vec![0u8; (width * height * 4) as usize],
            duration,
            time_base,
        })
    }

    fn next_frame(&mut self) -> Option<&[u8]> {
        self.decode_next_packet()?;
        Some(&self.frame_buffer)
    }

    fn seek(&mut self, timestamp: f64) -> Result<()> {
        let ts = (timestamp / self.time_base) as i64;
        self.input
            .seek(ts, ..ts)
            .map_err(|e| anyhow::anyhow!("Seek failed: {}", e))?;
        self.decoder.flush();
        Ok(())
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// The video decoder type used by the daemon (FFmpeg-based).
pub type DefaultVideoDecoder = FfmpegVideoDecoder;
