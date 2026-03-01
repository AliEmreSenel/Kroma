//! Font atlas texture source.
//!
//! Rasterizes a TrueType/OpenType font into a GPU texture atlas on load.
//! Like [`ImageTexture`](super::image::ImageTexture), the atlas is static
//! after creation — [`update`](FontTexture::update) always returns
//! [`TextureUpdate::Unchanged`] after the initial upload.

use anyhow::Result;
use log::info;

use crate::font;

use super::{TextureSource, TextureUpdate};

/// A rasterized font atlas texture.
pub struct FontTexture {
    /// RGBA8 atlas pixel data.
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    /// True only on the first frame (initial upload).
    needs_upload: bool,
}

impl FontTexture {
    /// Rasterize a font into an atlas texture.
    pub fn load(font_data: &[u8], font_size: f32) -> Result<Self> {
        let atlas = font::rasterize_font_atlas(font_data, font_size)?;
        let width = atlas.width;
        let height = atlas.height;
        let rgba = atlas.rgba_data;
        info!("FontTexture loaded ({}x{}, {:.0}px)", width, height, font_size);
        Ok(Self {
            rgba,
            width,
            height,
            needs_upload: true,
        })
    }
}

impl TextureSource for FontTexture {
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
        "font"
    }
}
