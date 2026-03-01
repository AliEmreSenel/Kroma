//! Slideshow texture source — lazy loading variant.
//!
//! Cycles through a list of child texture sources (images and videos) on
//! a timer. Only the **current** child is loaded in memory at any time.
//! Image data is never kept in memory — it is read from the mmap-backed
//! shade package on demand when a slide becomes active.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use log::info;

use kroma_shared::shade::LiveShadePackage;
use kroma_shared::types::{SlideSource, SlideSourceType, TextureFilter, TextureWrap};

use super::image::ImageTexture;
use super::video::VideoTexture;
use super::{TextureSource, TextureUpdate, read_asset_or_disk, resolve_video_path};

// ---------------------------------------------------------------------------
// Slide entry — lightweight metadata kept for every slide
// ---------------------------------------------------------------------------

/// Metadata for a single slide. No pixel data is stored here — images are
/// read from the shade package's mmap on demand, and videos reference a
/// filesystem path.
struct SlideEntry {
    /// Asset path / name (as it appears in the package or on disk).
    source: String,
    /// For videos: resolved filesystem path (temp-extracted or on-disk).
    /// For images: `None` — read from package each time.
    video_path: Option<PathBuf>,
    /// Whether this entry is an image or video.
    ty: SlideSourceType,
}

// ---------------------------------------------------------------------------
// Active child — the currently loaded texture
// ---------------------------------------------------------------------------

/// A loaded child source in the slideshow.
enum SlideChild {
    Image(ImageTexture),
    Video(VideoTexture),
}

impl SlideChild {
    fn as_source(&self) -> &dyn TextureSource {
        match self {
            SlideChild::Image(t) => t,
            SlideChild::Video(t) => t,
        }
    }

    fn as_source_mut(&mut self) -> &mut dyn TextureSource {
        match self {
            SlideChild::Image(t) => t,
            SlideChild::Video(t) => t,
        }
    }
}

/// Load a [`SlideChild`] on demand from the shade package / filesystem.
fn load_slide_child(
    entry: &SlideEntry,
    pkg: &LiveShadePackage,
) -> Result<SlideChild> {
    match entry.ty {
        SlideSourceType::Image => {
            // Read directly from the mmap-backed package (or disk)
            let bytes = read_asset_or_disk(pkg, &entry.source)
                .with_context(|| format!("Slideshow image '{}'", entry.source))?;
            let tex = ImageTexture::load(&bytes, &TextureFilter::Linear, &TextureWrap::Clamp)?;
            Ok(SlideChild::Image(tex))
        }
        SlideSourceType::Video => {
            let path = entry
                .video_path
                .as_ref()
                .context("Video slide has no file path")?;
            let tex = VideoTexture::load(path, true)?;
            Ok(SlideChild::Video(tex))
        }
    }
}

// ---------------------------------------------------------------------------
// SlideshowTexture
// ---------------------------------------------------------------------------

/// A slideshow texture that cycles through child textures on a timer.
///
/// Only the currently-active child is loaded into memory. When the timer
/// fires, the old child is dropped and the next one is loaded on demand
/// from the mmap-backed shade package.
pub struct SlideshowTexture {
    /// The shade package (mmap-backed), used to read image data on demand.
    pkg: Arc<LiveShadePackage>,
    /// Lightweight metadata for every slide.
    entries: Vec<SlideEntry>,
    /// Index of the currently active slide.
    current: usize,
    /// The loaded child for the current slide.
    current_child: Option<SlideChild>,
    /// Timer accumulator (seconds).
    timer: f64,
    /// Seconds between slides.
    interval: f64,
}

impl SlideshowTexture {
    /// Build a slideshow from a list of slide source definitions.
    ///
    /// Only resolves video paths (temp-extraction if embedded). Image data
    /// is NOT read at this point — it is decompressed from the mmap on
    /// demand when the slide becomes active.
    pub fn load(
        pkg: Arc<LiveShadePackage>,
        sources: &[SlideSource],
        interval: f64,
        shuffle: bool,
    ) -> Result<Self> {
        anyhow::ensure!(!sources.is_empty(), "Slideshow requires at least one source");

        let mut entries = Vec::with_capacity(sources.len());

        for slide in sources {
            let entry = match slide.ty {
                SlideSourceType::Image => SlideEntry {
                    source: slide.source.clone(),
                    video_path: None,
                    ty: SlideSourceType::Image,
                },
                SlideSourceType::Video => {
                    let path = resolve_video_path(&pkg, &slide.source)
                        .with_context(|| format!("Slideshow video '{}'", slide.source))?;
                    SlideEntry {
                        source: slide.source.clone(),
                        video_path: Some(path),
                        ty: SlideSourceType::Video,
                    }
                }
            };
            entries.push(entry);
        }

        // Shuffle if requested
        if shuffle && entries.len() > 1 {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as usize;
            for i in (1..entries.len()).rev() {
                let j = (seed.wrapping_mul(i).wrapping_add(7)) % (i + 1);
                entries.swap(i, j);
            }
        }

        // Load only the first child eagerly
        let first_child = load_slide_child(&entries[0], &pkg)
            .context("Failed to load first slideshow child")?;

        info!(
            "SlideshowTexture loaded: {} slides, {:.1}s interval, shuffle={}",
            entries.len(),
            interval,
            shuffle
        );

        Ok(Self {
            pkg,
            entries,
            current: 0,
            current_child: Some(first_child),
            timer: 0.0,
            interval,
        })
    }
}

impl TextureSource for SlideshowTexture {
    fn update(&mut self, dt: f64) -> Result<TextureUpdate> {
        if self.entries.is_empty() {
            return Ok(TextureUpdate::Unchanged);
        }

        // Advance timer and swap if needed
        self.timer += dt;
        if self.timer >= self.interval && self.entries.len() > 1 {
            self.timer -= self.interval;
            let next = (self.current + 1) % self.entries.len();

            info!(
                "Slideshow: advancing to slide {} of {}",
                next + 1,
                self.entries.len()
            );

            // Drop the old child (frees decoded pixels / decoder)
            self.current_child = None;
            self.current = next;

            // Load the new child on demand from the mmap
            match load_slide_child(&self.entries[self.current], &self.pkg) {
                Ok(child) => {
                    self.current_child = Some(child);
                }
                Err(e) => {
                    log::warn!(
                        "Failed to load slideshow child {}: {}",
                        self.current,
                        e
                    );
                    // Leave current_child as None — will return Unchanged
                }
            }
        }

        // Delegate to the current child
        match self.current_child.as_mut() {
            Some(child) => child.as_source_mut().update(dt),
            None => Ok(TextureUpdate::Unchanged),
        }
    }

    fn dimensions(&self) -> (u32, u32) {
        match &self.current_child {
            Some(child) => child.as_source().dimensions(),
            None => (1, 1),
        }
    }

    fn texture_type(&self) -> &'static str {
        "slideshow"
    }
}
