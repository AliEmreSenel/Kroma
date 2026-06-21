//! Video texture source.
//!
//! Wraps an FFmpeg decoder and produces RGBA frames at the video's
//! native frame rate. Handles accumulator-based timing, looping, and
//! hot-reload for external disk-backed files.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError, TrySendError};
use std::thread::JoinHandle;
use std::time::Duration;

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

struct DecodedVideoFrame {
    data: Vec<u8>,
    width: u32,
    height: u32,
    frame_interval: f64,
}

struct VideoDecodeWorker {
    stop_tx: mpsc::Sender<()>,
    frame_rx: Receiver<DecodedVideoFrame>,
    handle: JoinHandle<()>,
}

pub struct VideoTexture {
    worker: Option<VideoDecodeWorker>,
    looping: bool,
    frame_interval: f64,
    pending_frame: Option<DecodedVideoFrame>,
    last_width: u32,
    last_height: u32,
    hot_reload: SourceHotReload,
}

impl VideoTexture {
    fn decode_with_looping(decoder: &mut FfmpegVideoDecoder, looping: bool) -> Option<Vec<u8>> {
        match decoder.next_frame() {
            Some(data) => Some(data.to_vec()),
            None if looping => {
                if let Err(e) = decoder.seek(0.0) {
                    warn!("Video seek-to-start failed: {}", e);
                    return None;
                }
                decoder.next_frame().map(|d| d.to_vec())
            }
            None => None,
        }
    }

    fn spawn_worker(
        source: VideoSource,
        looping: bool,
        source_label: &str,
    ) -> Result<(VideoDecodeWorker, u32, u32)> {
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(u32, u32), String>>(1);
        let (frame_tx, frame_rx) = mpsc::sync_channel::<DecodedVideoFrame>(2);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        let label = source_label.to_string();
        let handle = std::thread::Builder::new()
            .name(format!("kroma-video-{}", label))
            .spawn(move || {
                let mut decoder = match source {
                    VideoSource::ExternalPath(path) => match FfmpegVideoDecoder::load(&path) {
                        Ok(d) => d,
                        Err(e) => {
                            let _ = ready_tx.send(Err(format!(
                                "Failed to open video '{}': {}",
                                path.display(),
                                e
                            )));
                            return;
                        }
                    },
                    VideoSource::EmbeddedStream(stream) => {
                        match FfmpegVideoDecoder::load_from_stream(stream, "embedded-stream") {
                            Ok(d) => d,
                            Err(e) => {
                                let _ = ready_tx.send(Err(format!(
                                    "Failed to open embedded video stream: {}",
                                    e
                                )));
                                return;
                            }
                        }
                    }
                };

                let (w, h) = decoder.dimensions();
                if ready_tx.send(Ok((w, h))).is_err() {
                    return;
                }

                // Prime the queue with the very first frame so callers do not
                // render a black placeholder while waiting for async decode.
                if let Some(data) = Self::decode_with_looping(&mut decoder, looping) {
                    let frame_interval = {
                        let raw = decoder.frame_interval();
                        if raw.is_finite() && raw > 0.0 {
                            raw
                        } else {
                            1.0 / 30.0
                        }
                    };
                    let _ = frame_tx.try_send(DecodedVideoFrame {
                        data,
                        width: w,
                        height: h,
                        frame_interval,
                    });
                }

                loop {
                    match stop_rx.try_recv() {
                        Ok(()) | Err(TryRecvError::Disconnected) => return,
                        Err(TryRecvError::Empty) => {}
                    }

                    let data = match Self::decode_with_looping(&mut decoder, looping) {
                        Some(data) => data,
                        None => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                    };

                    let (fw, fh) = decoder.dimensions();
                    let frame_interval = {
                        let raw = decoder.frame_interval();
                        if raw.is_finite() && raw > 0.0 {
                            raw
                        } else {
                            1.0 / 30.0
                        }
                    };

                    let frame = DecodedVideoFrame {
                        data,
                        width: fw,
                        height: fh,
                        frame_interval,
                    };

                    match frame_tx.try_send(frame) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {}
                        Err(TrySendError::Disconnected(_)) => return,
                    }

                    let sleep_secs = frame_interval.clamp(1.0 / 240.0, 1.0);
                    std::thread::sleep(Duration::from_secs_f64(sleep_secs));
                }
            })
            .with_context(|| {
                format!(
                    "Failed to spawn video decode worker for '{}': {}",
                    label, label
                )
            })?;

        let (w, h) = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("Timed out waiting for video decode worker init"))?
            .map_err(|e| anyhow!(e))?;

        Ok((
            VideoDecodeWorker {
                stop_tx,
                frame_rx,
                handle,
            },
            w,
            h,
        ))
    }

    fn stop_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.stop_tx.send(());
            let _ = worker.handle.join();
        }
    }

    fn restart_worker_from_path(&mut self, path: &Path) -> Result<()> {
        self.stop_worker();
        let (worker, w, h) = Self::spawn_worker(
            VideoSource::ExternalPath(path.to_path_buf()),
            self.looping,
            &path.display().to_string(),
        )
        .with_context(|| format!("Failed to reload video decoder '{}'", path.display()))?;
        self.worker = Some(worker);
        self.last_width = w;
        self.last_height = h;
        self.frame_interval = 1.0 / 30.0;
        self.pending_frame = None;
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

                let decoder_path = hot_reload
                    .source_path()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| path.clone());

                let mut pending_frame = None;
                if let Ok(mut primer) = FfmpegVideoDecoder::load(&decoder_path)
                    && let Some(data) = Self::decode_with_looping(&mut primer, looping)
                {
                    let (pw, ph) = primer.dimensions();
                    let frame_interval = {
                        let raw = primer.frame_interval();
                        if raw.is_finite() && raw > 0.0 {
                            raw
                        } else {
                            1.0 / 30.0
                        }
                    };
                    pending_frame = Some(DecodedVideoFrame {
                        data,
                        width: pw,
                        height: ph,
                        frame_interval,
                    });
                }

                match Self::spawn_worker(
                    VideoSource::ExternalPath(decoder_path.clone()),
                    looping,
                    &decoder_path.display().to_string(),
                ) {
                    Ok((worker, w, h)) => {
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
                            worker: Some(worker),
                            looping,
                            frame_interval: 1.0 / 30.0,
                            pending_frame,
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
                            worker: None,
                            looping,
                            frame_interval: 1.0 / 30.0,
                            pending_frame: None,
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
                match Self::spawn_worker(
                    VideoSource::EmbeddedStream(stream),
                    looping,
                    "embedded-stream",
                ) {
                    Ok((worker, w, h)) => {
                        info!(
                            "Embedded VideoTexture loaded: {}x{}, looping={}",
                            w, h, looping
                        );
                        Ok(Self {
                            worker: Some(worker),
                            looping,
                            frame_interval: 1.0 / 30.0,
                            pending_frame: None,
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
                            worker: None,
                            looping,
                            frame_interval: 1.0 / 30.0,
                            pending_frame: None,
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
}

impl Drop for VideoTexture {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

impl TextureSource for VideoTexture {
    fn update(&mut self, dt: f64) -> Result<TextureUpdate> {
        if let Some(frame) = self.pending_frame.take() {
            self.last_width = frame.width;
            self.last_height = frame.height;
            self.frame_interval = frame.frame_interval;
            return Ok(TextureUpdate::NewFrame {
                width: self.last_width,
                height: self.last_height,
                data: frame.data,
            });
        }

        if let Some(path) = self.hot_reload.take_changed_path()
            && let Err(e) = self.restart_worker_from_path(&path)
        {
            warn!("VideoTexture reload failed: {}", e);
        }

        let _ = dt;

        let Some(worker) = self.worker.as_ref() else {
            return Ok(TextureUpdate::Unchanged);
        };

        let mut latest: Option<DecodedVideoFrame> = None;
        loop {
            match worker.frame_rx.try_recv() {
                Ok(frame) => latest = Some(frame),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }

        if let Some(frame) = latest {
            self.last_width = frame.width;
            self.last_height = frame.height;
            self.frame_interval = frame.frame_interval;
            return Ok(TextureUpdate::NewFrame {
                width: self.last_width,
                height: self.last_height,
                data: frame.data,
            });
        }

        Ok(TextureUpdate::Unchanged)
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.last_width, self.last_height)
    }

    fn texture_type(&self) -> &'static str {
        "video"
    }
}
