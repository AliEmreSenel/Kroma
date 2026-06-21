//! Texture source system for Kroma.
//!
//! Each texture type (image, video, font, slideshow, audio, noise) implements the
//! [`TextureSource`] trait, which provides a self-contained lifecycle:
//! loading from a shade package, advancing state each frame, and
//! yielding new pixel data when the GPU texture needs updating.

pub mod audio;
pub mod ffmpeg_io;
pub mod font;
pub mod hot_reload;
pub mod image;
pub mod noise;
pub mod shader;
pub mod slideshow;
pub mod video;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use kroma_shared::shade::{AssetByteStream, LiveShadePackage};
use kroma_shared::types::{TextureDef, TextureType};

pub enum VideoSource {
    ExternalPath(PathBuf),
    EmbeddedStream(AssetByteStream),
}

// ---------------------------------------------------------------------------
// GpuContext — shared GPU resources for texture sources that need them
// ---------------------------------------------------------------------------

/// Shared GPU context for texture sources that perform their own rendering.
///
/// Passed to the factory when creating shader textures. Other texture types
/// don't need this and will simply ignore it.
#[derive(Clone)]
pub struct GpuContext {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    /// Output format of the parent surface (used by shader textures to
    /// create compatible render targets).
    #[allow(dead_code)]
    pub surface_format: wgpu::TextureFormat,
}

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
    fn texture_type(&self) -> &'static str;

    /// Whether this source manages its own GPU texture (zero-copy path).
    ///
    /// When `true`, the renderer will call [`gpu_texture_view`] and
    /// [`gpu_sampler`] instead of uploading CPU pixel data. This is used
    /// by shader textures that render to an offscreen target.
    fn is_gpu_managed(&self) -> bool {
        false
    }

    /// GPU texture view for zero-copy sources.
    ///
    /// Only called when [`is_gpu_managed`] returns `true`.
    fn gpu_texture_view(&self) -> Option<&wgpu::TextureView> {
        None
    }

    /// GPU sampler for zero-copy sources.
    ///
    /// Only called when [`is_gpu_managed`] returns `true`.
    fn gpu_sampler(&self) -> Option<&wgpu::Sampler> {
        None
    }

    /// Update system uniforms for GPU-managed sources (shader textures).
    ///
    /// Called once per frame so that sub-shaders receive the same system
    /// data (time, resolution, mouse, CPU, RAM, etc.) as the root shader.
    fn update_uniforms(&mut self, _uniforms: &kroma_shared::types::ShaderUniforms) {}

    /// Perform the GPU render pass for GPU-managed sources.
    ///
    /// Called once per frame after [`update`] and [`update_uniforms`].
    /// The source should submit its own command buffers.
    fn gpu_render(&mut self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Create a [`TextureSource`] from a shade package and texture definition.
///
/// Each texture type's `load()` method accepts `def.optional`: when `true`
/// and loading fails, the texture degrades to a transparent placeholder
/// instead of returning an error. Types with hot-reload support (Image,
/// Video) keep their filesystem watcher active so the texture resolves
/// automatically when the file appears.
///
/// `gpu` must be provided when shader textures may be present; it is
/// ignored for all other texture types.
pub fn create_texture_source(
    pkg: &Arc<LiveShadePackage>,
    def: &TextureDef,
    gpu: Option<&GpuContext>,
) -> Result<Option<Box<dyn TextureSource>>> {
    match def.ty {
        TextureType::Image => {
            let source = def
                .source
                .as_ref()
                .context("Image texture requires a `source` path")?;
            // Resolve asset bytes + disk path. When the read fails and the
            // texture is optional, pass empty bytes so ImageTexture::load
            // can degrade to a placeholder with its hot-reload watcher.
            let (bytes, disk_path) = match read_asset_or_disk_with_path(pkg, source) {
                Ok(result) => result,
                Err(_) if def.optional => (vec![], Some(PathBuf::from(source))),
                Err(e) => return Err(e),
            };
            let tex = self::image::ImageTexture::load(
                &bytes,
                &def.filter,
                &def.wrap,
                disk_path.as_deref(),
                def.hot_reload,
                def.optional,
            )?;
            Ok(Some(Box::new(tex)))
        }
        TextureType::Video => {
            let source = def
                .source
                .as_ref()
                .context("Video texture requires a `source` path")?;
            let tex = create_video_source(pkg, source, def.looping, def.hot_reload, def.optional)?;
            Ok(Some(Box::new(tex)))
        }
        TextureType::Font => {
            let source = def
                .source
                .as_ref()
                .context("Font texture requires a `source` path")?;
            let bytes = match read_asset_or_disk(pkg, source) {
                Ok(b) => b,
                Err(_) if def.optional => vec![],
                Err(e) => return Err(e),
            };
            let size = def.font_size.unwrap_or(32.0);
            let tex = self::font::FontTexture::load(&bytes, size, def.optional)?;
            Ok(Some(Box::new(tex)))
        }
        TextureType::Slideshow => {
            let interval = def.interval.unwrap_or(30.0);
            let tex = self::slideshow::SlideshowTexture::load(
                Arc::clone(pkg),
                &def.sources,
                interval,
                def.transition.as_deref(),
                def.shuffle,
                def.hot_reload,
                def.optional,
                gpu,
            )?;
            Ok(Some(Box::new(tex)))
        }
        TextureType::AudioSpectrum => {
            let source = def.source.as_deref().unwrap_or("desktop");
            let bands = def.fft_bands.unwrap_or(512);
            let tex = self::audio::AudioTexture::load(source, bands, def.optional)?;
            Ok(Some(Box::new(tex)))
        }
        TextureType::Noise => {
            let width = def.width.unwrap_or(512).max(1);
            let height = def.height.unwrap_or(512).max(1);
            let tex = self::noise::NoiseTexture::load(width, height, def.seed)?;
            Ok(Some(Box::new(tex)))
        }
        TextureType::Shader => {
            let gpu = gpu.context(
                "Shader texture requires GPU context — cannot create shader texture \
                 without a GpuContext",
            )?;
            let shader_path = def
                .shader
                .as_ref()
                .context("Shader texture requires a `shader` path")?;
            let glsl_source = read_asset_or_disk(pkg, shader_path)
                .with_context(|| format!("Failed to read shader source '{}'", shader_path))?;
            let glsl_str =
                String::from_utf8(glsl_source).context("Shader source is not valid UTF-8")?;
            let width = def.width.unwrap_or(512);
            let height = def.height.unwrap_or(512);
            let tex = self::shader::ShaderTexture::load(
                gpu,
                Arc::clone(pkg),
                &glsl_str,
                width,
                height,
                &def.textures,
                &def.uniforms,
                def.optional,
            )?;
            Ok(Some(Box::new(tex)))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read an asset from the shade package or from disk.
pub fn read_asset_or_disk(pkg: &LiveShadePackage, source: &str) -> Result<Vec<u8>> {
    let (data, _) = read_asset_or_disk_with_path(pkg, source)?;
    Ok(data)
}

/// Read an asset from the shade package or from disk, returning the resolved
/// disk path when the source is external.
pub fn read_asset_or_disk_with_path(
    pkg: &LiveShadePackage,
    source: &str,
) -> Result<(Vec<u8>, Option<PathBuf>)> {
    // 1. Try the package's embedded assets
    if let Some(data) = pkg.read_asset(source) {
        return Ok((data, None));
    }
    // 2. Try loading from disk
    let path = Path::new(source);
    if path.exists() {
        let data = std::fs::read(path)
            .with_context(|| format!("Failed to read asset from disk: {}", source))?;
        return Ok((data, Some(path.to_path_buf())));
    }
    anyhow::bail!("Asset '{}' not found in package or on disk", source)
}

/// Create a video texture source from either embedded stream data or a disk path.
pub fn create_video_source(
    pkg: &LiveShadePackage,
    source: &str,
    looping: bool,
    hot_reload: bool,
    optional: bool,
) -> Result<self::video::VideoTexture> {
    let (video_source, is_external_disk) = match resolve_video_source_with_origin(pkg, source) {
        Ok(result) => result,
        Err(_) if optional => {
            // Asset not found — pass the raw source path so the
            // VideoTexture watcher can monitor it.
            (VideoSource::ExternalPath(PathBuf::from(source)), true)
        }
        Err(e) => return Err(e),
    };
    self::video::VideoTexture::load(
        video_source,
        looping,
        hot_reload && is_external_disk,
        optional,
    )
}

/// Resolve a video source to either an embedded stream or filesystem path,
/// also returning whether the source is external on-disk media.
pub fn resolve_video_source_with_origin(
    pkg: &LiveShadePackage,
    source: &str,
) -> Result<(VideoSource, bool)> {
    // Try embedded asset first
    if let Some(stream) = pkg.open_asset_stream(source) {
        return Ok((VideoSource::EmbeddedStream(stream), false));
    }
    // Try disk path
    let path = std::path::Path::new(source);
    if path.exists() {
        return Ok((VideoSource::ExternalPath(path.to_path_buf()), true));
    }
    anyhow::bail!("Video asset '{}' not found in package or on disk", source)
}
