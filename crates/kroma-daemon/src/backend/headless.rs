use std::collections::HashMap;
use std::fs::File;
use std::os::unix::io::AsRawFd;

use anyhow::{Context, Result, anyhow};
use log::info;
use raw_window_handle::{DrmDisplayHandle, DrmWindowHandle, RawDisplayHandle, RawWindowHandle};

use kroma_shared::traits::SurfaceProvider;
use kroma_shared::types::{MonitorConfig, MonitorId};

pub struct HeadlessSurfaceProvider {
    drm_node: Option<File>,
    monitors: Vec<MonitorConfig>,
    virtual_surfaces: HashMap<u32, (u32, u32)>,
}

impl HeadlessSurfaceProvider {
    pub fn new() -> Self {
        Self {
            drm_node: None,
            monitors: Vec::new(),
            virtual_surfaces: HashMap::new(),
        }
    }

    pub fn create_virtual_monitors(&mut self) {
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
        info!("Connecting to DRM render node for Headless DMA-BUF capability...");

        let file = File::open("/dev/dri/renderD128")
            .context("Failed to open /dev/dri/renderD128. Are graphics drivers installed?")?;

        self.drm_node = Some(file);
        self.create_virtual_monitors();

        info!("Successfully opened DRM render node");
        Ok(())
    }

    fn size(&self, monitor: MonitorId) -> Result<(u32, u32)> {
        self.virtual_surfaces
            .get(&monitor.0)
            .copied()
            .ok_or_else(|| anyhow!("Virtual monitor {:?} does not exist", monitor))
    }

    fn display_handle(&self) -> Result<RawDisplayHandle> {
        let file = self
            .drm_node
            .as_ref()
            .context("DRM node not initialized. Did you call connect()?")?;

        let fd = file.as_raw_fd();

        let handle = DrmDisplayHandle::new(fd);

        Ok(RawDisplayHandle::Drm(handle))
    }

    fn create_surface(&self, monitor: MonitorId) -> Result<RawWindowHandle> {
        if !self.virtual_surfaces.contains_key(&monitor.0) {
            return Err(anyhow!("No virtual surface configured for {:?}", monitor));
        }

        info!("Providing headless generic window handle for {:?}", monitor);

        let handle = DrmWindowHandle::new(0);
        Ok(RawWindowHandle::Drm(handle))
    }

    fn list_monitors(&self) -> Result<Vec<MonitorConfig>> {
        Ok(self.monitors.clone())
    }

    fn dispatch(&mut self) -> Result<()> {
        Ok(())
    }
}
