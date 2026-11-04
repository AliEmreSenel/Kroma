//! Isolated preview jobs reuse the daemon lifecycle, uniforms, textures and renderer.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use crate::{Daemon, DaemonPhase};

#[derive(Deserialize)]
struct Job {
    thumbnail: Option<PathBuf>,
    video: Option<PathBuf>,
    width: u32,
    height: u32,
    fps: u32,
    seconds: f64,
}

struct Encoder(Option<Child>);

impl Drop for Encoder {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Run a job inside the private XDG environment created by the CLI.
pub(crate) fn run(path: &std::path::Path) -> Result<()> {
    let job: Job = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        job.fps > 0
            && job.fps <= 60
            && job.seconds.is_finite()
            && job.seconds > 0.0
            && job.seconds <= 30.0,
        "Invalid preview duration/FPS"
    );
    ensure!(
        job.thumbnail.is_some() || job.video.is_some(),
        "No preview output requested"
    );
    ensure!(
        job.video.is_none() || (job.width.is_multiple_of(2) && job.height.is_multiple_of(2)),
        "Video preview dimensions must be even"
    );
    // Refuse to run this mode against a real desktop or shared runtime socket.
    ensure!(
        std::env::var("KROMA_PREVIEW_JOB").ok().as_deref() == Some("1")
            && std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref() == Some("KDE"),
        "Preview jobs require the CLI's isolated environment"
    );
    let mut daemon = Daemon::new()?;
    ensure!(
        daemon.runtime_shade.is_some() && daemon.current_phase != DaemonPhase::Terminal,
        "Package failed to load for preview"
    );
    let capture = daemon
        .renderer
        .create_preview_capture(job.width, job.height)?;
    let mut encoder = Encoder(if let Some(video) = &job.video {
        Some(
            Command::new("ffmpeg")
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-n",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgba",
                    "-video_size",
                ])
                .arg(format!("{}x{}", job.width, job.height))
                .args([
                    "-framerate",
                    &job.fps.to_string(),
                    "-i",
                    "pipe:0",
                    "-an",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-crf",
                    "28",
                    "-threads",
                    "1",
                    "-pix_fmt",
                    "yuv420p",
                    "-movflags",
                    "+faststart",
                ])
                .arg(video)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()
                .context("Start preview encoder")?,
        )
    } else {
        None
    });
    // Let asynchronous video decoders initialize before recording. Lifecycle
    // advancement and nested/feedback rendering still run at the daemon's rate.
    daemon.last_frame_time = Instant::now();
    let start = Instant::now();
    let warmup = Duration::from_millis(150);
    let interval = Duration::from_secs_f64(1.0 / f64::from(job.fps));
    let frames = (job.seconds * f64::from(job.fps)).ceil() as u32;
    let mut recorded = 0;
    let mut next_capture = start + warmup;
    let mut poster_written = false;
    while recorded < frames {
        daemon.render_tick(&[])?;
        if Instant::now() < next_capture {
            continue;
        }
        let elapsed_slots = ((Instant::now() - next_capture).as_secs_f64() / interval.as_secs_f64())
            .floor() as u32
            + 1;
        let copies = elapsed_slots.min(frames - recorded);
        let rgba = daemon.renderer.capture_preview(&capture)?;
        if let Some(child) = encoder.0.as_mut() {
            for _ in 0..copies {
                child
                    .stdin
                    .as_mut()
                    .context("Preview encoder input closed")?
                    .write_all(&rgba)?;
            }
        }
        // Prefer an active-phase poster, preserving load-phase frames in the clip.
        if !poster_written
            && let Some(thumbnail) = &job.thumbnail
            && (daemon.current_phase == DaemonPhase::Active || recorded + 1 == frames)
        {
            let image = image::RgbaImage::from_raw(job.width, job.height, rgba)
                .context("Invalid captured frame")?;
            let rgb = image::DynamicImage::ImageRgba8(image).to_rgb8();
            let output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(thumbnail)?;
            image::codecs::jpeg::JpegEncoder::new_with_quality(output, 85).encode_image(&rgb)?;
            poster_written = true;
        }
        recorded += copies;
        if encoder.0.is_none() && poster_written {
            break;
        }
        next_capture += interval * copies;
    }
    if let Some(mut child) = encoder.0.take() {
        drop(child.stdin.take());
        ensure!(child.wait()?.success(), "Preview video encoder failed");
    }
    daemon.shutdown()?;
    Ok(())
}
