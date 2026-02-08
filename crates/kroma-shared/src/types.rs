//! Core types shared across the Kroma engine.

use bytemuck::{Pod, Zeroable};
use glam::Vec2;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Monitor types
// ---------------------------------------------------------------------------

/// Opaque monitor identifier (index-based for now).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MonitorId(pub u32);

/// Describes a single monitor in the current display layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorConfig {
    pub id: MonitorId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
}

// ---------------------------------------------------------------------------
// System stats
// ---------------------------------------------------------------------------

/// Snapshot of system resource usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemStats {
    /// CPU usage percentage (0.0 – 100.0).
    pub cpu_usage: f32,
    /// Total RAM in bytes.
    pub ram_total: u64,
    /// Used RAM in bytes.
    pub ram_used: u64,
    /// Battery charge percentage (0.0 – 100.0), `None` if no battery.
    pub battery: Option<f32>,
}

// ---------------------------------------------------------------------------
// Shader uniform buffer (GPU-side)
// ---------------------------------------------------------------------------

/// The uniform buffer uploaded to the GPU each frame.
/// Must be kept in sync with the shader `Globals` struct.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct ShaderUniforms {
    /// Elapsed time in seconds.
    pub u_time: f32,
    /// Time since last frame in seconds.
    pub u_delta_time: f32,
    /// Current frame index.
    pub u_frame: u32,
    /// Padding for alignment.
    pub _pad0: u32,
    /// Viewport resolution (width, height).
    pub u_resolution: [f32; 2],
    /// Normalised mouse position (0.0 – 1.0).
    pub u_mouse: [f32; 2],
    /// CPU usage (0.0 – 1.0).
    pub u_cpu: f32,
    /// RAM usage (0.0 – 1.0).
    pub u_ram: f32,
    /// Battery (0.0 – 1.0, negative if unavailable).
    pub u_battery: f32,
    /// Audio level from microphone/desktop audio (0.0 – 1.0).
    pub u_audio_level: f32,
}

impl Default for ShaderUniforms {
    fn default() -> Self {
        Self {
            u_time: 0.0,
            u_delta_time: 0.0,
            u_frame: 0,
            _pad0: 0,
            u_resolution: [1920.0, 1080.0],
            u_mouse: [0.5, 0.5],
            u_cpu: 0.0,
            u_ram: 0.0,
            u_battery: -1.0,
            u_audio_level: 0.0,
        }
    }
}

impl ShaderUniforms {
    /// Update from a `SystemStats` snapshot.
    pub fn apply_system_stats(&mut self, stats: &SystemStats) {
        self.u_cpu = stats.cpu_usage / 100.0;
        if stats.ram_total > 0 {
            self.u_ram = stats.ram_used as f32 / stats.ram_total as f32;
        }
        self.u_battery = stats.battery.map(|b| b / 100.0).unwrap_or(-1.0);
    }

    /// Update from a normalised cursor position.
    pub fn apply_cursor(&mut self, pos: Vec2) {
        self.u_mouse = [pos.x, pos.y];
    }
}

// ---------------------------------------------------------------------------
// Shade package config (config.toml inside .shade)
// ---------------------------------------------------------------------------

/// Root of a `.shade` package's `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadeConfig {
    pub meta: ShadeMeta,
    #[serde(default)]
    pub uniforms: std::collections::HashMap<String, UniformDef>,
    #[serde(default)]
    pub textures: std::collections::HashMap<String, TextureDef>,
}

/// Package metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadeMeta {
    pub name: String,
    pub author: String,
    #[serde(default = "default_version")]
    pub version: String,
}

fn default_version() -> String {
    "1.0".into()
}

/// A user-tweakable uniform definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniformDef {
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub default: Option<toml::Value>,
}

/// A texture channel binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureDef {
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default, rename = "loop")]
    pub looping: bool,
}
