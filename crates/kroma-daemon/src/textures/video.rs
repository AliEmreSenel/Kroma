//! Video texture source.
//!
//! Wraps an FFmpeg decoder and produces RGBA frames at the video's
//! native frame rate. Handles accumulator-based timing, looping, and
//! hot-reload for external disk-backed files.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError, TrySendError};
use std::sync::Arc;
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
use super::{GpuContext, TextureSource, TextureUpdate, VideoSource};

use kroma_shared::shade::AssetByteStream;

pub struct FfmpegVideoDecoder {
    input: ffmpeg_io::InputContext,
    decoder: ffmpeg_next::decoder::Video,
    video_stream_index: usize,
    scaler: ffmpeg_next::software::scaling::Context,
    width: u32,
    height: u32,
    rgba_frame: frame::Video,
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
            rgba_frame: frame::Video::empty(),
            frame_buffer: Vec::with_capacity((width * height * 4) as usize),
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

    /// Try to decode one video frame from the input stream into `out`.
    fn decode_next_packet_into(&mut self, out: &mut Vec<u8>) -> Option<()> {
        loop {
            // Try to receive a decoded frame first
            let mut decoded = frame::Video::empty();
            if self.decoder.receive_frame(&mut decoded).is_ok() {
                return self.scale_frame_into(&decoded, out);
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
                    return self.scale_frame_into(&decoded, out);
                }
                return None;
            }
        }
    }

    /// Scale a decoded frame to the reusable FFmpeg RGBA frame.
    fn scale_frame_to_rgba(&mut self, decoded: &frame::Video) -> Option<()> {
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

        // Reuse the same FFmpeg RGBA output frame. Creating a fresh Video frame
        // here forces FFmpeg to allocate an output buffer every decoded frame.
        self.scaler.run(decoded, &mut self.rgba_frame).ok()?;
        Some(())
    }

    /// Scale a decoded frame to RGBA and store it in `out`.
    fn scale_frame_into(&mut self, decoded: &frame::Video, out: &mut Vec<u8>) -> Option<()> {
        self.scale_frame_to_rgba(decoded)?;

        let data = self.rgba_frame.data(0);
        let stride = self.rgba_frame.stride(0);
        let w = self.width as usize;
        let h = self.height as usize;
        let row_bytes = w * 4;

        out.resize(w * h * 4, 0);
        for y in 0..h {
            let src_start = y * stride;
            let dst_start = y * row_bytes;
            if src_start + row_bytes <= data.len() && dst_start + row_bytes <= out.len() {
                out[dst_start..dst_start + row_bytes]
                    .copy_from_slice(&data[src_start..src_start + row_bytes]);
            }
        }
        Some(())
    }

    /// Decode one frame into the reusable FFmpeg RGBA frame without copying to Rust memory.
    fn next_frame_rgba(&mut self) -> Option<()> {
        loop {
            let mut decoded = frame::Video::empty();
            if self.decoder.receive_frame(&mut decoded).is_ok() {
                return self.scale_frame_to_rgba(&decoded);
            }

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
                let _ = self.decoder.send_eof();
                let mut decoded = frame::Video::empty();
                if self.decoder.receive_frame(&mut decoded).is_ok() {
                    return self.scale_frame_to_rgba(&decoded);
                }
                return None;
            }
        }
    }

    fn next_frame_into(&mut self, out: &mut Vec<u8>) -> Option<()> {
        self.decode_next_packet_into(out)
    }

    fn upload_current_rgba_to(&self, queue: &wgpu::Queue, texture: &wgpu::Texture) {
        let data = self.rgba_frame.data(0);
        let stride = self.rgba_frame.stride(0) as u32;
        let width = self.width.max(1);
        let height = self.height.max(1);

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
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
        let mut frame_buffer = std::mem::take(&mut self.frame_buffer);
        let decoded = self.next_frame_into(&mut frame_buffer).is_some();
        self.frame_buffer = frame_buffer;
        decoded.then_some(&self.frame_buffer)
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
    data: Option<Vec<u8>>,
    width: u32,
    height: u32,
    frame_interval: f64,
}

struct GpuVideoTarget {
    _texture: Arc<wgpu::Texture>,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    width: u32,
    height: u32,
}

struct VideoWorkerReady {
    width: u32,
    height: u32,
    gpu_target: Option<GpuVideoTarget>,
}

struct VideoDecodeWorker {
    stop_tx: mpsc::Sender<()>,
    frame_rx: Receiver<DecodedVideoFrame>,
    recycle_tx: mpsc::SyncSender<Vec<u8>>,
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
    gpu: Option<GpuContext>,
    gpu_target: Option<GpuVideoTarget>,
    gpu_rebind_pending: bool,
}

impl VideoTexture {
    fn decode_with_looping_into(
        decoder: &mut FfmpegVideoDecoder,
        looping: bool,
        out: &mut Vec<u8>,
    ) -> Option<()> {
        match decoder.next_frame_into(out) {
            Some(()) => Some(()),
            None if looping => {
                if let Err(e) = decoder.seek(0.0) {
                    warn!("Video seek-to-start failed: {}", e);
                    return None;
                }
                decoder.next_frame_into(out)
            }
            None => None,
        }
    }

    fn decode_with_looping_rgba(decoder: &mut FfmpegVideoDecoder, looping: bool) -> Option<()> {
        match decoder.next_frame_rgba() {
            Some(()) => Some(()),
            None if looping => {
                if let Err(e) = decoder.seek(0.0) {
                    warn!("Video seek-to-start failed: {}", e);
                    return None;
                }
                decoder.next_frame_rgba()
            }
            None => None,
        }
    }

    fn create_gpu_target(gpu: &GpuContext, width: u32, height: u32, label: &str) -> GpuVideoTarget {
        let width = width.max(1);
        let height = height.max(1);
        let texture = Arc::new(gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{}-sampler", label)),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        GpuVideoTarget {
            _texture: texture,
            view,
            sampler,
            width,
            height,
        }
    }

    fn recycle_buffer_to(tx: &mpsc::SyncSender<Vec<u8>>, mut data: Vec<u8>) {
        data.clear();
        let _ = tx.try_send(data);
    }

    fn take_recycled_buffer(
        rx: &Receiver<Vec<u8>>,
        required_len: usize,
        stop_rx: &Receiver<()>,
    ) -> Option<Vec<u8>> {
        loop {
            match stop_rx.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => {}
            }

            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(mut data) => {
                    if data.capacity() < required_len {
                        data = Vec::with_capacity(required_len);
                    }
                    data.clear();
                    return Some(data);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    fn spawn_worker(
        source: VideoSource,
        looping: bool,
        source_label: &str,
        gpu: Option<GpuContext>,
    ) -> Result<(VideoDecodeWorker, VideoWorkerReady)> {
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<VideoWorkerReady, String>>(1);
        let (frame_tx, frame_rx) = mpsc::sync_channel::<DecodedVideoFrame>(1);
        let (recycle_tx, recycle_rx) = mpsc::sync_channel::<Vec<u8>>(2);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        let worker_recycle_tx = recycle_tx.clone();
        let label = source_label.to_string();
        let worker_label = label.clone();
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
                let gpu_target = gpu.as_ref().map(|gpu| {
                    Self::create_gpu_target(gpu, w, h, &format!("kroma-video-{}", worker_label))
                });
                let gpu_upload = gpu
                    .as_ref()
                    .zip(gpu_target.as_ref())
                    .map(|(gpu, target)| (Arc::clone(&gpu.queue), Arc::clone(&target._texture)));

                let ready = VideoWorkerReady {
                    width: w,
                    height: h,
                    gpu_target,
                };
                if ready_tx.send(Ok(ready)).is_err() {
                    return;
                }

                let frame_len = (w as usize).saturating_mul(h as usize).saturating_mul(4);
                for _ in 0..2 {
                    let _ = worker_recycle_tx.try_send(Vec::with_capacity(frame_len));
                }

                // Prime the first frame. In GPU-managed mode this uploads straight
                // from FFmpeg's RGBA frame into the wgpu texture and sends only
                // metadata to the renderer; no per-frame Vec crosses threads.
                if let Some((queue, texture)) = gpu_upload.as_ref() {
                    if Self::decode_with_looping_rgba(&mut decoder, looping).is_some() {
                        decoder.upload_current_rgba_to(queue, texture);
                        let frame_interval = {
                            let raw = decoder.frame_interval();
                            if raw.is_finite() && raw > 0.0 {
                                raw
                            } else {
                                1.0 / 30.0
                            }
                        };
                        let _ = frame_tx.try_send(DecodedVideoFrame {
                            data: None,
                            width: w,
                            height: h,
                            frame_interval,
                        });
                    }
                } else {
                    let Some(mut data) = Self::take_recycled_buffer(&recycle_rx, frame_len, &stop_rx) else {
                        return;
                    };
                    if Self::decode_with_looping_into(&mut decoder, looping, &mut data).is_some() {
                        let frame_interval = {
                            let raw = decoder.frame_interval();
                            if raw.is_finite() && raw > 0.0 {
                                raw
                            } else {
                                1.0 / 30.0
                            }
                        };
                        let frame = DecodedVideoFrame {
                            data: Some(data),
                            width: w,
                            height: h,
                            frame_interval,
                        };
                        if frame_tx.send(frame).is_err() {
                            return;
                        }
                    } else {
                        Self::recycle_buffer_to(&worker_recycle_tx, data);
                    }
                }

                loop {
                    match stop_rx.try_recv() {
                        Ok(()) | Err(TryRecvError::Disconnected) => return,
                        Err(TryRecvError::Empty) => {}
                    }

                    let (fw, fh) = decoder.dimensions();
                    let frame_interval;

                    if let Some((queue, texture)) = gpu_upload.as_ref() {
                        if Self::decode_with_looping_rgba(&mut decoder, looping).is_none() {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        decoder.upload_current_rgba_to(queue, texture);
                        let raw = decoder.frame_interval();
                        frame_interval = if raw.is_finite() && raw > 0.0 {
                            raw
                        } else {
                            1.0 / 30.0
                        };
                        match frame_tx.try_send(DecodedVideoFrame {
                            data: None,
                            width: fw,
                            height: fh,
                            frame_interval,
                        }) {
                            Ok(()) | Err(TrySendError::Full(_)) => {}
                            Err(TrySendError::Disconnected(_)) => return,
                        }
                    } else {
                        let frame_len = (fw as usize).saturating_mul(fh as usize).saturating_mul(4);
                        let Some(mut data) = Self::take_recycled_buffer(&recycle_rx, frame_len, &stop_rx) else {
                            return;
                        };
                        if Self::decode_with_looping_into(&mut decoder, looping, &mut data).is_none()
                        {
                            Self::recycle_buffer_to(&worker_recycle_tx, data);
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }

                        let raw = decoder.frame_interval();
                        frame_interval = if raw.is_finite() && raw > 0.0 {
                            raw
                        } else {
                            1.0 / 30.0
                        };

                        let frame = DecodedVideoFrame {
                            data: Some(data),
                            width: fw,
                            height: fh,
                            frame_interval,
                        };

                        if frame_tx.send(frame).is_err() {
                            return;
                        }
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

        let ready = ready_rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| anyhow!("Timed out waiting for video decode worker init"))?
            .map_err(|e| anyhow!(e))?;

        Ok((
            VideoDecodeWorker {
                stop_tx,
                frame_rx,
                recycle_tx,
                handle,
            },
            ready,
        ))
    }

    fn stop_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            let VideoDecodeWorker {
                stop_tx,
                frame_rx,
                recycle_tx,
                handle,
            } = worker;
            let _ = stop_tx.send(());
            drop(frame_rx);
            drop(recycle_tx);
            let _ = handle.join();
        }
    }

    fn restart_worker_from_path(&mut self, path: &Path) -> Result<()> {
        self.stop_worker();
        let (worker, ready) = Self::spawn_worker(
            VideoSource::ExternalPath(path.to_path_buf()),
            self.looping,
            &path.display().to_string(),
            self.gpu.clone(),
        )
        .with_context(|| format!("Failed to reload video decoder '{}'", path.display()))?;
        self.worker = Some(worker);
        self.last_width = ready.width;
        self.last_height = ready.height;
        self.gpu_target = ready.gpu_target;
        self.gpu_rebind_pending = self.gpu_target.is_some();
        self.frame_interval = 1.0 / 30.0;
        self.pending_frame = None;
        info!(
            "VideoTexture reloaded from disk: {} ({}x{})",
            path.display(),
            ready.width,
            ready.height
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
        gpu: Option<&GpuContext>,
    ) -> Result<Self> {
        // Keep video uploads on the render thread. Uploading directly from the
        // decode worker can enqueue unbounded driver-side staging work that is
        // not visible in process RSS/heap profiles.
        let _ = gpu;
        let gpu: Option<GpuContext> = None;
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

                let pending_frame = None;

                match Self::spawn_worker(
                    VideoSource::ExternalPath(decoder_path.clone()),
                    looping,
                    &decoder_path.display().to_string(),
                    gpu.clone(),
                ) {
                    Ok((worker, ready)) => {
                        if let Some(watch_path) = hot_reload.source_path() {
                            info!(
                                "VideoTexture loaded: {}x{}, looping={}, watcher={}",
                                ready.width,
                                ready.height,
                                looping,
                                watch_path.display()
                            );
                        } else {
                            info!("VideoTexture loaded: {}x{}, looping={}", ready.width, ready.height, looping);
                        }
                        Ok(Self {
                            worker: Some(worker),
                            looping,
                            frame_interval: 1.0 / 30.0,
                            pending_frame,
                            last_width: ready.width,
                            last_height: ready.height,
                            hot_reload,
                            gpu,
                            gpu_target: ready.gpu_target,
                            gpu_rebind_pending: false,
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
                            gpu,
                            gpu_target: None,
                            gpu_rebind_pending: false,
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
                    gpu.clone(),
                ) {
                    Ok((worker, ready)) => {
                        info!(
                            "Embedded VideoTexture loaded: {}x{}, looping={}",
                            ready.width, ready.height, looping
                        );
                        Ok(Self {
                            worker: Some(worker),
                            looping,
                            frame_interval: 1.0 / 30.0,
                            pending_frame: None,
                            last_width: ready.width,
                            last_height: ready.height,
                            hot_reload,
                            gpu,
                            gpu_target: ready.gpu_target,
                            gpu_rebind_pending: false,
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
                            gpu,
                            gpu_target: None,
                            gpu_rebind_pending: false,
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
            if let Some(data) = frame.data {
                return Ok(TextureUpdate::NewFrame {
                    width: self.last_width,
                    height: self.last_height,
                    data,
                });
            }
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
                Ok(frame) => {
                    if let Some(old) = latest.replace(frame)
                        && let Some(data) = old.data
                    {
                        Self::recycle_buffer_to(&worker.recycle_tx, data);
                    }
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }

        if let Some(frame) = latest {
            let old_dimensions = (self.last_width, self.last_height);
            self.last_width = frame.width;
            self.last_height = frame.height;
            self.frame_interval = frame.frame_interval;

            if let Some(data) = frame.data {
                return Ok(TextureUpdate::NewFrame {
                    width: self.last_width,
                    height: self.last_height,
                    data,
                });
            }

            // GPU-managed video frames are already uploaded by the decode worker.
            // The renderer only needs a signal when the texture/view changed, e.g.
            // after hot reload with new dimensions. Same-size video frames do not
            // need to pass through update_textures at all.
            if self.gpu_rebind_pending || old_dimensions != (self.last_width, self.last_height) {
                self.gpu_rebind_pending = false;
                return Ok(TextureUpdate::NewFrame {
                    width: self.last_width,
                    height: self.last_height,
                    data: Vec::new(),
                });
            }
        }

        if self.gpu_rebind_pending {
            self.gpu_rebind_pending = false;
            return Ok(TextureUpdate::NewFrame {
                width: self.last_width,
                height: self.last_height,
                data: Vec::new(),
            });
        }

        Ok(TextureUpdate::Unchanged)
    }

    fn recycle_frame(&mut self, data: Vec<u8>) {
        if data.is_empty() {
            return;
        }
        if let Some(worker) = self.worker.as_ref() {
            Self::recycle_buffer_to(&worker.recycle_tx, data);
        }
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.last_width, self.last_height)
    }

    fn texture_type(&self) -> &'static str {
        "video"
    }

    fn is_gpu_managed(&self) -> bool {
        self.gpu_target.is_some()
    }

    fn gpu_texture_view(&self) -> Option<&wgpu::TextureView> {
        self.gpu_target.as_ref().map(|target| &target.view)
    }

    fn gpu_sampler(&self) -> Option<&wgpu::Sampler> {
        self.gpu_target.as_ref().map(|target| &target.sampler)
    }
}
