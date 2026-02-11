//! IPC protocol for daemon ↔ GUI communication.
//!
//! Uses a Unix domain socket at `$XDG_RUNTIME_DIR/kroma.sock`.
//! Messages are newline-delimited JSON.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Returns the path to the IPC socket.
pub fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("kroma.sock")
}

// ---------------------------------------------------------------------------
// Messages: GUI → Daemon
// ---------------------------------------------------------------------------

/// Commands sent from the GUI to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonCommand {
    /// Load a .shade package file.
    LoadShade { path: String },

    /// Update a uniform value at runtime.
    SetUniform { name: String, value: UniformValue },

    /// Pause rendering (e.g., screensaver, fullscreen app).
    Pause,

    /// Resume rendering.
    Resume,

    /// Reload the currently loaded shade package.
    Reload,

    /// Gracefully shut down the daemon.
    Shutdown,

    /// Query the daemon's current status. The daemon responds with a
    /// `DaemonEvent::Status` on the same connection.
    StatusQuery,

    /// Hot-reload: push raw Shadertoy-compatible GLSL source directly
    /// to the daemon for immediate rendering (live preview).
    LiveReload { glsl_source: String },

    /// Query real-time system info (CPU, RAM, battery, cursor, audio).
    /// The daemon responds with a `DaemonEvent::SystemInfo`.
    QuerySystemInfo,
}

/// Runtime uniform value (matches types in `config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UniformValue {
    Float(f64),
    Bool(bool),
    Int(i64),
}

// ---------------------------------------------------------------------------
// Messages: Daemon → GUI
// ---------------------------------------------------------------------------

/// Events sent from the daemon to the GUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DaemonEvent {
    /// Daemon is ready and rendering.
    Ready,

    /// Shade package loaded successfully.
    ShadeLoaded { name: String },

    /// An error occurred.
    Error { message: String },

    /// Current status snapshot.
    Status {
        fps: f32,
        paused: bool,
        loaded_shade: Option<String>,
    },

    /// Result of a shader compilation attempt (from LiveReload or LoadShade).
    CompileResult {
        success: bool,
        /// Error messages with optional line numbers.
        errors: Vec<CompileError>,
        /// Non-fatal warnings.
        warnings: Vec<String>,
    },

    /// Real-time system info snapshot from the daemon.
    SystemInfo {
        cpu_usage: f32,
        ram_usage: f32,
        battery: Option<f32>,
        audio_level: f32,
        cursor_x: f32,
        cursor_y: f32,
    },
}

/// A single compilation error with optional source location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileError {
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}
