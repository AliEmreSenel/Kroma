pub mod wayland;
pub mod x11;

use crate::backend::wayland::{WaylandBackend, WaylandSurfaceProvider, hyprland};
use crate::backend::x11::X11SurfaceProvider;
use std::sync::mpsc;

use anyhow::{Result, anyhow};
use kroma_shared::traits::SurfaceProvider;
use log::info;

pub enum Backend {
    Wayland {
        surface: WaylandSurfaceProvider,
        backend: WaylandBackend,
    },
    X11 {
        surface: X11SurfaceProvider,
    },
}

impl Backend {
    // Helper to get a dynamic reference to the surface
    pub fn surface(&self) -> Option<&dyn SurfaceProvider> {
        match self {
            Backend::Wayland { surface, .. } => Some(surface),
            Backend::X11 { surface, .. } => Some(surface),
        }
    }

    pub fn new(session_type: &str, desktop_env: &str) -> Result<Self> {
        info!("Session: type={}, desktop={}", session_type, desktop_env);

        if session_type == "wayland" || std::env::var("WAYLAND_DISPLAY").is_ok() {
            info!("Detected Wayland session — using layer shell backend");
            let mut surface_provider = WaylandSurfaceProvider::new();
            match surface_provider
                .connect()
                .and_then(|_| surface_provider.create_all_surfaces())
            {
                Ok(()) => {
                    // ---------------------------------------------------------------
                    // 3b. Start Hyprland event listener (optional)
                    // ---------------------------------------------------------------
                    let (hypr_tx, hypr_rx) = mpsc::channel();
                    let backend = match hyprland::start_listener(hypr_tx) {
                        Ok(h) => {
                            info!("Hyprland event listener started");
                            WaylandBackend::Hyprland {
                                _handle: h,
                                rx: hypr_rx,
                            }
                        }
                        Err(e) => {
                            log::warn!(
                                "Hyprland events unavailable: {} — running without compositor awareness",
                                e
                            );
                            WaylandBackend::Generic
                        }
                    };

                    Ok(Backend::Wayland {
                        surface: surface_provider,
                        backend,
                    })
                }
                Err(e) => {
                    log::warn!(
                        "Wayland surface creation failed: {} - trying X11 fallback",
                        e
                    );
                    Backend::new("x11", desktop_env)
                }
            }
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
