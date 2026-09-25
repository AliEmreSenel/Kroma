pub mod headless;
pub mod wayland;
pub mod x11;

use crate::backend::x11::X11SurfaceProvider;
use crate::backend::{headless::HeadlessSurfaceProvider, wayland::WaylandBackend};

use anyhow::{Result, anyhow};
use glam::Vec2;
use kroma_shared::traits::SurfaceProvider;
use log::{info, warn};

#[allow(clippy::large_enum_variant)]
pub enum Backend {
    Wayland { backend: WaylandBackend },
    X11 { surface: X11SurfaceProvider },
    Headless { surface: HeadlessSurfaceProvider },
}

impl Backend {
    // Helper to get a dynamic reference to the surface
    pub fn surface(&self) -> Option<&dyn SurfaceProvider> {
        match self {
            Backend::Wayland { backend, .. } => backend.surface(),
            Backend::X11 { surface, .. } => Some(surface),
            Backend::Headless { surface, .. } => Some(surface),
        }
    }

    pub fn surface_mut(&mut self) -> Option<&mut dyn SurfaceProvider> {
        match self {
            Backend::Wayland { backend, .. } => backend.surface_mut(),
            Backend::X11 { surface, .. } => Some(surface),
            Backend::Headless { surface, .. } => Some(surface),
        }
    }

    pub fn cursor_pos(&self) -> Option<Vec2> {
        match self {
            Backend::Wayland { backend, .. } => backend.cursor_pos(),
            Backend::X11 { .. } => None,
            Backend::Headless { .. } => None,
        }
    }

    pub fn new(session_type: &str, desktop_env: &str) -> Result<Self> {
        info!("Session: type={}, desktop={}", session_type, desktop_env);

        if desktop_env == "KDE" {
            let mut headless_provider = HeadlessSurfaceProvider::new();
            headless_provider
                .connect()
                .map(|_| headless_provider.create_virtual_monitors())?;
            Ok(Backend::Headless {
                surface: headless_provider,
            })
        } else if session_type == "wayland" || std::env::var("WAYLAND_DISPLAY").is_ok() {
            WaylandBackend::new(session_type, desktop_env)
                .map(|backend| Backend::Wayland { backend })
                .or_else(|e| {
                    warn!(
                        "Failed to initialize wayland backend: {}. Falling back to X11",
                        e
                    );
                    Backend::new("x11", desktop_env)
                })
        } else if session_type == "x11" || std::env::var("DISPLAY").is_ok() {
            info!("Detected X11 session — using desktop window backend");
            let mut x11_provider = X11SurfaceProvider::new();
            x11_provider
                .connect()
                .and_then(|_| x11_provider.discover_monitors())
                .and_then(|_| x11_provider.create_all_windows())?;
            Ok(Backend::X11 {
                surface: x11_provider,
            })
        } else {
            Err(anyhow!("No backend found"))
        }
    }
}
