//! Texture source system for Kroma.
//!
//! Each texture type (image, video, font, slideshow, audio) implements the
//! [`TextureSource`] trait, which provides a self-contained lifecycle:
//! loading from a shade package, advancing state each frame, and
//! yielding new pixel data when the GPU texture needs updating.

pub mod audio;
pub mod font;
pub mod image;
pub mod slideshow;
pub mod video;

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};

use kroma_shared::shade::LiveShadePackage;
use kroma_shared::types::{TextureDef, TextureType};

// ---------------------------------------------------------------------------
// TextureFormat — pixel format of the texture data
// ---------------------------------------------------------------------------

/// Pixel format that a [`TextureSource`] produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    /// Standard 4-byte-per-pixel RGBA (images, videos, fonts).
    Rgba8,
    /// Single-channel 32-bit float (audio spectrum).
    R32Float,
}

impl TextureFormat {
    /// Bytes per pixel for this format.
    pub fn bytes_per_pixel(&self) -> u32 {
        match self {
            Self::Rgba8 => 4,
            Self::R32Float => 4,
        }
    }

    /// Corresponding wgpu texture format.
    pub fn wgpu_format(&self) -> wgpu::TextureFormat {
        match self {
            Self::Rgba8 => wgpu::TextureFormat::Rgba8UnormSrgb,
            Self::R32Float => wgpu::TextureFormat::R32Float,
        }
    }
}

// ---------------------------------------------------------------------------
// TextureUpdate — returned by TextureSource::update()
// ---------------------------------------------------------------------------

/// Describes what happened after a texture source's per-frame update.
pub enum TextureUpdate {
    /// No new pixel data — the GPU texture is still valid.
    Unchanged,
    /// New pixel data ready for upload (format depends on source).
    NewFrame {
        data: Vec<u8>,
        width: u32,
        height: u32,
    },
}

// ---------------------------------------------------------------------------
// TextureSource trait
// ---------------------------------------------------------------------------

/// A self-contained texture lifecycle.
///
/// Implementations manage their own timing, decoding, and frame production.
/// The renderer simply calls [`update`](TextureSource::update) each frame
/// and uploads any new pixel data to the GPU.
pub trait TextureSource {
    /// Advance the texture state by `dt` seconds.
    ///
    /// Returns [`TextureUpdate::NewFrame`] when the GPU texture should be
    /// re-uploaded, or [`TextureUpdate::Unchanged`] otherwise.
    fn update(&mut self, dt: f64) -> Result<TextureUpdate>;

    /// Current pixel dimensions `(width, height)`.
    fn dimensions(&self) -> (u32, u32);

    /// Pixel format produced by this source.
    fn format(&self) -> TextureFormat {
        TextureFormat::Rgba8
    }

    /// Audio level (RMS, 0.0–1.0). Only meaningful for audio textures.
    fn audio_level(&self) -> f32 {
        0.0
    }

    /// Human-readable type name (for logging).
    #[allow(dead_code)]
    fn texture_type(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create a [`TextureSource`] from a shade package and texture definition.
pub fn create_texture_source(
    pkg: &Arc<LiveShadePackage>,
    def: &TextureDef,
) -> Result<Option<Box<dyn TextureSource>>> {
    match def.ty {
        TextureType::Image => {
            let source = def
                .source
                .as_ref()
                .context("Image texture requires a `source` path")?;
            let bytes = read_asset_or_disk(pkg, source)?;
            let tex = self::image::ImageTexture::load(&bytes, &def.filter, &def.wrap)?;
            Ok(Some(Box::new(tex)))
        }
        TextureType::Video => {
            let source = def
                .source
                .as_ref()
                .context("Video texture requires a `source` path")?;
            let tex = create_video_source(pkg, source, def.looping)?;
            Ok(Some(Box::new(tex)))
        }
        TextureType::Font => {
            let source = def
                .source
                .as_ref()
                .context("Font texture requires a `source` path")?;
            let bytes = read_asset_or_disk(pkg, source)?;
            let size = def.font_size.unwrap_or(32.0);
            let tex = self::font::FontTexture::load(&bytes, size)?;
            Ok(Some(Box::new(tex)))
        }
        TextureType::Slideshow => {
            let interval = def.interval.unwrap_or(30.0);
            let tex = self::slideshow::SlideshowTexture::load(
                Arc::clone(pkg),
                &def.sources,
                interval,
                def.shuffle,
            )?;
            Ok(Some(Box::new(tex)))
        }
        TextureType::AudioSpectrum => {
            let source = def
                .source
                .as_deref()
                .unwrap_or("desktop");
            let bands = def.fft_bands.unwrap_or(512);
            let tex = self::audio::AudioTexture::load(source, bands)?;
            Ok(Some(Box::new(tex)))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read an asset from the shade package or from disk.
pub fn read_asset_or_disk(pkg: &LiveShadePackage, source: &str) -> Result<Vec<u8>> {
    // 1. Try the package's embedded assets (ZIP)
    if let Some(data) = pkg.read_asset(source) {
        return Ok(data);
    }
    // 2. Try loading from disk
    let path = Path::new(source);
    if path.exists() {
        let data = std::fs::read(path)
            .with_context(|| format!("Failed to read asset from disk: {}", source))?;
        return Ok(data);
    }
    anyhow::bail!("Asset '{}' not found in package or on disk", source)
}

/// Create a video texture source, extracting embedded data to a temp file
/// if needed (FFmpeg requires a file path).
pub fn create_video_source(
    pkg: &LiveShadePackage,
    source: &str,
    looping: bool,
) -> Result<self::video::VideoTexture> {
    let path = resolve_video_path(pkg, source)?;
    self::video::VideoTexture::load(&path, looping)
}

/// Resolve a video source to a filesystem path.
///
/// If the video is embedded in the package, extracts it to a temp file.
/// If it's already a path on disk, returns that directly.
pub fn resolve_video_path(
    pkg: &LiveShadePackage,
    source: &str,
) -> Result<std::path::PathBuf> {
    // Try embedded asset first
    if let Some(data) = pkg.read_asset(source) {
        return extract_video_to_temp(source, &data);
    }
    // Try disk path
    let path = std::path::Path::new(source);
    if path.exists() {
        return Ok(path.to_path_buf());
    }
    anyhow::bail!("Video asset '{}' not found in package or on disk", source)
}

/// Extract embedded video data to a temp file so FFmpeg can open it.
fn extract_video_to_temp(source: &str, data: &[u8]) -> Result<std::path::PathBuf> {
    let extension = Path::new(source)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4");
    let temp_dir = std::env::temp_dir().join("kroma-video");
    std::fs::create_dir_all(&temp_dir)?;
    let hash = {
        let mut h = DefaultHasher::new();
        source.hash(&mut h);
        data.len().hash(&mut h);
        h.finish()
    };
    let temp_path = temp_dir.join(format!("kroma-video_{:016x}.{}", hash, extension));
    std::fs::write(&temp_path, data)?;
    log::info!("Extracted video to temp: {}", temp_path.display());
    Ok(temp_path)
}
