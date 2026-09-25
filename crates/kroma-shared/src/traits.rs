//! Core traits that define the modular boundaries of Kroma.
//!
//! Every major subsystem is represented by a trait so that implementations
//! can be swapped at compile-time or behind feature flags.

use std::path::Path;

use anyhow::Result;
use glam::Vec2;

use crate::types::{MonitorConfig, MonitorId, SystemStats};

// ---------------------------------------------------------------------------
// SurfaceProvider — windowing backend
// ---------------------------------------------------------------------------

/// Abstraction over the display-server / windowing backend.
///
/// Implementations exist for Hyprland (Layer Shell), and in the future for
/// KDE, GNOME, and X11.
pub trait SurfaceProvider {
    /// Initialize the connection to the display server.
    fn connect(&mut self) -> Result<()>;

    fn size(&self, monitor: MonitorId) -> Result<(u32, u32)>;

    fn display_handle(&self) -> Result<raw_window_handle::RawDisplayHandle>;

    /// Create the drawing surface on the given monitor.
    ///
    /// Returns an opaque handle that can be used with `wgpu` to create a
    /// rendering surface.
    fn create_surface(&self, monitor: MonitorId) -> Result<raw_window_handle::RawWindowHandle>;

    /// Return the current monitor layout.
    fn list_monitors(&self) -> Result<Vec<MonitorConfig>>;

    /// Dispatch pending events (non-blocking).
    fn dispatch(&mut self) -> Result<()>;
}

// ---------------------------------------------------------------------------
// DataProvider — system data feeds
// ---------------------------------------------------------------------------

/// Provides normalised system data to the shader uniform buffer.
pub trait DataProvider: Send + Sync {
    /// Returns a snapshot of current system stats.
    fn get_system_stats(&self) -> SystemStats;

    /// Returns the mouse position normalised to 0.0–1.0 relative to the
    /// primary monitor.
    fn get_cursor_pos(&self) -> Vec2;
}

// ---------------------------------------------------------------------------
// VideoDecoder — video texture pipeline
// ---------------------------------------------------------------------------

/// Decodes video files frame-by-frame for use as shader textures.
pub trait VideoDecoder {
    /// Open a video file and prepare the decoding pipeline.
    fn load(path: &Path) -> Result<Self>
    where
        Self: Sized;

    /// Decode and return the next frame as an RGBA byte buffer.
    fn next_frame(&mut self) -> Option<&[u8]>;

    /// Seek to the given timestamp (in seconds). Used for looping.
    fn seek(&mut self, timestamp: f64) -> Result<()>;

    /// Returns `(width, height)` of the video stream.
    fn dimensions(&self) -> (u32, u32);
}

pub trait AudioProvider: Send + Sync {
    /// Returns a normalised audio spectrum: `SPECTRUM_BANDS` values in 0.0–1.0.
    fn get_spectrum(&self) -> Vec<f32>;

    /// Returns the current audio level (RMS, 0.0–1.0).
    fn get_level(&self) -> f32;
}

// ---------------------------------------------------------------------------
// LightingSink — external lighting output
// ---------------------------------------------------------------------------

/// A downsampled RGBA representation of the final composited render output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightingFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl LightingFrame {
    /// Creates a lighting frame after validating its RGBA byte length.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let expected_len = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .and_then(|bytes| usize::try_from(bytes).ok())
            .ok_or_else(|| anyhow::anyhow!("Lighting frame dimensions overflow"))?;
        if rgba.len() != expected_len {
            anyhow::bail!(
                "Lighting frame contains {} bytes; expected {} for {}x{} RGBA",
                rgba.len(),
                expected_len,
                width,
                height
            );
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// Returns the frame width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Returns the frame height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Returns the row-major RGBA8 pixel data.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

/// Receives final render frames for an external lighting backend.
///
/// Implementations must keep [`Self::submit_frame`] non-blocking so slow
/// devices or network connections cannot stall the renderer.
pub trait LightingSink: Send + Sync {
    /// Queues the newest composited frame, replacing any older pending frame.
    fn submit_frame(&self, frame: LightingFrame) -> Result<()>;

    /// Relinquishes control without changing the device's last colors.
    fn disconnect(&self) -> Result<()>;
}

#[cfg(test)]
mod lighting_tests {
    use super::LightingFrame;

    #[test]
    fn lighting_frame_validates_rgba_length() {
        assert!(LightingFrame::new(2, 1, vec![0; 8]).is_ok());
        assert!(LightingFrame::new(2, 1, vec![0; 7]).is_err());
    }
}
