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

    pub fn new(session_type: &str, desktop_env: &str, disabled: &[String]) -> Result<Self> {
        info!("Session: type={}, desktop={}", session_type, desktop_env);

        let is_wayland = session_type == "wayland" || std::env::var("WAYLAND_DISPLAY").is_ok();
        let is_x11 = session_type == "x11" || std::env::var("DISPLAY").is_ok();

        // Prefer the declared session type; for hybrid environments (both
        // WAYLAND_DISPLAY and DISPLAY set) prefer Wayland.
        let prefer_wayland = match session_type {
            "wayland" => true,
            "x11" => false,
            _ => is_wayland,
        };

        // XWayland's DISPLAY does not turn a KDE Wayland session into X11.
        if desktop_env == "KDE" && prefer_wayland {
            return Self::headless(disabled);
        }

        if prefer_wayland && is_wayland {
            match WaylandBackend::new(session_type, desktop_env, disabled) {
                Ok(backend) => return Ok(Backend::Wayland { backend }),
                Err(error) => warn!("Wayland initialization failed: {error:#}; trying X11"),
            }
        }
        if is_x11 {
            return Self::x11(disabled);
        }

        Err(anyhow!(
            "No display backend available (no Wayland or X11 session found)"
        ))
    }

    fn x11(disabled: &[String]) -> Result<Self> {
        info!("Detected X11 session — using desktop window backend");
        let mut x11_provider = X11SurfaceProvider::new();
        x11_provider.apply_monitor_policy(disabled);
        x11_provider
            .connect()
            .and_then(|_| x11_provider.discover_monitors())
            .and_then(|_| x11_provider.create_all_windows())?;
        Ok(Backend::X11 {
            surface: x11_provider,
        })
    }

    fn headless(disabled: &[String]) -> Result<Self> {
        let mut provider = HeadlessSurfaceProvider::new();
        provider.apply_monitor_policy(disabled);
        provider.connect()?;
        Ok(Backend::Headless { surface: provider })
    }
}
