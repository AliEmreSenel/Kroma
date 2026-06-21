//! Configuration file management for the daemon.
//!
//! Loads and saves daemon-level settings from `$XDG_CONFIG_HOME/kroma/config.toml`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use wgpu::PowerPreference;

/// GPU power preference.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GpuPower {
    /// Prefer low power (integrated) GPU.
    #[default]
    Low,
    /// Prefer high performance (discrete) GPU.
    High,
}

impl From<&GpuPower> for PowerPreference {
    fn from(val: &GpuPower) -> Self {
        match val {
            GpuPower::Low => PowerPreference::LowPower,
            GpuPower::High => PowerPreference::HighPerformance,
        }
    }
}

/// Daemon-level configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Target frames per second (default: 60).
    #[serde(default = "default_fps")]
    pub target_fps: u32,

    /// Path to the currently loaded .shade package (if any).
    #[serde(default)]
    pub current_shade: Option<String>,

    /// Whether to auto-pause when a fullscreen window is detected.
    #[serde(default = "default_true")]
    pub pause_on_fullscreen: bool,

    /// Whether to pause rendering on inactive workspaces.
    #[serde(default = "default_true")]
    pub pause_on_inactive: bool,

    /// GPU power preference.
    #[serde(default)]
    pub gpu_power: GpuPower,

    /// Monitor-specific overrides.
    #[serde(default)]
    pub monitors: Vec<MonitorOverride>,

    /// Preview stream defaults and limits.
    #[serde(default)]
    pub preview: PreviewConfig,

    /// Logging-related daemon settings.
    #[serde(default)]
    pub logging: LoggingConfig,

    /// Runtime behavior settings.
    #[serde(default)]
    pub runtime: RuntimeConfig,
}

/// Preview stream configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PreviewConfig {
    /// Default preview width when client does not specify one.
    #[serde(default = "default_preview_width")]
    pub default_width: u32,
    /// Default preview height when client does not specify one.
    #[serde(default = "default_preview_height")]
    pub default_height: u32,
    /// Default preview stream target FPS.
    #[serde(default = "default_preview_target_fps")]
    pub default_target_fps: u32,
    /// Hard upper-bound for preview stream FPS requests.
    #[serde(default = "default_preview_max_target_fps")]
    pub max_target_fps: u32,
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LoggingConfig {
    /// Interval, in seconds, between periodic FPS log lines.
    #[serde(default = "default_fps_log_interval_secs")]
    pub fps_log_interval_secs: u32,
    /// Emit deterministic per-frame transition trace lines for debugging.
    #[serde(default)]
    pub transition_trace: bool,
}

/// Runtime behavior configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeConfig {
    /// Persist the currently loaded shade path to config on successful load.
    #[serde(default = "default_true")]
    pub persist_current_shade: bool,
}

/// Per-monitor configuration override.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorOverride {
    /// Monitor name (e.g., "DP-1", "HDMI-A-1").
    pub name: String,
    /// Optional override .shade package for this monitor.
    pub shade: Option<String>,
    /// Whether rendering is enabled on this monitor (default: true).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_fps() -> u32 {
    60
}
fn default_true() -> bool {
    true
}
fn default_preview_width() -> u32 {
    480
}
fn default_preview_height() -> u32 {
    270
}
fn default_preview_target_fps() -> u32 {
    15
}
fn default_preview_max_target_fps() -> u32 {
    60
}
fn default_fps_log_interval_secs() -> u32 {
    5
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self {
            default_width: default_preview_width(),
            default_height: default_preview_height(),
            default_target_fps: default_preview_target_fps(),
            max_target_fps: default_preview_max_target_fps(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            fps_log_interval_secs: default_fps_log_interval_secs(),
            transition_trace: false,
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            persist_current_shade: true,
        }
    }
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            target_fps: 60,
            current_shade: None,
            pause_on_fullscreen: true,
            pause_on_inactive: true,
            gpu_power: GpuPower::Low,
            monitors: Vec::new(),
            preview: PreviewConfig::default(),
            logging: LoggingConfig::default(),
            runtime: RuntimeConfig::default(),
        }
    }
}

impl DaemonConfig {
    /// Returns the config file path.
    pub fn config_path() -> PathBuf {
        let config_dir = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            format!("{}/.config", home)
        });
        PathBuf::from(config_dir).join("kroma").join("config.toml")
    }

    /// Load the config file.
    ///
    /// If the file does not exist, writes a default config and returns it.
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        Self::load_from_path(&path)
    }

    fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            let default = Self::default();
            default.save_to_path(path)?;
            log::info!(
                "No config file found at {}, generated defaults",
                path.display()
            );
            return Ok(default);
        }

        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config: {}", path.display()))?;
        let config: Self = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config: {}", path.display()))?;

        log::info!("Loaded config from {}", path.display());
        Ok(config)
    }

    /// Save the current config to disk.
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        self.save_to_path(&path)
    }

    fn save_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        std::fs::write(path, content)
            .with_context(|| format!("Failed to write config: {}", path.display()))?;

        log::info!("Config saved to {}", path.display());
        Ok(())
    }

    /// Get the frame budget duration for the target FPS.
    /// Returns `Duration::ZERO` when `target_fps` is 0 (no frame limiting).
    pub fn frame_budget(&self) -> std::time::Duration {
        if self.target_fps == 0 {
            std::time::Duration::ZERO
        } else {
            std::time::Duration::from_micros(1_000_000 / self.target_fps as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_serializes() {
        let cfg = DaemonConfig::default();
        let s = toml::to_string_pretty(&cfg).unwrap();
        assert!(s.contains("target_fps = 60"));
        assert!(s.contains("pause_on_fullscreen = true"));
        assert!(s.contains("[preview]"));
        assert!(s.contains("[logging]"));
        assert!(s.contains("[runtime]"));
    }

    #[test]
    fn config_roundtrip() {
        let cfg = DaemonConfig {
            target_fps: 30,
            current_shade: Some("/home/user/test.shade".into()),
            preview: PreviewConfig {
                default_width: 640,
                default_height: 360,
                default_target_fps: 24,
                max_target_fps: 60,
            },
            ..Default::default()
        };
        let s = toml::to_string_pretty(&cfg).unwrap();
        let parsed: DaemonConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed.target_fps, 30);
        assert_eq!(parsed.current_shade.unwrap(), "/home/user/test.shade");
        assert_eq!(parsed.preview.default_width, 640);
        assert_eq!(parsed.preview.default_height, 360);
        assert_eq!(parsed.preview.default_target_fps, 24);
    }

    #[test]
    fn frame_budget_calculation() {
        let cfg = DaemonConfig {
            target_fps: 60,
            ..Default::default()
        };
        let budget = cfg.frame_budget();
        // 1_000_000 / 60 = 16_666 microseconds
        assert_eq!(budget.as_micros(), 16666);
    }

    #[test]
    fn load_creates_default_config_when_missing() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "kroma-daemon-config-test-{}-{}",
            std::process::id(),
            unique
        ));
        let path = dir.join("config.toml");

        let cfg = DaemonConfig::load_from_path(&path).unwrap();

        assert!(path.exists());
        assert_eq!(cfg.target_fps, 60);
        assert!(cfg.pause_on_fullscreen);
        assert!(cfg.pause_on_inactive);
        assert_eq!(cfg.preview.default_width, 480);
        assert_eq!(cfg.preview.default_height, 270);
        assert_eq!(cfg.preview.default_target_fps, 15);

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("target_fps = 60"));
        assert!(raw.contains("[preview]"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logging_transition_trace_defaults_and_roundtrips() {
        let cfg = DaemonConfig::default();
        assert!(!cfg.logging.transition_trace);

        let s = toml::to_string_pretty(&cfg).unwrap();
        assert!(s.contains("transition_trace = false"));

        let parsed: DaemonConfig = toml::from_str(&s).unwrap();
        assert!(!parsed.logging.transition_trace);
    }
}
