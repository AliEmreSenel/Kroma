use anyhow::{Result, anyhow};
use log::info;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::collections::HashMap;

use kroma_shared::traits::SurfaceProvider;
use kroma_shared::types::{MonitorConfig, MonitorId};

pub struct HeadlessSurfaceProvider {
    monitors: Vec<MonitorConfig>,
    virtual_surfaces: HashMap<u32, (u32, u32)>,
    disabled_names: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl HeadlessSurfaceProvider {
    pub fn new() -> Self {
        Self {
            monitors: Vec::new(),
            virtual_surfaces: HashMap::new(),
            disabled_names: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Create the default 1080p output, or an explicitly sized profiling output.
    pub fn create_virtual_monitors(&mut self) -> Result<()> {
        let (width, height) = parse_size(std::env::var("KROMA_HEADLESS_SIZE").ok().as_deref())?;
        self.monitors.clear();
        self.virtual_surfaces.clear();
        let default_monitor = MonitorConfig {
            id: MonitorId(0),
            name: "headless-virtual-0".into(),
            width,
            height,
            x: 0,
            y: 0,
            scale: 1.0,
        };

        self.virtual_surfaces.insert(
            default_monitor.id.0,
            (default_monitor.width, default_monitor.height),
        );
        self.monitors.push(default_monitor);

        info!("Created headless virtual monitor: {width}x{height}");
        Ok(())
    }
}

impl SurfaceProvider for HeadlessSurfaceProvider {
    fn connect(&mut self) -> Result<()> {
        self.create_virtual_monitors()
    }

    fn is_headless(&self) -> bool {
        true
    }

    fn size(&self, monitor: MonitorId) -> Result<(u32, u32)> {
        self.virtual_surfaces
            .get(&monitor.0)
            .copied()
            .ok_or_else(|| anyhow!("Virtual monitor {:?} does not exist", monitor))
    }

    fn display_handle(&self) -> Result<RawDisplayHandle> {
        Err(anyhow!("Offscreen targets have no display handle"))
    }

    fn create_surface(&self, _monitor: MonitorId) -> Result<RawWindowHandle> {
        Err(anyhow!("Offscreen targets have no native window"))
    }

    fn list_monitors(&self) -> Result<Vec<MonitorConfig>> {
        let disabled = self
            .disabled_names
            .lock()
            .map(|d| d.clone())
            .unwrap_or_default();
        Ok(self
            .monitors
            .iter()
            .filter(|m| !disabled.iter().any(|d| d == &m.name))
            .cloned()
            .collect())
    }

    fn apply_monitor_policy(&mut self, disabled_names: &[String]) {
        if let Ok(mut slot) = self.disabled_names.lock() {
            *slot = disabled_names.to_vec();
        }
    }

    fn dispatch(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Only the offscreen backend reads this override; native outputs use their real size.
fn parse_size(size: Option<&str>) -> Result<(u32, u32)> {
    let Some(size) = size else {
        return Ok((1920, 1080));
    };
    let (width, height) = size
        .split_once('x')
        .ok_or_else(|| anyhow!("KROMA_HEADLESS_SIZE must be WIDTHxHEIGHT"))?;
    let width: u32 = width.parse()?;
    let height: u32 = height.parse()?;
    anyhow::ensure!(
        width > 0 && height > 0,
        "Headless dimensions must be positive"
    );
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_headless_sizes_validate_and_default_to_1080p() {
        assert_eq!(parse_size(None).unwrap(), (1920, 1080));
        assert_eq!(parse_size(Some("3840x2160")).unwrap(), (3840, 2160));
        for input in ["0x2160", "1920x0", "-1x1080", "4K", "1x2x3"] {
            assert!(parse_size(Some(input)).is_err());
        }
    }
}
