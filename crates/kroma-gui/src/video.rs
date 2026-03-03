//! Video preview playback — manages frame-by-frame decoding for the asset preview panel.
//!
//! Uses a persistent decoder to avoid re-opening the video file on every tick.

use std::path::PathBuf;

/// Persistent video player — keeps the ffmpeg decoder/scaler alive between ticks.
pub struct VideoPlayerState {
    /// Path to the temp file being played.
    pub temp_path: PathBuf,
    /// Name of the asset being played.
    pub asset_name: String,
    /// Current decoded frame (width, height, RGBA pixels).
    pub current_frame: Option<(u32, u32, Vec<u8>)>,
    /// Total duration in seconds.
    pub duration: f64,
    /// Current playback position in seconds.
    pub position: f64,
    /// Whether playback is active.
    pub playing: bool,
    /// Frame rate (fps) of the video.
    #[allow(dead_code)]
    pub fps: f64,
    /// Video width.
    pub width: u32,
    /// Video height.
    pub height: u32,
    /// Format info string (e.g. "MP4 | H.264 | 1920x1080 | 30fps").
    pub format_info: String,
    /// Persistent ffmpeg input context.
    input_ctx: ffmpeg_next::format::context::Input,
    /// Persistent video decoder.
    decoder: ffmpeg_next::decoder::Video,
    /// Persistent pixel format scaler (to RGBA).
    scaler: ffmpeg_next::software::scaling::Context,
    /// Video stream index in the container.
    stream_index: usize,
    /// Time base for PTS → seconds conversion.
    time_base: f64,
}

impl VideoPlayerState {
    /// Open a video file and create a persistent player.
    pub fn open(
        temp_path: PathBuf,
        asset_name: String,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let _ = ffmpeg_next::init();

        let input_ctx = ffmpeg_next::format::input(&temp_path)?;

        let video_stream = input_ctx
            .streams()
            .best(ffmpeg_next::media::Type::Video)
            .ok_or("No video stream found")?;

        let stream_index = video_stream.index();
        let time_base = f64::from(video_stream.time_base());
        let raw_duration = video_stream.duration();
        let duration = if raw_duration > 0 {
            raw_duration as f64 * time_base
        } else {
            let ctx_dur = input_ctx.duration();
            if ctx_dur > 0 {
                ctx_dur as f64 / f64::from(ffmpeg_next::ffi::AV_TIME_BASE)
            } else {
                10.0 // fallback
            }
        };

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

        // Estimate fps from stream (fallback to 30)
        let fps = {
            let r = video_stream.avg_frame_rate();
            if r.1 > 0 {
                r.0 as f64 / r.1 as f64
            } else {
                30.0
            }
        };

        // Build format info string from container format and codec
        let container = {
            temp_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("?")
                .to_uppercase()
        };
        let codec_name = decoder
            .codec()
            .map(|c| c.name().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let format_info = format!(
            "{} | {} | {}x{} | {:.0}fps",
            container, codec_name, width, height, fps
        );

        Ok(Self {
            temp_path,
            asset_name,
            current_frame: None,
            duration,
            position: 0.0,
            playing: true,
            fps,
            width,
            height,
            format_info,
            input_ctx,
            decoder,
            scaler,
            stream_index,
            time_base,
        })
    }

    /// Advance to the next frame. Call this on each video tick.
    /// Returns true if a new frame was decoded.
    pub fn advance_frame(&mut self) -> bool {
        // Try to decode the next frame from already-buffered packets
        {
            let mut decoded = ffmpeg_next::util::frame::Video::empty();
            if self.decoder.receive_frame(&mut decoded).is_ok() {
                return self.convert_frame(&decoded);
            }
        }

        // Need to feed more packets — read one at a time to avoid borrow conflicts.
        // `packets().next()` borrows input_ctx; we extract owned data before dropping the borrow.
        for _ in 0..200 {
            let packet_result = {
                let mut iter = self.input_ctx.packets();
                iter.next().map(|(stream, packet)| (stream.index(), packet))
            };

            match packet_result {
                Some((stream_index, packet)) => {
                    if stream_index != self.stream_index {
                        continue;
                    }
                    if self.decoder.send_packet(&packet).is_ok() {
                        let mut decoded = ffmpeg_next::util::frame::Video::empty();
                        if self.decoder.receive_frame(&mut decoded).is_ok() {
                            let pts = decoded.pts().unwrap_or(0) as f64 * self.time_base;
                            self.position = pts;
                            return self.convert_frame(&decoded);
                        }
                    }
                }
                None => {
                    // End of file — loop back to beginning
                    self.position = 0.0;
                    let _ = self.input_ctx.seek(0, 0..i64::MAX);
                    self.decoder.flush();
                    return false;
                }
            }
        }
        false
    }

    /// Convert a decoded frame to RGBA and store as current_frame.
    fn convert_frame(&mut self, decoded: &ffmpeg_next::util::frame::Video) -> bool {
        let mut rgba_frame = ffmpeg_next::util::frame::Video::empty();
        if self.scaler.run(decoded, &mut rgba_frame).is_err() {
            return false;
        }

        let data = rgba_frame.data(0);
        let stride = rgba_frame.stride(0);
        let w = self.width as usize;
        let h = self.height as usize;
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            let src_start = y * stride;
            let dst_start = y * w * 4;
            let row_bytes = w * 4;
            if src_start + row_bytes <= data.len() && dst_start + row_bytes <= buf.len() {
                buf[dst_start..dst_start + row_bytes]
                    .copy_from_slice(&data[src_start..src_start + row_bytes]);
            }
        }

        let pts = decoded.pts().unwrap_or(0) as f64 * self.time_base;
        self.position = pts;
        self.current_frame = Some((self.width, self.height, buf));
        true
    }

    /// Seek to a specific position (0.0 - 1.0 of duration).
    pub fn seek_to(&mut self, fraction: f32) {
        let target_secs = fraction as f64 * self.duration;
        // Convert to stream time base for seeking
        let seek_ts = (target_secs / self.time_base) as i64;
        // Seek backward to nearest keyframe, then decode forward
        let _ = self.input_ctx.seek(seek_ts, 0..seek_ts);
        self.decoder.flush();
        self.position = target_secs;
        // Decode frames forward until we reach (or pass) the target position
        for _ in 0..300 {
            let packet_result = {
                let mut iter = self.input_ctx.packets();
                iter.next().map(|(stream, packet)| (stream.index(), packet))
            };
            match packet_result {
                Some((stream_index, packet)) => {
                    if stream_index != self.stream_index {
                        continue;
                    }
                    if self.decoder.send_packet(&packet).is_ok() {
                        let mut decoded = ffmpeg_next::util::frame::Video::empty();
                        if self.decoder.receive_frame(&mut decoded).is_ok() {
                            let pts = decoded.pts().unwrap_or(0) as f64 * self.time_base;
                            self.position = pts;
                            self.convert_frame(&decoded);
                            // Found a frame at or past target — good enough
                            if pts >= target_secs - 0.1 {
                                return;
                            }
                        }
                    }
                }
                None => {
                    // EOF while seeking
                    return;
                }
            }
        }
    }

    /// Clean up temp file on drop.
    pub fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.temp_path);
    }
}
