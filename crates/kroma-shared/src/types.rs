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
    /// Padding for vec4 alignment of u_mouse.
    pub _pad1: [f32; 2],
    /// Mouse position and click state (x, y, click_x, click_y).
    /// Matches Shadertoy iMouse semantics.
    pub u_mouse: [f32; 4],
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
            _pad1: [0.0, 0.0],
            u_mouse: [0.0, 0.0, 0.0, 0.0],
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
        self.u_mouse[0] = pos.x;
        self.u_mouse[1] = pos.y;
    }
}

// ---------------------------------------------------------------------------
// Shade package config (config.toml inside .shade)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Wallpaper mode — what kind of wallpaper this shade package provides
// ---------------------------------------------------------------------------

/// The type of wallpaper this shade package provides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum WallpaperMode {
    /// Custom fragment shader (may optionally reference textures).
    #[default]
    Shader,
    /// Static image wallpaper — no shader needed.
    Image,
    /// Video loop wallpaper — no shader needed.
    Video,
    /// Slideshow of images.
    Slideshow,
}

/// Root of a `.shade` package's `config.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadeConfig {
    pub meta: ShadeMeta,
    /// Wallpaper mode. Defaults to "shader" for backwards compatibility.
    #[serde(default)]
    pub mode: WallpaperMode,
    #[serde(default)]
    pub rendering: RenderingConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub uniforms: std::collections::HashMap<String, UniformDef>,
    #[serde(default)]
    pub textures: std::collections::HashMap<String, TextureDef>,
    #[serde(default)]
    pub slideshow: SlideshowConfig,
    #[serde(default)]
    pub fonts: std::collections::HashMap<String, FontDef>,
    /// Render buffer passes (multi-pass shaders, Shadertoy-style).
    /// Keys are buffer names like "A", "B", "C", "D".
    #[serde(default)]
    pub buffers: std::collections::HashMap<String, BufferDef>,
}

/// Package metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadeMeta {
    pub name: String,
    pub author: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_version() -> String {
    "1.0".into()
}

/// Rendering preferences (optional section in config.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderingConfig {
    /// Target FPS. 0 = match monitor refresh rate (default).
    #[serde(default)]
    pub target_fps: u32,
    /// Whether to pause when the workspace is not visible.
    #[serde(default = "bool_true")]
    pub pause_offscreen: bool,
    /// Whether to pause when a fullscreen window is active.
    #[serde(default = "bool_true")]
    pub pause_fullscreen: bool,
}

impl Default for RenderingConfig {
    fn default() -> Self {
        Self {
            target_fps: 0,
            pause_offscreen: true,
            pause_fullscreen: true,
        }
    }
}

/// Audio capture configuration (optional section in config.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Enable audio capture for this shade.
    #[serde(default)]
    pub enabled: bool,
    /// Audio source: "desktop" (monitor/loopback), "microphone", or a device name.
    /// Default is "desktop".
    #[serde(default = "default_audio_source")]
    pub source: String,
    /// Number of FFT bands (default 512).
    #[serde(default = "default_fft_bands")]
    pub fft_bands: usize,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            source: "desktop".into(),
            fft_bands: 512,
        }
    }
}

fn bool_true() -> bool {
    true
}
fn default_audio_source() -> String {
    "desktop".into()
}
fn default_fft_bands() -> usize {
    512
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

/// Texture filtering mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum TextureFilter {
    #[default]
    Linear,
    Nearest,
}

/// Texture wrapping (address) mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum TextureWrap {
    Repeat,
    #[default]
    Clamp,
    Mirror,
}

/// A texture channel binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureDef {
    /// "image", "video", or "audio_spectrum".
    #[serde(rename = "type")]
    pub ty: String,
    /// Path to the asset file (relative to package root, e.g. "assets/bg.png").
    #[serde(default)]
    pub source: Option<String>,
    /// Whether video textures loop.
    #[serde(default, rename = "loop")]
    pub looping: bool,
    /// Texture filtering mode.
    #[serde(default)]
    pub filter: TextureFilter,
    /// Texture address / wrapping mode.
    #[serde(default)]
    pub wrap: TextureWrap,
    /// GPU binding index (0-based). Auto-assigned if unset.
    #[serde(default)]
    pub binding: Option<u32>,
}

/// Slideshow configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideshowConfig {
    /// Interval between slides in seconds.
    #[serde(default = "default_slideshow_interval")]
    pub interval: f64,
    /// Whether to shuffle the order.
    #[serde(default)]
    pub shuffle: bool,
    /// Crossfade duration in seconds (0 = instant).
    #[serde(default)]
    pub crossfade: f64,
}

fn default_slideshow_interval() -> f64 {
    30.0
}

impl Default for SlideshowConfig {
    fn default() -> Self {
        Self {
            interval: default_slideshow_interval(),
            shuffle: false,
            crossfade: 0.0,
        }
    }
}

/// A font asset definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontDef {
    /// Path to the font file (e.g. "assets/font.ttf").
    pub source: String,
    /// Font size in pixels for atlas rasterization.
    #[serde(default = "default_font_size")]
    pub size: f32,
}

fn default_font_size() -> f32 {
    32.0
}

/// A render buffer pass definition (multi-pass rendering).
///
/// Shadertoy-style Buffer A/B/C/D passes that render to offscreen
/// textures which can be sampled by subsequent passes and the main shader.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferDef {
    /// Fragment shader file for this buffer (relative to package root).
    /// e.g. "buffer_a.frag"
    pub shader: String,
    /// Input channels that this buffer reads from.
    /// Can reference other buffers ("BufferA", "BufferB") or regular textures.
    #[serde(default)]
    pub inputs: Vec<BufferInput>,
    /// Whether this buffer feeds back to itself (reads its own previous frame).
    #[serde(default)]
    pub feedback: bool,
}

/// An input channel for a buffer pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferInput {
    /// Channel index (0-3).
    pub channel: u32,
    /// Source: "BufferA", "BufferB", "BufferC", "BufferD", or an asset name.
    pub source: String,
    /// Texture filter for this input.
    #[serde(default)]
    pub filter: TextureFilter,
    /// Texture wrap mode for this input.
    #[serde(default)]
    pub wrap: TextureWrap,
}
