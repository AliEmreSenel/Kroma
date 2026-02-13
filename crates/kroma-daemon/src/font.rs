//! Font atlas generation for Kroma.
//!
//! Rasterizes TrueType/OpenType font glyphs into a GPU texture atlas
//! that shaders can sample for text rendering.

#![allow(dead_code)]

use anyhow::Result;
use log::info;

/// A rasterized font atlas with glyph metrics.
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

    for (i, ((metrics, bitmap), &ch)) in
        rasterized.iter().zip(chars.iter()).enumerate()
    {
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
                    rgba[dst_idx] = 255;     // R
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
