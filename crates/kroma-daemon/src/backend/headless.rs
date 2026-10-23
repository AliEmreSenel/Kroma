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

    pub fn create_virtual_monitors(&mut self) {
        self.monitors.clear();
        self.virtual_surfaces.clear();
        let default_monitor = MonitorConfig {
            id: MonitorId(0),
            name: "headless-virtual-0".into(),
            width: 1920,
            height: 1080,
            x: 0,
            y: 0,
            scale: 1.0,
        };

        self.virtual_surfaces.insert(
            default_monitor.id.0,
            (default_monitor.width, default_monitor.height),
        );
        self.monitors.push(default_monitor);

        info!("Created headless virtual monitor: 1920x1080");
    }
}

impl SurfaceProvider for HeadlessSurfaceProvider {
    fn connect(&mut self) -> Result<()> {
        self.create_virtual_monitors();
        Ok(())
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
