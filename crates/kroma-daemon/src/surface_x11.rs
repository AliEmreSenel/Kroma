//! X11 surface provider for KDE (X11 session), XFCE, MATE, Cinnamon, etc.
//!
//! Creates a fullscreen desktop-type window on each screen using XCB,
//! suitable for rendering wallpaper beneath all other windows.

use std::ptr::NonNull;

use anyhow::{Context, Result};
use log::info;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

use kroma_shared::traits::SurfaceProvider;
use kroma_shared::types::{MonitorConfig, MonitorId};

/// X11-based surface provider targeting traditional X11 desktops.
pub struct X11SurfaceProvider {
    conn: Option<RustConnection>,
    screen_num: usize,
    monitors: Vec<MonitorConfig>,
    /// Created windows keyed by monitor id.
    windows: Vec<(u32, u32)>, // (monitor_id, x11_window)
    /// Raw xcb connection pointer for wgpu.
    _xcb_connection_ptr: Option<NonNull<std::ffi::c_void>>,
}

impl X11SurfaceProvider {
    pub fn new() -> Self {
        Self {
            conn: None,
            screen_num: 0,
            monitors: Vec::new(),
            windows: Vec::new(),
            _xcb_connection_ptr: None,
        }
    }

    /// Connect to the X11 display.
    pub fn connect(&mut self) -> Result<()> {
        let (conn, screen_num) =
            RustConnection::connect(None).context("Failed to connect to X11 display")?;
        info!("Connected to X11 display (screen {})", screen_num);
        self.screen_num = screen_num;
        self.conn = Some(conn);
        Ok(())
    }

    /// Enumerate screens/monitors via Xinerama or RandR, or fall back to the
    /// root screen dimensions.
    pub fn discover_monitors(&mut self) -> Result<()> {
        let conn = self.conn.as_ref().context("Not connected to X11")?;
        let setup = conn.setup();
        let screen = &setup.roots[self.screen_num];

        // Try to use RandR to get individual monitor info
        let monitors = self.query_randr_monitors(conn, screen.root)?;

        if monitors.is_empty() {
            // Fallback: treat the whole root window as one monitor
            info!("No RandR monitors found — using root window as single screen");
            self.monitors.push(MonitorConfig {
                id: MonitorId(0),
                name: "default".into(),
                width: screen.width_in_pixels as u32,
                height: screen.height_in_pixels as u32,
                x: 0,
                y: 0,
                scale: 1.0,
            });
        } else {
            self.monitors = monitors;
        }

        Ok(())
    }

    /// Query RandR for monitor configuration.
    fn query_randr_monitors(&self, conn: &RustConnection, root: u32) -> Result<Vec<MonitorConfig>> {
        use x11rb::protocol::randr::ConnectionExt as _;

        let mut monitors = Vec::new();

        let reply = match conn.randr_get_screen_resources(root) {
            Ok(cookie) => match cookie.reply() {
                Ok(r) => r,
                Err(_) => return Ok(monitors),
            },
            Err(_) => return Ok(monitors),
        };

        for (idx, &crtc) in reply.crtcs.iter().enumerate() {
            if let Ok(cookie) = conn.randr_get_crtc_info(crtc, 0) {
                if let Ok(crtc_info) = cookie.reply() {
                    if crtc_info.width == 0 || crtc_info.height == 0 {
                        continue; // Disabled CRTC
                    }

                    let name = if let Some(&output) = crtc_info.outputs.first() {
                        conn.randr_get_output_info(output, 0)
                            .ok()
                            .and_then(|c| c.reply().ok())
                            .map(|info| String::from_utf8_lossy(&info.name).to_string())
                            .unwrap_or_else(|| format!("screen-{}", idx))
                    } else {
                        format!("screen-{}", idx)
                    };

                    monitors.push(MonitorConfig {
                        id: MonitorId(idx as u32),
                        name,
                        width: crtc_info.width as u32,
                        height: crtc_info.height as u32,
                        x: crtc_info.x as i32,
                        y: crtc_info.y as i32,
                        scale: 1.0,
                    });
                }
            }
        }

        Ok(monitors)
    }

    /// Create desktop-type windows on each monitor.
    pub fn create_all_windows(&mut self) -> Result<()> {
        let conn = self.conn.as_ref().context("Not connected to X11")?;
        let setup = conn.setup();
        let screen = &setup.roots[self.screen_num];

        // Intern required atoms
        let net_wm_window_type = conn
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE")?
            .reply()?
            .atom;
        let net_wm_window_type_desktop = conn
            .intern_atom(false, b"_NET_WM_WINDOW_TYPE_DESKTOP")?
            .reply()?
            .atom;
        let net_wm_state = conn.intern_atom(false, b"_NET_WM_STATE")?.reply()?.atom;
        let net_wm_state_below = conn
            .intern_atom(false, b"_NET_WM_STATE_BELOW")?
            .reply()?
            .atom;
        let net_wm_state_sticky = conn
            .intern_atom(false, b"_NET_WM_STATE_STICKY")?
            .reply()?
            .atom;
        let net_wm_state_skip_taskbar = conn
            .intern_atom(false, b"_NET_WM_STATE_SKIP_TASKBAR")?
            .reply()?
            .atom;
        let net_wm_state_skip_pager = conn
            .intern_atom(false, b"_NET_WM_STATE_SKIP_PAGER")?
            .reply()?
            .atom;

        for monitor in &self.monitors {
            let window_id = conn.generate_id()?;

            conn.create_window(
                COPY_DEPTH_FROM_PARENT,
                window_id,
                screen.root,
                monitor.x as i16,
                monitor.y as i16,
                monitor.width as u16,
                monitor.height as u16,
                0, // border width
                WindowClass::INPUT_OUTPUT,
                0, // visual (copy from parent)
                &CreateWindowAux::new()
                    .override_redirect(0) // Let the WM manage it
                    .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY)
                    .background_pixel(0), // black background
            )?;

            // Set window type to Desktop (renders behind all windows)
            conn.change_property32(
                PropMode::REPLACE,
                window_id,
                net_wm_window_type,
                AtomEnum::ATOM,
                &[net_wm_window_type_desktop],
            )?;

            // Set window state: below, sticky, skip taskbar/pager
            conn.change_property32(
                PropMode::REPLACE,
                window_id,
                net_wm_state,
                AtomEnum::ATOM,
                &[
                    net_wm_state_below,
                    net_wm_state_sticky,
                    net_wm_state_skip_taskbar,
                    net_wm_state_skip_pager,
                ],
            )?;

            // Set window name
            conn.change_property8(
                PropMode::REPLACE,
                window_id,
                AtomEnum::WM_NAME,
                AtomEnum::STRING,
                format!("Kroma Wallpaper ({})", monitor.name).as_bytes(),
            )?;

            // Map (show) the window
            conn.map_window(window_id)?;

            self.windows.push((monitor.id.0, window_id));
            info!(
                "Created X11 desktop window 0x{:x} for {} ({}x{} @ {}, {})",
                window_id, monitor.name, monitor.width, monitor.height, monitor.x, monitor.y
            );
        }

        conn.flush()?;

        // Wait for the first Expose/ConfigureNotify
        while let Ok(event) = conn.wait_for_event() {
            match event {
                x11rb::protocol::Event::Expose(_) | x11rb::protocol::Event::ConfigureNotify(_) => {
                    break
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Get the raw X11 display (xcb_connection_t*) pointer for wgpu.
    ///
    /// x11rb's RustConnection is implemented in pure Rust, not libxcb.
    /// We need to use a different approach — provide the Xlib display pointer
    /// by opening a parallel Xlib connection. Alternatively, use the screen
    /// number for wgpu's Xcb backend.
    ///
    /// NOTE: wgpu actually supports creating surfaces from X11 window IDs
    /// using the Xlib backend. We use x11rb for window management but
    /// convert to wgpu-compatible handles.
    pub fn get_window_id(&self, monitor_id: u32) -> Option<u32> {
        self.windows
            .iter()
            .find(|(mid, _)| *mid == monitor_id)
            .map(|(_, wid)| *wid)
    }

    pub fn screen_num(&self) -> i32 {
        self.screen_num as i32
    }

    pub fn monitors(&self) -> &[MonitorConfig] {
        &self.monitors
    }
}

impl SurfaceProvider for X11SurfaceProvider {
    fn connect(&mut self) -> Result<()> {
        self.connect()?;
        self.discover_monitors()?;
        self.create_all_windows()?;
        Ok(())
    }

    fn create_surface(&self, monitor: MonitorId) -> Result<raw_window_handle::RawWindowHandle> {
        // X11 backend uses window IDs directly via get_window_id().
        // Return an Xcb handle for wgpu compatibility.
        let wid = self
            .get_window_id(monitor.0)
            .ok_or_else(|| anyhow::anyhow!("No X11 window for monitor {}", monitor.0))?;
        let handle = raw_window_handle::XcbWindowHandle::new(
            std::num::NonZeroU32::new(wid)
                .ok_or_else(|| anyhow::anyhow!("X11 window ID is 0 for monitor {}", monitor.0))?,
        );
        Ok(raw_window_handle::RawWindowHandle::Xcb(handle))
    }

    fn list_monitors(&self) -> Result<Vec<MonitorConfig>> {
        Ok(self.monitors.clone())
    }

    fn dispatch(&mut self) -> Result<()> {
        // X11 desktop windows are passive; no event dispatch needed for
        // wallpaper rendering. Events are handled by x11rb internally.
        Ok(())
    }
}
