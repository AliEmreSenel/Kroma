//! IPC protocol for daemon ↔ GUI communication.
//!
//! Uses a Unix domain socket at `$XDG_RUNTIME_DIR/kroma.sock`.
//! Messages are newline-delimited JSON.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::mpsc};

/// Returns the path to the IPC socket.
pub fn socket_path() -> PathBuf {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("kroma.sock")
}

pub fn maybe_send<T: Send + Sync + 'static>(
    maybe_tx: Option<mpsc::Sender<T>>,
    event: T,
) -> Result<()> {
    if let Some(tx) = maybe_tx {
        tx.send(event)?
    };
    Ok(())
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

    /// Request a single preview frame from the daemon.
    /// The daemon renders at reduced resolution, JPEG-encodes, and responds
    /// with `DaemonEvent::PreviewFrame`.
    RequestPreviewFrame {
        /// Render width (e.g., 1/4 of monitor width).
        width: u32,
        /// Render height.
        height: u32,
    },

    /// Start continuous preview frame streaming at the given FPS.
    StartPreviewStream {
        width: u32,
        height: u32,
        /// Target frames per second for the preview stream.
        target_fps: u32,
    },

    /// Stop the preview frame stream.
    StopPreviewStream,
}

/// Runtime uniform value (matches types in `config.toml`).
/// Variant order matters for `#[serde(untagged)]` — more specific types first.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UniformValue {
    Bool(bool),
    Int(i64),
    Float(f64),
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

    /// A single preview frame rendered by the daemon.
    /// The image data is JPEG.
    PreviewFrame {
        /// JPEG image data.
        jpeg: Vec<u8>,
        /// Width of the rendered frame.
        width: u32,
        /// Height of the rendered frame.
        height: u32,
    },
}

/// A single compilation error with optional source location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileError {
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl From<String> for CompileError {
    fn from(value: String) -> Self {
        // shaderc pattern: "N:LINE:" where N is the source id
        // Look for two consecutive numbers separated by colon followed by colon
        if let Some(line_num) = extract_shaderc_line(&value) {
            return CompileError {
                message: value,
                line: Some(line_num),
                column: None,
            };
        }

        // Generic "line N" pattern (case-insensitive manual search)
        let lower = value.to_lowercase();
        if let Some(idx) = lower.find("line ") {
            let after = &value[idx + 5..];
            let num_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = num_str.parse::<u32>() {
                return CompileError {
                    message: value,
                    line: Some(n),
                    column: None,
                };
            }
        }

        CompileError {
            message: value,
            line: None,
            column: None,
        }
    }
}

/// Extract line number from shaderc-style error messages (e.g., "0:42: error").
fn extract_shaderc_line(msg: &str) -> Option<u32> {
    // Find patterns like "N:LINE:" where both are digits
    let bytes = msg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for a digit followed by ':'
        if bytes[i].is_ascii_digit() {
            // Skip the source ID digits
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b':' {
                i += 1;
                // Now try to parse the line number
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i > start && i < bytes.len() && bytes[i] == b':'
                    && let Ok(line) = msg[start..i].parse::<u32>() {
                        return Some(line);
                    }
            }
        }
        i += 1;
    }
    None
}
