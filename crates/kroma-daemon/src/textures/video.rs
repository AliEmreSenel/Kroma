//! Video texture source.
//!
//! Wraps an FFmpeg decoder and produces RGBA frames at the video's
//! native frame rate. Handles accumulator-based timing, looping, and
//! hot-reload for external disk-backed files.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
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
    decoded_frame: frame::Video,
    rgba_frame: frame::Video,
    frame_buffer: Vec<u8>,
    time_base: f64,
    frame_interval: f64,     // Now dynamic: updates per frame
    avg_frame_interval: f64, // Fallback: calculated from stream average FPS
}

impl FfmpegVideoDecoder {
    fn from_input(input: ffmpeg_io::InputContext, source_label: &str) -> Result<Self> {
        let _timing = crate::timing::scope("video.decoder.create");
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
        let interval = frame_rate.denominator() as f64 / frame_rate.numerator() as f64;
        let avg_frame_interval = if interval.is_finite() && interval > 0.0 {
            interval
        } else {
            1.0 / 30.0
        };

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
            decoded_frame: frame::Video::empty(),
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

    /// Receive into a reusable native frame, then scale into reusable RGBA storage.
    fn next_frame_rgba(&mut self) -> Option<()> {
        loop {
            if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
                break;
            }
            let mut found_video = false;
            for (stream, packet) in self.input.as_input_mut().packets() {
                if stream.index() == self.video_stream_index
                    && self.decoder.send_packet(&packet).is_ok()
                {
                    found_video = true;
                    break;
                }
            }
            if !found_video {
                let _ = self.decoder.send_eof();
                self.decoder.receive_frame(&mut self.decoded_frame).ok()?;
                break;
            }
        }
        self.frame_interval = self.avg_frame_interval;
        self.scaler
            .run(&self.decoded_frame, &mut self.rgba_frame)
            .ok()?;
        Some(())
    }

    fn next_frame_into(&mut self, out: &mut Vec<u8>) -> Option<()> {
        self.next_frame_rgba()?;
        copy_rgba_rows(
            self.rgba_frame.data(0),
            self.rgba_frame.stride(0),
            self.width as usize,
            self.height as usize,
            out,
        )
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
        // Flush uploads even if rendering is paused or this source is replaced
        // before the next draw. Otherwise repeated paused loads retain staging.
        queue.submit(std::iter::empty());
    }

    pub fn frame_interval(&self) -> f64 {
        self.frame_interval
    }
}

/// Copy only pixel bytes, retaining initialized recycled storage between frames.
fn copy_rgba_rows(
    data: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    out: &mut Vec<u8>,
) -> Option<()> {
    let row_bytes = width.checked_mul(4)?;
    let len = row_bytes.checked_mul(height)?;
    if width == 0
        || height == 0
        || stride < row_bytes
        || data.len() < (height - 1).checked_mul(stride)?.checked_add(row_bytes)?
    {
        return None;
    }
    out.resize(len, 0);
    if stride == row_bytes {
        out.copy_from_slice(&data[..len]);
    } else {
        for (dst, src) in out.chunks_exact_mut(row_bytes).zip(data.chunks(stride)) {
            dst.copy_from_slice(&src[..row_bytes]);
        }
    }
    Some(())
}

impl VideoDecoder for FfmpegVideoDecoder {
    fn load(path: &Path) -> Result<Self>
    where
        Self: Sized,
    {
        let _timing = crate::timing::scope("video.external.open");
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
}

struct VideoWorkerReady {
    frame_interval: f64,
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
        }
    }

    fn recycle_buffer_to(tx: &mpsc::SyncSender<Vec<u8>>, data: Vec<u8>) {
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
                    frame_interval: decoder.frame_interval(),
                    width: w,
                    height: h,
                    gpu_target,
                };
                if ready_tx.send(Ok(ready)).is_err() {
                    return;
                }

                let frame_len = (w as usize).saturating_mul(h as usize).saturating_mul(4);
                if gpu_upload.is_none() {
                    for _ in 0..2 {
                        let _ = worker_recycle_tx.try_send(Vec::with_capacity(frame_len));
                    }
                }

                let first_frame_timing = crate::timing::scope("video.first_frame");
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
                    let Some(mut data) =
                        Self::take_recycled_buffer(&recycle_rx, frame_len, &stop_rx)
                    else {
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

                drop(first_frame_timing);
                loop {
                    match stop_rx.try_recv() {
                        Ok(()) | Err(TryRecvError::Disconnected) => return,
                        Err(TryRecvError::Empty) => {}
                    }

                    let (fw, fh) = decoder.dimensions();
                    let frame_interval;

                    if let Some((queue, texture)) = gpu_upload.as_ref() {
                        if Self::decode_with_looping_rgba(&mut decoder, looping).is_none() {
                            return;
                        }
                        decoder.upload_current_rgba_to(queue, texture);
                        let raw = decoder.frame_interval();
                        frame_interval = if raw.is_finite() && raw > 0.0 {
                            raw
                        } else {
                            1.0 / 30.0
                        };
                        if frame_tx
                            .send(DecodedVideoFrame {
                                data: None,
                                width: fw,
                                height: fh,
                                frame_interval,
                            })
                            .is_err()
                        {
                            return;
                        }
                    } else {
                        let frame_len = (fw as usize).saturating_mul(fh as usize).saturating_mul(4);
                        let Some(mut data) =
                            Self::take_recycled_buffer(&recycle_rx, frame_len, &stop_rx)
                        else {
                            return;
                        };
                        if Self::decode_with_looping_into(&mut decoder, looping, &mut data)
                            .is_none()
                        {
                            Self::recycle_buffer_to(&worker_recycle_tx, data);
                            return;
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
        self.frame_interval = ready.frame_interval;
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
        // The bounded metadata channel applies backpressure to GPU uploads too.
        // Paused rendering can queue at most two frames, rather than accumulating
        // unbounded driver staging work. This avoids a full RGBA copy through Vec.
        let gpu = gpu.cloned();
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
                            info!(
                                "VideoTexture loaded: {}x{}, looping={}",
                                ready.width, ready.height, looping
                            );
                        }
                        Ok(Self {
                            worker: Some(worker),
                            looping,
                            frame_interval: ready.frame_interval,
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
                            frame_interval: ready.frame_interval,
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
    fn frame_rate(&self) -> Option<f64> {
        self.worker.as_ref().map(|_| 1.0 / self.frame_interval)
    }

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
        while let Ok(frame) = worker.frame_rx.try_recv() {
            if let Some(old) = latest.replace(frame)
                && let Some(data) = old.data
            {
                Self::recycle_buffer_to(&worker.recycle_tx, data);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "manual real-media decode profiling; set KROMA_VIDEO_PROFILE_PATH"]
    fn profile_decode() {
        let path = std::env::var("KROMA_VIDEO_PROFILE_PATH").expect("real media path");
        let count: usize = std::env::var("KROMA_VIDEO_PROFILE_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120);
        assert!(count > 0);
        let start = std::time::Instant::now();
        let mut decoder = FfmpegVideoDecoder::load(Path::new(&path)).unwrap();
        let open_ms = start.elapsed().as_secs_f64() * 1000.0;
        let mut output = Vec::new();
        let start = std::time::Instant::now();
        decoder.next_frame_into(&mut output).unwrap();
        let first_ms = start.elapsed().as_secs_f64() * 1000.0;
        let hash = |data: &[u8]| {
            data.iter().fold(0xcbf29ce484222325u64, |h, b| {
                (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
            })
        };
        let first_hash = hash(&output);
        let start = std::time::Instant::now();
        for _ in 0..count {
            VideoTexture::decode_with_looping_into(&mut decoder, true, &mut output).unwrap();
        }
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        println!(
            "VIDEO_PROFILE {}",
            serde_json::json!({"path": path, "width": decoder.width, "height": decoder.height,
            "open_ms": open_ms, "first_frame_ms": first_ms, "frames": count, "decode_ms": elapsed,
            "first_frame_hash": format!("{first_hash:016x}"), "last_frame_hash": format!("{:016x}", hash(&output))})
        );
    }

    #[test]
    #[ignore = "real-media GPU byte check; set KROMA_VIDEO_PROFILE_PATH"]
    fn profile_gpu_transfer() {
        let path = std::env::var("KROMA_VIDEO_PROFILE_PATH").expect("real media path");
        let instance = wgpu::Instance::default();
        let adapter = crate::renderer::pollster_block(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        )
        .unwrap();
        let (device, queue) =
            crate::renderer::pollster_block(adapter.request_device(&wgpu::DeviceDescriptor {
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            }))
            .unwrap();
        let gpu = GpuContext {
            device: Arc::new(device),
            queue: Arc::new(queue),
            surface_format: wgpu::TextureFormat::Rgba8UnormSrgb,
        };
        let mut decoder = FfmpegVideoDecoder::load(Path::new(&path)).unwrap();
        let mut expected = Vec::new();
        decoder.next_frame_into(&mut expected).unwrap();
        let (width, height) = decoder.dimensions();
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video-byte-check"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        decoder.upload_current_rgba_to(&gpu.queue, &target);
        let row_bytes = width * 4;
        let padded = row_bytes.div_ceil(256) * 256;
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
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
        gpu.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            tx.send(result).unwrap()
        });
        gpu.device
            .poll(wgpu::wgt::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(30)),
            })
            .unwrap();
        rx.recv_timeout(Duration::from_secs(30)).unwrap().unwrap();
        let bytes = buffer.get_mapped_range(..);
        let mut actual = Vec::new();
        copy_rgba_rows(
            &bytes,
            padded as usize,
            width as usize,
            height as usize,
            &mut actual,
        )
        .unwrap();
        assert_eq!(
            actual, expected,
            "GPU upload must preserve every decoded RGBA byte"
        );
        println!(
            "VIDEO_GPU_CHECK {}",
            serde_json::json!({"path":path,"width":width,"height":height,"bytes_verified":actual.len()})
        );
    }

    #[test]
    fn rgba_copy_handles_padding_reuse_and_incomplete_frames() {
        let mut output = vec![99; 16];
        let allocation = output.as_ptr();
        copy_rgba_rows(
            &[
                1, 2, 3, 4, 5, 6, 7, 8, 90, 90, 9, 10, 11, 12, 13, 14, 15, 16,
            ],
            10,
            2,
            2,
            &mut output,
        )
        .unwrap();
        assert_eq!(output, (1..=16).collect::<Vec<_>>());
        assert_eq!(allocation, output.as_ptr());
        assert!(copy_rgba_rows(&[0; 8], 8, 2, 2, &mut output).is_none());
        assert_eq!(output, (1..=16).collect::<Vec<_>>());
        copy_rgba_rows(&[42; 16], 8, 2, 2, &mut output).unwrap();
        assert_eq!(output, vec![42; 16]);
    }

    #[test]
    fn recycling_retains_initialized_frame_storage() {
        let (tx, rx) = mpsc::sync_channel(1);
        let (_stop_tx, stop_rx) = mpsc::channel();
        let data = vec![7; 32];
        let allocation = data.as_ptr();
        VideoTexture::recycle_buffer_to(&tx, data);
        let recycled = VideoTexture::take_recycled_buffer(&rx, 32, &stop_rx).unwrap();
        assert_eq!(recycled, vec![7; 32]);
        assert_eq!(recycled.as_ptr(), allocation);
    }
}
