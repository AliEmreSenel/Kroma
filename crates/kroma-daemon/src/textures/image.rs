//! Static image texture source.
//!
//! Loads a PNG/JPEG/WebP/GIF image once and never changes. The
//! [`update`](ImageTexture::update) method always returns
//! [`TextureUpdate::Unchanged`].

use anyhow::{Context, Result};
use log::info;

use kroma_shared::types::{TextureFilter, TextureWrap};

use super::{TextureSource, TextureUpdate};

/// A static image texture.
pub struct ImageTexture {
    /// Decoded RGBA8 pixel data.
    pub(crate) rgba: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// True only on the first frame (initial upload).
    pub(crate) needs_upload: bool,
}

impl ImageTexture {
    /// Decode image bytes into an RGBA8 texture.
    pub fn load(bytes: &[u8], _filter: &TextureFilter, _wrap: &TextureWrap) -> Result<Self> {
        let img = image::load_from_memory(bytes).context("Failed to decode image")?;
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
        info!("ImageTexture loaded ({}x{})", width, height);
        Ok(Self {
            rgba: rgba.into_raw(),
            width,
            height,
            needs_upload: true,
        })
    }
}

impl TextureSource for ImageTexture {
    fn update(&mut self, _dt: f64) -> Result<TextureUpdate> {
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
