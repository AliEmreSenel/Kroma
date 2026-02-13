//! Configuration file management for the daemon.
//!
//! Loads and saves daemon-level settings from `$XDG_CONFIG_HOME/kroma/config.toml`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            target_fps: 60,
            current_shade: None,
            pause_on_fullscreen: true,
            pause_on_inactive: true,
            gpu_power: GpuPower::Low,
            monitors: Vec::new(),
        }
    }
}

impl DaemonConfig {
    /// Returns the config file path.
    pub fn config_path() -> PathBuf {
        let config_dir = std::env::var("XDG_CONFIG_HOME")
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                format!("{}/.config", home)
            });
        PathBuf::from(config_dir).join("kroma").join("config.toml")
    }

    /// Load the config file, or return defaults if it doesn't exist.
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if !path.exists() {
            log::info!("No config file found at {}, using defaults", path.display());
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config: {}", path.display()))?;
        let config: Self = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config: {}", path.display()))?;

        log::info!("Loaded config from {}", path.display());
        Ok(config)
    }

    /// Save the current config to disk.
    #[allow(dead_code)]
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)
            .context("Failed to serialize config")?;
        std::fs::write(&path, content)
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
    }

    #[test]
    fn config_roundtrip() {
        let cfg = DaemonConfig {
            target_fps: 30,
            current_shade: Some("/home/user/test.shade".into()),
            ..Default::default()
        };
        let s = toml::to_string_pretty(&cfg).unwrap();
        let parsed: DaemonConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed.target_fps, 30);
        assert_eq!(parsed.current_shade.unwrap(), "/home/user/test.shade");
    }

    #[test]
    fn frame_budget_calculation() {
        let cfg = DaemonConfig { target_fps: 60, ..Default::default() };
        let budget = cfg.frame_budget();
        // 1_000_000 / 60 = 16_666 microseconds
        assert_eq!(budget.as_micros(), 16666);
    }
}
