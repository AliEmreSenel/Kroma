//! Video texture source.
//!
//! Wraps an FFmpeg decoder and produces RGBA frames at the video's
//! native frame rate. Handles accumulator-based timing, looping, and
//! hot-reload for external disk-backed files.

use std::path::Path;

use anyhow::anyhow;
use anyhow::{Context, Result};
use ffmpeg_next::codec;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::frame;
use ffmpeg_next::media;
use ffmpeg_next::software::scaling;
use log::{info, warn};

use kroma_shared::traits::VideoDecoder;

use super::ffmpeg_io;
use super::hot_reload::SourceHotReload;
use super::{TextureSource, TextureUpdate, VideoSource};

use kroma_shared::shade::AssetByteStream;

pub struct FfmpegVideoDecoder {
    input: ffmpeg_io::InputContext,
    decoder: ffmpeg_next::decoder::Video,
    video_stream_index: usize,
    scaler: ffmpeg_next::software::scaling::Context,
    width: u32,
    height: u32,
    frame_buffer: Vec<u8>,
    time_base: f64,
    frame_interval: f64,     // Now dynamic: updates per frame
    avg_frame_interval: f64, // Fallback: calculated from stream average FPS
}

impl FfmpegVideoDecoder {
    fn from_input(input: ffmpeg_io::InputContext, source_label: &str) -> Result<Self> {
        let video_stream = input
            .as_input()
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| anyhow!("No video stream found in '{}'", source_label))?;

        let video_stream_index = video_stream.index();
        let time_base = f64::from(video_stream.time_base());
        let raw_duration = video_stream.duration();
        let duration = if raw_duration <= 0 {
            0.0
        } else {
            raw_duration as f64 * time_base
        };

        let stream_parameters = video_stream.parameters();

        let context_decoder = codec::Context::from_parameters(stream_parameters)?;
        let decoder = context_decoder.decoder().video()?;

        let width = decoder.width();
        let height = decoder.height();

        let frame_rate = video_stream.rate();
        let avg_frame_interval = frame_rate.denominator() as f64 / frame_rate.numerator() as f64;

        let scaler = scaling::Context::get(
            decoder.format(),
            width,
            height,
            Pixel::RGBA,
            width,
            height,
            scaling::Flags::BILINEAR,
        )?;

        info!(
            "FFmpeg video decoder: {}x{}, {:.1}s duration, {} FPS (avg), stream {}",
            width, height, duration, frame_rate, video_stream_index
        );

        Ok(Self {
            input,
            decoder,
            video_stream_index,
            scaler,
            width,
            height,
            frame_buffer: vec![0u8; (width * height * 4) as usize],
            time_base,
            frame_interval: avg_frame_interval,
            avg_frame_interval,
        })
    }

    pub fn load_from_stream(stream: AssetByteStream, source_label: &str) -> Result<Self> {
        ffmpeg_next::init().map_err(|e| anyhow::anyhow!("Failed to initialize FFmpeg: {}", e))?;
        let input = ffmpeg_io::InputContext::open_from_stream(stream)
            .with_context(|| format!("Failed to open embedded stream for '{}'", source_label))?;
        Self::from_input(input, source_label)
    }

    /// Try to decode one video frame from the input stream.
    fn decode_next_packet(&mut self) -> Option<()> {
        loop {
            // Try to receive a decoded frame first
            let mut decoded = frame::Video::empty();
            if self.decoder.receive_frame(&mut decoded).is_ok() {
                return self.scale_frame(&decoded);
            }

            // Send next packet to the decoder
            let mut found_video = false;
            for (stream, packet) in self.input.as_input_mut().packets() {
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
                let mut decoded = frame::Video::empty();
                if self.decoder.receive_frame(&mut decoded).is_ok() {
                    return self.scale_frame(&decoded);
                }
                return None;
            }
        }
    }

    /// Scale a decoded frame to RGBA and store in `frame_buffer`.
    fn scale_frame(&mut self, decoded: &frame::Video) -> Option<()> {
        // Calculate variable frame duration
        // decoded.duration() returns duration in stream time_base units
        /*let duration = decoded.duration();
        if duration > 0 {
            self.frame_interval = duration as f64 * self.time_base;
        } else */
        {
            // Fallback to average if individual frame duration is missing
            self.frame_interval = self.avg_frame_interval;
        }

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

    pub fn frame_interval(&self) -> f64 {
        self.frame_interval
    }
}

impl VideoDecoder for FfmpegVideoDecoder {
    fn load(path: &Path) -> Result<Self>
    where
        Self: Sized,
    {
        ffmpeg_next::init().map_err(|e| anyhow::anyhow!("Failed to initialize FFmpeg: {}", e))?;

        let input = ffmpeg_next::format::input(&path)
            .map_err(|e| anyhow!("Failed to open video '{}': {}", path.display(), e))?;
        let input = ffmpeg_io::InputContext::from_input(input);
        Self::from_input(input, &path.display().to_string())
    }

    fn next_frame(&mut self) -> Option<&[u8]> {
        self.decode_next_packet()?;
        Some(&self.frame_buffer)
    }

    fn seek(&mut self, timestamp: f64) -> Result<()> {
        if self.time_base <= 0.0 {
            anyhow::bail!(
                "Cannot seek: stream has invalid time_base ({})",
                self.time_base
            );
        }
        let ts = (timestamp / self.time_base) as i64;
        self.input
            .as_input_mut()
            .seek(ts, ..ts)
            .map_err(|e| anyhow::anyhow!("Seek failed: {}", e))?;
        self.decoder.flush();
        Ok(())
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// A video texture that decodes frames on its own schedule.
pub struct VideoTexture {
    /// `None` for placeholder sources (optional textures whose file is missing).
    decoder: Option<FfmpegVideoDecoder>,
    /// Frame-rate accumulator (seconds).
    accum: f64,
    /// Whether to loop the video on EOF.
    looping: bool,
    /// True on the first update (forces an initial frame decode).
    pub(crate) first_frame: bool,
    /// Cached last frame data for dimensions.
    last_width: u32,
    last_height: u32,
    /// Optional filesystem watcher for external hot-reload.
    hot_reload: SourceHotReload,
}

impl VideoTexture {
    fn load_decoder_from_path(path: &Path) -> Result<(FfmpegVideoDecoder, u32, u32)> {
        let decoder = FfmpegVideoDecoder::load(path)?;
        let (w, h) = decoder.dimensions();
        Ok((decoder, w, h))
    }

    fn load_decoder_from_stream(
        stream: AssetByteStream,
        source_label: &str,
    ) -> Result<(FfmpegVideoDecoder, u32, u32)> {
        let decoder = FfmpegVideoDecoder::load_from_stream(stream, source_label)?;
        let (w, h) = decoder.dimensions();
        Ok((decoder, w, h))
    }

    fn reload_decoder_from_path(&mut self, path: &Path) -> Result<()> {
        let (decoder, w, h) = Self::load_decoder_from_path(path)
            .with_context(|| format!("Failed to reload video decoder '{}'", path.display()))?;
        self.decoder = Some(decoder);
        self.last_width = w;
        self.last_height = h;
        self.accum = 0.0;
        self.first_frame = true;
        info!(
            "VideoTexture reloaded from disk: {} ({}x{})",
            path.display(),
            w,
            h
        );
        Ok(())
    }

    /// Open a video file and prepare the decoder.
    ///
    /// When `optional` is `true` and the decoder fails to initialise, the
    /// texture degrades to a 1×1 transparent placeholder instead of
    /// returning an error. The hot-reload watcher (if enabled) stays active
    /// so the decoder is created once the file appears/changes.
    pub fn load(
        source: VideoSource,
        looping: bool,
        hot_reload: bool,
        optional: bool,
    ) -> Result<Self> {
        match source {
            VideoSource::ExternalPath(path) => {
                let hot_reload = if hot_reload {
                    match SourceHotReload::from_source(Some(&path)) {
                        Ok(hot_reload) => hot_reload,
                        Err(e) => {
                            warn!(
                                "VideoTexture watcher disabled for {}: {}",
                                path.display(),
                                e
                            );
                            SourceHotReload::disabled()
                        }
                    }
                } else {
                    SourceHotReload::disabled()
                };

                let decoder_path = hot_reload.source_path().unwrap_or(&path);
                match Self::load_decoder_from_path(decoder_path) {
                    Ok((decoder, w, h)) => {
                        if let Some(watch_path) = hot_reload.source_path() {
                            info!(
                                "VideoTexture loaded: {}x{}, looping={}, watcher={}",
                                w,
                                h,
                                looping,
                                watch_path.display()
                            );
                        } else {
                            info!("VideoTexture loaded: {}x{}, looping={}", w, h, looping);
                        }
                        Ok(Self {
                            decoder: Some(decoder),
                            accum: 0.0,
                            looping,
                            first_frame: true,
                            last_width: w,
                            last_height: h,
                            hot_reload,
                        })
                    }
                    Err(e) if optional => {
                        warn!(
                            "Optional video '{}' failed to load (using placeholder): {}",
                            path.display(),
                            e
                        );
                        Ok(Self {
                            decoder: None,
                            accum: 0.0,
                            looping,
                            first_frame: true,
                            last_width: 1,
                            last_height: 1,
                            hot_reload,
                        })
                    }
                    Err(e) => Err(e.context(format!(
                        "Failed to load video decoder '{}'",
                        decoder_path.display()
                    ))),
                }
            }
            VideoSource::EmbeddedStream(stream) => {
                let hot_reload = SourceHotReload::disabled();
                match Self::load_decoder_from_stream(stream, "embedded-stream") {
                    Ok((decoder, w, h)) => {
                        info!(
                            "Embedded VideoTexture loaded: {}x{}, looping={}",
                            w, h, looping
                        );
                        Ok(Self {
                            decoder: Some(decoder),
                            accum: 0.0,
                            looping,
                            first_frame: true,
                            last_width: w,
                            last_height: h,
                            hot_reload,
                        })
                    }
                    Err(e) if optional => {
                        warn!(
                            "Optional embedded video failed to load (using placeholder): {}",
                            e
                        );
                        Ok(Self {
                            decoder: None,
                            accum: 0.0,
                            looping,
                            first_frame: true,
                            last_width: 1,
                            last_height: 1,
                            hot_reload,
                        })
                    }
                    Err(e) => Err(e.context("Failed to load embedded video decoder")),
                }
            }
        }
    }

    /// Decode one frame, handling looping.
    fn decode_one_frame(&mut self) -> Option<Vec<u8>> {
        let decoder = self.decoder.as_mut()?;
        let (w, h) = decoder.dimensions();
        self.last_width = w;
        self.last_height = h;
        match decoder.next_frame() {
            Some(data) => Some(data.to_vec()),
            None => {
                if self.looping {
                    if let Err(e) = decoder.seek(0.0) {
                        log::warn!("Video seek-to-start failed: {}", e);
                    }
                    // Try again after seeking
                    decoder.next_frame().map(|d| d.to_vec())
                } else {
                    None
                }
            }
        }
    }

    fn frame_interval(&self) -> f64 {
        let raw = self
            .decoder
            .as_ref()
            .map(|d| d.frame_interval())
            .unwrap_or(1.0 / 30.0);
        if raw.is_finite() && raw > 0.0 {
            raw
        } else {
            1.0 / 30.0
        }
    }
}

impl TextureSource for VideoTexture {
    fn update(&mut self, dt: f64) -> Result<TextureUpdate> {
        if let Some(path) = self.hot_reload.take_changed_path()
            && let Err(e) = self.reload_decoder_from_path(&path)
        {
            warn!("VideoTexture reload failed: {}", e);
        }

        // On first frame, decode immediately regardless of timing.
        if self.first_frame {
            self.first_frame = false;
            if let Some(data) = self.decode_one_frame() {
                return Ok(TextureUpdate::NewFrame {
                    width: self.last_width,
                    height: self.last_height,
                    data,
                });
            }
            return Ok(TextureUpdate::Unchanged);
        }

        let interval = self.frame_interval();
        self.accum += dt;

        // Only produce one frame per update even if multiple intervals elapsed
        // to avoid decoding too many frames when the render loop hitches.
        let mut latest_frame: Option<Vec<u8>> = None;
        while self.accum >= interval {
            self.accum -= interval;
            if let Some(data) = self.decode_one_frame() {
                latest_frame = Some(data);
            }
        }

        match latest_frame {
            Some(data) => Ok(TextureUpdate::NewFrame {
                width: self.last_width,
                height: self.last_height,
                data,
            }),
            None => Ok(TextureUpdate::Unchanged),
        }
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.last_width, self.last_height)
    }

    fn texture_type(&self) -> &'static str {
        "video"
    }
}
