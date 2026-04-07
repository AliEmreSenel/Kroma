//! Slideshow texture source.
//!
//! Cycles through a list of child texture sources on a timer. Each slide is
//! a full texture definition, so a slideshow can contain image/video/shader/
//! audio/font/slideshow entries. Only the current child source is loaded at
//! any moment.

use std::sync::Arc;

use anyhow::{Context, Result};
use log::{info, warn};

use kroma_shared::shade::LiveShadePackage;
use kroma_shared::types::TextureDef;

use super::{GpuContext, TextureFormat, TextureSource, TextureUpdate};

// ---------------------------------------------------------------------------
// Slide entry — lightweight metadata kept for every slide
// ---------------------------------------------------------------------------

/// Metadata for a single slide.
struct SlideEntry {
    def: TextureDef,
}

// ---------------------------------------------------------------------------
// Active child — the currently loaded texture
// ---------------------------------------------------------------------------

/// Load a slide source on demand from the shade package / filesystem.
fn load_slide_child(
    entry: &SlideEntry,
    pkg: &Arc<LiveShadePackage>,
    hot_reload: bool,
    gpu: Option<&GpuContext>,
) -> Result<Box<dyn TextureSource>> {
    let mut def = entry.def.clone();
    def.hot_reload = def.hot_reload || hot_reload;

    let source = super::create_texture_source(pkg, &def, gpu)
        .with_context(|| {
            format!(
                "Failed to create slideshow source of type '{}'",
                def.ty.as_str()
            )
        })?
        .context("Slideshow source did not produce a texture source")?;

    Ok(source)
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
    current_child: Option<Box<dyn TextureSource>>,
    /// Timer accumulator (seconds).
    timer: f64,
    /// Seconds between slides.
    interval: f64,
    /// Whether external slide sources should auto-reload from filesystem changes.
    hot_reload: bool,
    /// Shared GPU context for slides that are GPU-managed (shader textures).
    gpu: Option<GpuContext>,
}

impl SlideshowTexture {
    /// Build a slideshow from a list of slide source definitions.
    ///
    /// Only resolves video paths (temp-extraction if embedded). Image data
    /// is NOT read at this point — it is decompressed from the mmap on
    /// demand when the slide becomes active.
    ///
    /// When `optional` is `true` and construction fails (e.g. no valid
    /// sources, or the first child cannot be loaded), the texture degrades
    /// to an empty slideshow that emits [`TextureUpdate::Unchanged`].
    pub fn load(
        pkg: Arc<LiveShadePackage>,
        sources: &[TextureDef],
        interval: f64,
        shuffle: bool,
        hot_reload: bool,
        optional: bool,
        gpu: Option<&GpuContext>,
    ) -> Result<Self> {
        let pkg_fallback = if optional {
            Some(Arc::clone(&pkg))
        } else {
            None
        };
        let inner = || -> Result<Self> {
            anyhow::ensure!(
                !sources.is_empty(),
                "Slideshow requires at least one source"
            );

            let mut entries = Vec::with_capacity(sources.len());

            for slide in sources {
                let entry = SlideEntry { def: slide.clone() };
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
            let first_child = load_slide_child(&entries[0], &pkg, hot_reload, gpu)
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
                hot_reload,
                gpu: gpu.cloned(),
            })
        };

        match inner() {
            Ok(tex) => Ok(tex),
            Err(e) if optional => {
                warn!(
                    "Optional slideshow failed to load (using placeholder): {}",
                    e
                );
                Ok(Self {
                    pkg: pkg_fallback.expect("optional=true but no fallback pkg"),
                    entries: Vec::new(),
                    current: 0,
                    current_child: None,
                    timer: 0.0,
                    interval,
                    hot_reload,
                    gpu: gpu.cloned(),
                })
            }
            Err(e) => Err(e),
        }
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
            self.timer %= self.interval;
            let next = (self.current + 1) % self.entries.len();

            info!(
                "Slideshow: advancing to slide {} of {}",
                next + 1,
                self.entries.len()
            );

            // Load the new child on demand.
            match load_slide_child(
                &self.entries[next],
                &self.pkg,
                self.hot_reload,
                self.gpu.as_ref(),
            ) {
                Ok(child) => {
                    self.current_child = Some(child);
                    self.current = next;
                }
                Err(e) => {
                    log::warn!("Failed to load slideshow child {}: {}", next, e);
                }
            }
        }

        // Delegate to the current child
        match self.current_child.as_mut() {
            Some(child) => child.update(dt),
            None => Ok(TextureUpdate::Unchanged),
        }
    }

    fn dimensions(&self) -> (u32, u32) {
        match &self.current_child {
            Some(child) => child.dimensions(),
            None => (1, 1),
        }
    }

    fn format(&self) -> TextureFormat {
        match &self.current_child {
            Some(child) => child.format(),
            None => TextureFormat::Rgba8,
        }
    }

    fn audio_level(&self) -> f32 {
        match &self.current_child {
            Some(child) => child.audio_level(),
            None => 0.0,
        }
    }

    fn texture_type(&self) -> &'static str {
        "slideshow"
    }

    fn is_gpu_managed(&self) -> bool {
        self.current_child
            .as_ref()
            .map(|child| child.is_gpu_managed())
            .unwrap_or(false)
    }

    fn gpu_texture_view(&self) -> Option<&wgpu::TextureView> {
        self.current_child
            .as_ref()
            .and_then(|child| child.gpu_texture_view())
    }

    fn gpu_sampler(&self) -> Option<&wgpu::Sampler> {
        self.current_child
            .as_ref()
            .and_then(|child| child.gpu_sampler())
    }

    fn update_uniforms(&mut self, uniforms: &kroma_shared::types::ShaderUniforms) {
        if let Some(child) = self.current_child.as_mut() {
            child.update_uniforms(uniforms);
        }
    }

    fn gpu_render(&mut self) -> Result<()> {
        if let Some(child) = self.current_child.as_mut()
            && child.is_gpu_managed()
        {
            return child.gpu_render();
        }
        Ok(())
    }
}
