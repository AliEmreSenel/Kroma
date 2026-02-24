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
pub trait VideoDecoder: Send {
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
