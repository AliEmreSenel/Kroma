//! Image texture source.
//!
//! Loads a PNG/JPEG/WebP/GIF image and uploads it to the GPU. For disk-backed
//! external files, it also installs a file notifier and hot-reloads the image
//! data when the file changes.

use anyhow::{Context, Result};
use log::{info, warn};

use std::path::{Path, PathBuf};

use kroma_shared::types::{TextureFilter, TextureWrap};

use super::hot_reload::SourceHotReload;
use super::{TextureSource, TextureUpdate};

/// A static image texture.
pub struct ImageTexture {
    /// Decoded RGBA8 pixel data.
    pub(crate) rgba: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// True only on the first frame (initial upload).
    pub(crate) needs_upload: bool,
    /// Optional filesystem watcher for external hot-reload.
    hot_reload: SourceHotReload,
}

impl ImageTexture {
    fn decode(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
        let img = image::load_from_memory(bytes).context("Failed to decode image")?;
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
        Ok((rgba.into_raw(), width, height))
    }

    fn reload_from_source(&mut self, path: &Path) -> Result<()> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read updated image '{}'", path.display()))?;
        let (rgba, width, height) = Self::decode(&bytes)?;
        self.rgba = rgba;
        self.width = width;
        self.height = height;
        info!(
            "ImageTexture reloaded from disk: {} ({}x{})",
            path.display(),
            width,
            height
        );
        Ok(())
    }

    /// Decode image bytes into an RGBA8 texture.
    ///
    /// If `disk_source` is provided, initial load uses the same filesystem
    /// read path as hot-reload updates.
    ///
    /// When `optional` is `true` and any loading step fails, the texture
    /// degrades to a 1×1 transparent placeholder rather than returning an
    /// error. The hot-reload watcher (if enabled) stays active so the
    /// texture resolves automatically when the file appears/changes.
    pub fn load(
        bytes: &[u8],
        _filter: &TextureFilter,
        _wrap: &TextureWrap,
        disk_source: Option<&Path>,
        hot_reload: bool,
        optional: bool,
    ) -> Result<Self> {
        let hot_reload = if hot_reload {
            match SourceHotReload::from_source(disk_source) {
                Ok(hot_reload) => hot_reload,
                Err(e) => {
                    if let Some(path) = disk_source {
                        warn!("ImageTexture watcher disabled for {}: {}", path.display(), e);
                    } else {
                        warn!("ImageTexture watcher disabled: {}", e);
                    }
                    SourceHotReload::disabled()
                }
            }
        } else {
            SourceHotReload::disabled()
        };

        let mut tex = Self {
            rgba: vec![0u8; 4], // 1×1 transparent placeholder
            width: 1,
            height: 1,
            needs_upload: true,
            hot_reload,
        };

        let initial_path: Option<PathBuf> = tex
            .hot_reload
            .source_path()
            .or(disk_source)
            .map(|p| p.to_path_buf());

        let load_result = if let Some(path) = initial_path.as_deref() {
            tex.reload_from_source(path)
        } else if !bytes.is_empty() {
            Self::decode(bytes).map(|(rgba, w, h)| {
                tex.rgba = rgba;
                tex.width = w;
                tex.height = h;
            })
        } else {
            Err(anyhow::anyhow!("No image data available"))
        };

        match load_result {
            Ok(()) => {
                if let Some(path) = initial_path.as_deref() {
                    if tex.hot_reload.source_path().is_some() {
                        info!(
                            "ImageTexture loaded ({}x{}) with watcher: {}",
                            tex.width, tex.height, path.display()
                        );
                    } else {
                        info!(
                            "ImageTexture loaded from disk ({}x{}): {}",
                            tex.width, tex.height, path.display()
                        );
                    }
                } else {
                    info!("ImageTexture loaded ({}x{})", tex.width, tex.height);
                }
            }
            Err(e) if optional => {
                warn!("Optional image failed to load (using placeholder): {}", e);
                // tex is already a 1×1 transparent placeholder; hot-reload
                // watcher (if any) stays active for when file appears.
            }
            Err(e) => return Err(e),
        }

        Ok(tex)
    }
}

impl TextureSource for ImageTexture {
    fn update(&mut self, _dt: f64) -> Result<TextureUpdate> {
        if let Some(path) = self.hot_reload.take_changed_path() {
            if let Err(e) = self.reload_from_source(&path) {
                warn!("ImageTexture reload failed: {}", e);
            } else {
                return Ok(TextureUpdate::NewFrame {
                    data: self.rgba.clone(),
                    width: self.width,
                    height: self.height,
                });
            }
        }

        if self.needs_upload {
            self.needs_upload = false;
            Ok(TextureUpdate::NewFrame {
                data: self.rgba.clone(),
                width: self.width,
                height: self.height,
            })
        } else {
            Ok(TextureUpdate::Unchanged)
        }
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn texture_type(&self) -> &'static str {
        "image"
    }
}
