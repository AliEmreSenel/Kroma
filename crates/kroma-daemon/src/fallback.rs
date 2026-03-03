//! CPU-based text bitmap generation using `font8x8` for GPU fallback shaders.
//!
//! Generates a single-channel "text bitmap" texture where each pixel encodes
//! the text region type (title / subtitle / path) via the R channel value.
//! The actual visual rendering — background, text coloring, vignette, shadow —
//! is performed entirely by the companion WGSL fallback shader on the GPU.
//!
//! Region codes (Rgba8Unorm, R channel):
//!   0   → no text (background)
//!   85  → title text  (≈ 0.333 when normalized)
//!   170 → subtitle text (≈ 0.667)
//!   255 → path / detail text (≈ 1.0)

use font8x8::UnicodeFonts;

// ---------------------------------------------------------------------------
// Region codes (stored in R channel of Rgba8Unorm texels)
// ---------------------------------------------------------------------------

/// Title text (red).
const REGION_TITLE: u8 = 85;
/// Subtitle / description text (gray).
const REGION_SUBTITLE: u8 = 170;
/// File-path / detail text (light).
const REGION_PATH: u8 = 255;

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

/// Scale factor for 8×8 font glyphs.
const GLYPH_SCALE: u32 = 3;
/// Scaled glyph width in pixels.
const GLYPH_W: u32 = 8 * GLYPH_SCALE;
/// Scaled glyph height in pixels.
const GLYPH_H: u32 = 8 * GLYPH_SCALE;
/// Vertical gap between text lines.
const LINE_SPACING: u32 = 8;
/// Horizontal margin from left edge.
const MARGIN_X: u32 = 48;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a text bitmap for **required texture load failures**.
///
/// Returns `Rgba8Unorm` pixel data (`width × height × 4` bytes) where the R
/// channel encodes the text region. Upload with
/// `wgpu::TextureFormat::Rgba8Unorm` so the GPU shader reads exact values.
pub fn render_texture_error(paths: &[String], width: u32, height: u32) -> Vec<u8> {
    render_text_bitmap(
        "TEXTURE LOAD ERROR",
        "Required texture(s) could not be loaded:",
        paths,
        width,
        height,
    )
}

/// Render a text bitmap for **shader compile / load failures**.
pub fn render_load_error(details: &str, paths: &[String], width: u32, height: u32) -> Vec<u8> {
    render_text_bitmap("SHADER LOAD ERROR", details, paths, width, height)
}

/// Render a text bitmap for the **no shade loaded** fallback.
pub fn render_no_shade(width: u32, height: u32) -> Vec<u8> {
    render_text_bitmap(
        "NO SHADE LOADED",
        "Load a .shade file to start rendering.",
        &[],
        width,
        height,
    )
}

// ---------------------------------------------------------------------------
// Internal
// ---------------------------------------------------------------------------

/// Lay out the error message using `font8x8` glyphs and encode region types.
fn render_text_bitmap(
    title: &str,
    subtitle: &str,
    paths: &[String],
    width: u32,
    height: u32,
) -> Vec<u8> {
    let w = width.max(1);
    let h = height.max(1);
    // All zeroes = "no text" (region code 0).
    let mut pixels = vec![0u8; (w * h * 4) as usize];

    let line_h = GLYPH_H + LINE_SPACING;
    let max_chars = ((w.saturating_sub(MARGIN_X * 2)) / GLYPH_W).max(1) as usize;

    // Word-wrap subtitle and path lines.
    let subtitle_lines = wrap_text(subtitle, max_chars);
    let path_lines: Vec<Vec<String>> = paths
        .iter()
        .map(|p| wrap_text(&format!("> {}", p), max_chars))
        .collect();

    // Vertical centering (⅓ from top for aesthetic bias).
    let total_lines: usize = 1                                    // title
        + 1                                                       // gap
        + subtitle_lines.len()
        + 1                                                       // gap
        + path_lines.iter().map(|v| v.len()).sum::<usize>();
    let total_height = total_lines as u32 * line_h;
    let start_y = h.saturating_sub(total_height) / 3;

    let mut y = start_y;

    // — title —
    draw_text_region(&mut pixels, w, h, MARGIN_X, y, title, REGION_TITLE);
    y += line_h * 2;

    // — subtitle —
    for line in &subtitle_lines {
        draw_text_region(&mut pixels, w, h, MARGIN_X, y, line, REGION_SUBTITLE);
        y += line_h;
    }
    y += line_h;

    // — paths —
    for lines in &path_lines {
        for line in lines {
            draw_text_region(&mut pixels, w, h, MARGIN_X + GLYPH_W, y, line, REGION_PATH);
            y += line_h;
        }
    }

    pixels
}

/// Word-wrap `text` to fit within `max_chars` columns.
fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    if text.len() <= max_chars {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut remaining = text;
    while !remaining.is_empty() {
        if remaining.len() <= max_chars {
            lines.push(remaining.to_string());
            break;
        }
        let break_at = remaining[..max_chars].rfind(' ').unwrap_or(max_chars);
        let (chunk, rest) = remaining.split_at(break_at);
        lines.push(chunk.to_string());
        remaining = rest.trim_start();
    }
    lines
}

/// Render a line of text into the bitmap using `font8x8` glyphs.
fn draw_text_region(
    pixels: &mut [u8],
    img_w: u32,
    img_h: u32,
    x: u32,
    y: u32,
    text: &str,
    region: u8,
) {
    let mut cx = x;
    for ch in text.chars() {
        if cx + GLYPH_W > img_w {
            break;
        }
        if let Some(glyph) = font8x8::BASIC_FONTS.get(ch) {
            draw_glyph(pixels, img_w, img_h, cx, y, &glyph, region);
        }
        cx += GLYPH_W;
    }
}

/// Stamp a single 8×8 glyph (scaled) into the bitmap.
fn draw_glyph(
    pixels: &mut [u8],
    img_w: u32,
    img_h: u32,
    x: u32,
    y: u32,
    glyph: &[u8; 8],
    region: u8,
) {
    for row in 0..8u32 {
        let bits = glyph[row as usize];
        for col in 0..8u32 {
            if (bits >> col) & 1 != 0 {
                for sy in 0..GLYPH_SCALE {
                    for sx in 0..GLYPH_SCALE {
                        let px = x + col * GLYPH_SCALE + sx;
                        let py = y + row * GLYPH_SCALE + sy;
                        if px < img_w && py < img_h {
                            let idx = ((py * img_w + px) * 4) as usize;
                            if idx + 3 < pixels.len() {
                                pixels[idx] = region;   // R = region code
                                pixels[idx + 1] = 0;    // G (unused)
                                pixels[idx + 2] = 0;    // B (unused)
                                pixels[idx + 3] = 255;  // A
                            }
                        }
                    }
                }
            }
        }
    }
}
