//! General-purpose helper functions for the Kroma GUI.

use std::path::PathBuf;

use crate::dock;
use crate::importer;

/// Compute the (start, size) of a split node along its axis, given the path
/// to it through the dock tree and the current window dimensions.
///
/// This walks the tree from root to the target split, accumulating the
/// available bounds at each level so we can convert absolute cursor position
/// into a ratio for the dragged divider.
pub(crate) fn compute_split_bounds(
    root: &dock::tree::DockNode,
    target_path: &[dock::PathDir],
    target_axis: dock::tree::SplitAxis,
    window_w: f32,
    window_h: f32,
) -> (f32, f32) {
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    let mut w = window_w;
    let mut h = window_h;
    let mut node = root;
    let divider_px = 6.0; // match the 6px divider hit area

    for &dir in target_path {
        if let dock::tree::DockNode::Split {
            axis,
            ratio,
            left,
            right,
        } = node
        {
            match axis {
                dock::tree::SplitAxis::Horizontal => {
                    // left takes ratio * (w - divider), right gets the rest
                    let usable = w - divider_px;
                    let left_w = usable * ratio;
                    match dir {
                        dock::PathDir::Left => {
                            w = left_w;
                            node = left;
                        }
                        dock::PathDir::Right => {
                            x += left_w + divider_px;
                            w = usable - left_w;
                            node = right;
                        }
                    }
                }
                dock::tree::SplitAxis::Vertical => {
                    let usable = h - divider_px;
                    let top_h = usable * ratio;
                    match dir {
                        dock::PathDir::Left => {
                            h = top_h;
                            node = left;
                        }
                        dock::PathDir::Right => {
                            y += top_h + divider_px;
                            h = usable - top_h;
                            node = right;
                        }
                    }
                }
            }
        } else {
            break;
        }
    }

    // Now we're at the split being dragged
    match target_axis {
        dock::tree::SplitAxis::Horizontal => (x, w),
        dock::tree::SplitAxis::Vertical => (y, h),
    }
}

pub(crate) fn do_import(path: &str, name: &str, author: &str) -> String {
    let p = PathBuf::from(path);
    if !p.exists() {
        return format!("File not found: {}", p.display());
    }
    match importer::import_shadertoy_file(&p, name, author) {
        Ok(out) => format!("Created: {}", out.display()),
        Err(e) => format!("Import failed: {}", e),
    }
}

pub(crate) async fn do_download(url_or_id: &str) -> String {
    let dir = dirs_output_dir();
    match importer::download_shadertoy(url_or_id, &dir, None).await {
        Ok((path, name)) => format!("Downloaded '{}': {}", name, path.display()),
        Err(e) => format!("Download failed: {}", e),
    }
}

pub(crate) fn dirs_output_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(d).join("kroma/shaders")
    } else if let Some(h) = std::env::var_os("HOME") {
        PathBuf::from(h).join(".local/share/kroma/shaders")
    } else {
        PathBuf::from("./shaders")
    }
}

/// Map a file extension to a unicode emoji icon.
pub(crate) fn shade_file_icon(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("");
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" | "png" | "bmp" | "gif" | "webp" => "\u{1F5BC}",
        "mp4" | "webm" | "avi" | "mkv" => "\u{1F3AC}",
        "mp3" | "wav" | "ogg" | "flac" => "\u{1F3B5}",
        "ttf" | "otf" | "woff" | "woff2" => "\u{1F524}",
        "glsl" | "frag" | "vert" => "\u{1F4DD}",
        _ => "\u{1F4C4}",
    }
}

/// Get current local time as HH:MM:SS string.
pub(crate) fn chrono_now() -> String {
    // Use libc::localtime_r for local timezone instead of raw UTC epoch math
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let epoch = now.as_secs() as i64;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&epoch, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// Convert HSV (h in 0..1, s in 0..1, v in 0..1) to RGB (0..1).
pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let i = (h * 6.0).floor() as i32;
    let f = h * 6.0 - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}
