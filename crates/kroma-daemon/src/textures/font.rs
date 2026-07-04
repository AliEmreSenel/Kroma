//! Font atlas texture source.
//!
//! Rasterizes a TrueType/OpenType font into a GPU texture atlas on load.
//! Like [`ImageTexture`](super::image::ImageTexture), the atlas is static
//! after creation — [`update`](FontTexture::update) always returns
//! [`TextureUpdate::Unchanged`] after the initial upload.

use super::{TextureSource, TextureUpdate};
use anyhow::Result;
use font8x8::UnicodeFonts;
use log::{info, warn};

/// A rasterized font atlas with glyph metrics.
#[allow(dead_code)]
pub struct FontAtlas {
    /// RGBA8 texture data for the atlas.
    pub rgba_data: Vec<u8>,
    /// Width of the atlas texture in pixels.
    pub width: u32,
    /// Height of the atlas texture in pixels.
    pub height: u32,
    /// Per-character glyph metrics.
    pub glyphs: Vec<GlyphInfo>,
    /// Number of columns in the atlas grid.
    pub cols: u32,
    /// Number of rows in the atlas grid.
    pub rows: u32,
    /// Width of each glyph cell in pixels.
    pub cell_width: u32,
    /// Height of each glyph cell in pixels.
    pub cell_height: u32,
}

/// Metrics for a single rasterized glyph.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct GlyphInfo {
    /// The Unicode character.
    pub ch: char,
    /// Glyph index in the atlas (row-major: y * cols + x).
    pub index: u32,
    /// Horizontal advance width in pixels.
    pub advance_width: f32,
    /// Offset from baseline.
    pub offset_x: f32,
    pub offset_y: f32,
    /// UV coordinates in the atlas (normalized 0..1).
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
}

/// The printable ASCII range we rasterize by default.
const ATLAS_CHARS: std::ops::RangeInclusive<u8> = 32..=126; // space through tilde

/// Rasterize a font into an atlas texture.
///
/// # Arguments
/// * `font_data` — Raw .ttf/.otf bytes
/// * `font_size` — Desired rasterization size in pixels
pub fn rasterize_font_atlas(font_data: &[u8], font_size: f32) -> Result<FontAtlas> {
    let font = fontdue::Font::from_bytes(
        font_data,
        fontdue::FontSettings {
            scale: font_size,
            ..Default::default()
        },
    )
    .map_err(|e| anyhow::anyhow!("Failed to parse font: {}", e))?;

    let chars: Vec<char> = ATLAS_CHARS.map(|b| b as char).collect();
    let num_glyphs = chars.len();

    // Rasterize all glyphs to get max dimensions
    let mut rasterized: Vec<(fontdue::Metrics, Vec<u8>)> = Vec::with_capacity(num_glyphs);
    let mut max_w: u32 = 0;
    let mut max_h: u32 = 0;

    for &ch in &chars {
        let (metrics, bitmap) = font.rasterize(ch, font_size);
        max_w = max_w.max(metrics.width as u32);
        max_h = max_h.max(metrics.height as u32);
        rasterized.push((metrics, bitmap));
    }

    // Add 1px padding around each cell
    let cell_w = max_w + 2;
    let cell_h = max_h + 2;

    // Calculate atlas grid dimensions (roughly square)
    let cols = (num_glyphs as f32).sqrt().ceil() as u32;
    let rows = (num_glyphs as u32).div_ceil(cols);

    let atlas_w = cols * cell_w;
    let atlas_h = rows * cell_h;

    // Create RGBA8 atlas (initialized to transparent black)
    let mut rgba = vec![0u8; (atlas_w * atlas_h * 4) as usize];
    let mut glyphs = Vec::with_capacity(num_glyphs);

    for (i, ((metrics, bitmap), &ch)) in rasterized.iter().zip(chars.iter()).enumerate() {
        let col = (i as u32) % cols;
        let row = (i as u32) / cols;
        let base_x = col * cell_w + 1; // +1 for padding
        let base_y = row * cell_h + 1;

        // Copy grayscale bitmap to RGBA atlas (white text with alpha from bitmap)
        for y in 0..metrics.height {
            for x in 0..metrics.width {
                let src_idx = y * metrics.width + x;
                let dst_x = base_x + x as u32;
                let dst_y = base_y + y as u32;
                let dst_idx = ((dst_y * atlas_w + dst_x) * 4) as usize;

                if dst_idx + 3 < rgba.len() && src_idx < bitmap.len() {
                    let alpha = bitmap[src_idx];
                    rgba[dst_idx] = 255; // R
                    rgba[dst_idx + 1] = 255; // G
                    rgba[dst_idx + 2] = 255; // B
                    rgba[dst_idx + 3] = alpha; // A
                }
            }
        }

        let uv_min = [
            base_x as f32 / atlas_w as f32,
            base_y as f32 / atlas_h as f32,
        ];
        let uv_max = [
            (base_x + metrics.width as u32) as f32 / atlas_w as f32,
            (base_y + metrics.height as u32) as f32 / atlas_h as f32,
        ];

        glyphs.push(GlyphInfo {
            ch,
            index: i as u32,
            advance_width: metrics.advance_width,
            offset_x: metrics.xmin as f32,
            offset_y: metrics.ymin as f32,
            uv_min,
            uv_max,
        });
    }

    info!(
        "Font atlas rasterized: {}x{} ({} glyphs, {:.0}px, {} cols × {} rows)",
        atlas_w, atlas_h, num_glyphs, font_size, cols, rows
    );

    Ok(FontAtlas {
        rgba_data: rgba,
        width: atlas_w,
        height: atlas_h,
        glyphs,
        cols,
        rows,
        cell_width: cell_w,
        cell_height: cell_h,
    })
}

// ---------------------------------------------------------------------------
// font8x8 fallback atlas
// ---------------------------------------------------------------------------

/// Scale factor applied to each 8×8 font8x8 glyph.
const F8_SCALE: u32 = 2;

/// Build a [`FontAtlas`] from the built-in `font8x8` bitmap font.
///
/// Used as a fallback when the real font file fails to load on an optional
/// texture. The atlas covers the same printable ASCII range as the main
/// rasterizer and produces white-on-transparent RGBA8 data.
pub fn rasterize_font8x8_atlas() -> FontAtlas {
    let chars: Vec<char> = ATLAS_CHARS.map(|b| b as char).collect();
    let num_glyphs = chars.len();

    let glyph_w: u32 = 8 * F8_SCALE;
    let glyph_h: u32 = 8 * F8_SCALE;
    let cell_w = glyph_w + 2; // 1px padding each side
    let cell_h = glyph_h + 2;

    let cols = (num_glyphs as f32).sqrt().ceil() as u32;
    let rows = (num_glyphs as u32).div_ceil(cols);

    let atlas_w = cols * cell_w;
    let atlas_h = rows * cell_h;

    let mut rgba = vec![0u8; (atlas_w * atlas_h * 4) as usize];
    let mut glyphs = Vec::with_capacity(num_glyphs);

    for (i, &ch) in chars.iter().enumerate() {
        let col = (i as u32) % cols;
        let row = (i as u32) / cols;
        let base_x = col * cell_w + 1;
        let base_y = row * cell_h + 1;

        // Render the 8×8 glyph (scaled) into the atlas cell.
        if let Some(bitmap) = font8x8::BASIC_FONTS.get(ch) {
            for (gy, &glyph_row) in bitmap.iter().enumerate() {
                for gx in 0..8u32 {
                    if glyph_row & (1 << gx) != 0 {
                        // Fill the scaled pixel block.
                        for sy in 0..F8_SCALE {
                            for sx in 0..F8_SCALE {
                                let px = base_x + gx * F8_SCALE + sx;
                                let py = base_y + (gy as u32) * F8_SCALE + sy;
                                let idx = ((py * atlas_w + px) * 4) as usize;
                                if idx + 3 < rgba.len() {
                                    rgba[idx] = 255; // R
                                    rgba[idx + 1] = 255; // G
                                    rgba[idx + 2] = 255; // B
                                    rgba[idx + 3] = 255; // A
                                }
                            }
                        }
                    }
                }
            }
        }

        let uv_min = [
            base_x as f32 / atlas_w as f32,
            base_y as f32 / atlas_h as f32,
        ];
        let uv_max = [
            (base_x + glyph_w) as f32 / atlas_w as f32,
            (base_y + glyph_h) as f32 / atlas_h as f32,
        ];

        glyphs.push(GlyphInfo {
            ch,
            index: i as u32,
            advance_width: glyph_w as f32,
            offset_x: 0.0,
            offset_y: 0.0,
            uv_min,
            uv_max,
        });
    }

    info!(
        "font8x8 fallback atlas rasterized: {}x{} ({} glyphs, {}x scale)",
        atlas_w, atlas_h, num_glyphs, F8_SCALE
    );

    FontAtlas {
        rgba_data: rgba,
        width: atlas_w,
        height: atlas_h,
        glyphs,
        cols,
        rows,
        cell_width: cell_w,
        cell_height: cell_h,
    }
}

/// A rasterized font atlas texture.
pub struct FontTexture {
    /// RGBA8 atlas pixel data.
    rgba: Option<Vec<u8>>,
    width: u32,
    height: u32,
    /// True only on the first frame (initial upload).
    needs_upload: bool,
}

impl FontTexture {
    /// Rasterize a font into an atlas texture.
    ///
    /// When `optional` is `true` and rasterization fails, the texture
    /// degrades to a 1×1 transparent placeholder instead of returning an
    /// error.
    pub fn load(font_data: &[u8], font_size: f32, optional: bool) -> Result<Self> {
        match rasterize_font_atlas(font_data, font_size) {
            Ok(atlas) => {
                let width = atlas.width;
                let height = atlas.height;
                let rgba = atlas.rgba_data;
                info!(
                    "FontTexture loaded ({}x{}, {:.0}px)",
                    width, height, font_size
                );
                Ok(Self {
                    rgba: Some(rgba),
                    width,
                    height,
                    needs_upload: true,
                })
            }
            Err(e) if optional => {
                warn!(
                    "Optional font failed to load (using font8x8 fallback): {}",
                    e
                );
                let atlas = rasterize_font8x8_atlas();
                Ok(Self {
                    width: atlas.width,
                    height: atlas.height,
                    rgba: Some(atlas.rgba_data),
                    needs_upload: true,
                })
            }
            Err(e) => Err(e),
        }
    }
}

impl TextureSource for FontTexture {
    fn update(&mut self, _dt: f64) -> Result<TextureUpdate> {
        if self.needs_upload {
            self.needs_upload = false;
            if let Some(data) = self.rgba.take() {
                Ok(TextureUpdate::NewFrame {
                    data,
                    width: self.width,
                    height: self.height,
                })
            } else {
                Ok(TextureUpdate::Unchanged)
            }
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
