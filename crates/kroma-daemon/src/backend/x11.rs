//! X11 surface provider for KDE (X11 session), XFCE, MATE, Cinnamon, etc.
//!
//! Creates a fullscreen desktop-type window on each screen using XCB,
//! suitable for rendering wallpaper beneath all other windows.

use std::ptr::NonNull;

use anyhow::{Context, Result};
use log::{debug, info};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle};
use x11rb::COPY_DEPTH_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape::ConnectionExt as _;
use x11rb::protocol::xproto::*;
use x11rb::protocol::{Event, randr, shape};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection; // Using XCBConnection to get a raw libxcb pointer

use kroma_shared::traits::SurfaceProvider;
use kroma_shared::types::{MonitorConfig, MonitorId};

const OCCLUSION_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// X11-based surface provider targeting traditional X11 desktops.
pub struct X11SurfaceProvider {
    conn: Option<XCBConnection>,
    screen_num: usize,
    randr_available: bool,
    monitors: Vec<MonitorConfig>,
    /// Monitor names excluded by config (`enabled = false`); disabled
    /// monitors get no window and are hidden from list_monitors().
    disabled_names: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// Created windows, keyed by monitor name (stable across CRTC renumbering).
    windows: Vec<(String, u32, u32)>, // (name, monitor_id, x11_window)
    /// Monitors covered by fullscreen windows (EWMH); fail-open when unknown.
    occluded: std::sync::Mutex<Vec<MonitorId>>,
    last_occlusion_poll: std::time::Instant,
    retired_windows: Vec<u32>,
    atoms: std::collections::HashMap<&'static [u8], u32>,
}

impl Default for X11SurfaceProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl X11SurfaceProvider {
    pub fn new() -> Self {
        Self {
            conn: None,
            screen_num: 0,
            randr_available: false,
            monitors: Vec::new(),
            windows: Vec::new(),
            disabled_names: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            occluded: std::sync::Mutex::new(Vec::new()),
            last_occlusion_poll: std::time::Instant::now(),
            retired_windows: Vec::new(),
            atoms: std::collections::HashMap::new(),
        }
    }

    /// Connect to the X11 display.
    pub fn connect(&mut self) -> Result<()> {
        let (conn, screen_num) =
            XCBConnection::connect(None).context("Failed to connect to X11 display")?;
        info!("Connected to X11 display (screen {})", screen_num);
        self.screen_num = screen_num;
        let root = conn.setup().roots[screen_num].root;
        self.randr_available = conn
            .randr_query_version(1, 3)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some();
        let _ = conn.randr_select_input(
            root,
            randr::NotifyMask::SCREEN_CHANGE
                | randr::NotifyMask::CRTC_CHANGE
                | randr::NotifyMask::OUTPUT_CHANGE,
        );
        for name in [
            b"_NET_CLIENT_LIST".as_slice(),
            b"_NET_WM_STATE",
            b"_NET_WM_STATE_FULLSCREEN",
            b"_NET_WM_STATE_HIDDEN",
            b"_NET_WM_DESKTOP",
            b"_NET_CURRENT_DESKTOP",
            b"_NET_SHOWING_DESKTOP",
        ] {
            // Creating an atom does not imply support: absent properties still fail open.
            if let Some(atom) = intern_atom(&conn, name) {
                self.atoms.insert(name, atom);
            }
        }
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

        if monitors.is_empty() && !self.randr_available {
            // Fallback: treat the whole root window as one monitor
            info!("No RandR monitors found — using root window as single screen");
            self.monitors = vec![MonitorConfig {
                id: MonitorId(0),
                name: "default".into(),
                width: screen.width_in_pixels as u32,
                height: screen.height_in_pixels as u32,
                x: 0,
                y: 0,
                scale: 1.0,
            }];
        } else {
            self.monitors = monitors;
        }

        Ok(())
    }

    /// Query RandR for monitor configuration.
    fn query_randr_monitors(&self, conn: &XCBConnection, root: u32) -> Result<Vec<MonitorConfig>> {
        use x11rb::protocol::randr::ConnectionExt as _;

        let mut monitors = Vec::new();

        if !self.randr_available {
            let geometry = conn.get_geometry(root)?.reply()?;
            return Ok(vec![MonitorConfig {
                id: MonitorId(0),
                name: "default".into(),
                width: u32::from(geometry.width),
                height: u32::from(geometry.height),
                x: 0,
                y: 0,
                scale: 1.0,
            }]);
        }
        let reply = conn.randr_get_screen_resources(root)?.reply()?;

        for (idx, &crtc) in reply.crtcs.iter().enumerate() {
            if let Ok(cookie) = conn.randr_get_crtc_info(crtc, 0)
                && let Ok(crtc_info) = cookie.reply()
            {
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
                    id: MonitorId(crtc_info.outputs.first().copied().unwrap_or(crtc)),
                    name,
                    width: crtc_info.width as u32,
                    height: crtc_info.height as u32,
                    x: crtc_info.x as i32,
                    y: crtc_info.y as i32,
                    scale: 1.0,
                });
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

        for monitor in &self.active_monitors() {
            if self.windows.iter().any(|(_, id, _)| *id == monitor.id.0) {
                continue;
            }
            let window_id = conn.generate_id()?;

            let created = (|| -> Result<()> {
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

                // The wallpaper must never intercept pointer input.
                conn.shape_rectangles(
                    shape::SO::SET,
                    shape::SK::INPUT,
                    ClipOrdering::UNSORTED,
                    window_id,
                    0,
                    0,
                    &[],
                )?
                .check()?;
                conn.map_window(window_id)?;

                Ok(())
            })();
            if let Err(error) = created {
                let _ = conn.destroy_window(window_id);
                log::warn!("Cannot create X11 window for {}: {error:#}", monitor.name);
                continue;
            }

            self.windows
                .push((monitor.name.clone(), monitor.id.0, window_id));
            info!(
                "Created X11 desktop window 0x{:x} for {} ({}x{} @ {}, {})",
                window_id, monitor.name, monitor.width, monitor.height, monitor.x, monitor.y
            );
        }

        conn.flush()?;

        Ok(())
    }

    pub fn get_window_id(&self, monitor_id: u32) -> Option<u32> {
        self.windows
            .iter()
            .find(|(_, mid, _)| *mid == monitor_id)
            .map(|(_, _, wid)| *wid)
    }

    /// Refreshes outputs without replacing windows belonging to unchanged monitors.
    fn reconcile_layout(&mut self) {
        let Some(conn) = self.conn.as_ref() else {
            return;
        };
        let root = conn.setup().roots[self.screen_num].root;
        let Ok(monitors) = self.query_randr_monitors(conn, root) else {
            return;
        };
        self.monitors = monitors;
        let active = self.active_monitors();
        self.windows.retain(|(_, id, window)| {
            let keep = active.iter().any(|m| m.id.0 == *id);
            if !keep {
                self.retired_windows.push(*window);
            }
            keep
        });
        for (_, id, window) in &self.windows {
            if let Some(monitor) = active.iter().find(|m| m.id.0 == *id) {
                let _ = conn.configure_window(
                    *window,
                    &ConfigureWindowAux::new()
                        .x(monitor.x)
                        .y(monitor.y)
                        .width(monitor.width)
                        .height(monitor.height),
                );
            }
        }
        if let Err(error) = self.create_all_windows() {
            log::warn!("Failed to create X11 monitor window: {error}");
        }
    }

    /// The current monitor records minus config-disabled monitors.
    pub fn active_monitors(&self) -> Vec<MonitorConfig> {
        let disabled = self
            .disabled_names
            .lock()
            .map(|d| d.clone())
            .unwrap_or_default();
        self.monitors
            .iter()
            .filter(|m| !disabled.iter().any(|d| d == &m.name))
            .cloned()
            .collect()
    }

    /// Refreshes the occluded-monitor set from EWMH.
    ///
    /// A monitor is occluded when a `_NET_WM_STATE_FULLSCREEN` client window
    /// fully covers the monitor rectangle. Fails open (clears the set) on any
    /// error so an uncooperative desktop never blocks rendering.
    fn refresh_occlusion(&mut self) {
        let Some(conn) = self.conn.as_ref() else {
            return;
        };
        let root = conn.setup().roots[self.screen_num].root;
        let property = |window, name: &'static [u8], kind: AtomEnum| {
            property_values(conn, window, *self.atoms.get(name)?, kind)
        };
        let Some(fullscreen_atom) = self.atoms.get(b"_NET_WM_STATE_FULLSCREEN".as_slice()) else {
            self.set_occluded(Vec::new());
            return;
        };
        let Some(current_desktop) = property(root, b"_NET_CURRENT_DESKTOP", AtomEnum::CARDINAL)
            .and_then(|v| v.first().copied())
        else {
            self.set_occluded(Vec::new());
            return;
        };
        if property(root, b"_NET_SHOWING_DESKTOP", AtomEnum::CARDINAL)
            .is_some_and(|v| v.first() == Some(&1))
        {
            self.set_occluded(Vec::new());
            return;
        }
        let clients = property(root, b"_NET_CLIENT_LIST", AtomEnum::WINDOW).unwrap_or_default();
        let mut occluded = Vec::new();
        for window in clients {
            if self.windows.iter().any(|(_, _, id)| *id == window) {
                continue;
            }
            let Some(states) = property(window, b"_NET_WM_STATE", AtomEnum::ATOM) else {
                continue;
            };
            if !states.contains(fullscreen_atom)
                || self
                    .atoms
                    .get(b"_NET_WM_STATE_HIDDEN".as_slice())
                    .is_some_and(|a| states.contains(a))
            {
                continue;
            }
            let desktop = property(window, b"_NET_WM_DESKTOP", AtomEnum::CARDINAL)
                .and_then(|v| v.first().copied());
            if !desktop_visible(desktop, current_desktop) {
                continue;
            }
            let Some(attributes) = conn
                .get_window_attributes(window)
                .ok()
                .and_then(|c| c.reply().ok())
            else {
                continue;
            };
            if attributes.map_state != MapState::VIEWABLE {
                continue;
            }
            let Some(geom) = conn.get_geometry(window).ok().and_then(|c| c.reply().ok()) else {
                continue;
            };
            let Some(origin) = conn
                .translate_coordinates(window, root, 0, 0)
                .ok()
                .and_then(|c| c.reply().ok())
            else {
                continue;
            };
            if !origin.same_screen {
                continue;
            }
            for monitor in &self.monitors {
                if window_occupies(
                    monitor,
                    i32::from(origin.dst_x),
                    i32::from(origin.dst_y),
                    u32::from(geom.width),
                    u32::from(geom.height),
                ) && !occluded.contains(&monitor.id)
                {
                    occluded.push(monitor.id);
                }
            }
        }
        self.set_occluded(occluded);
    }

    fn set_occluded(&self, list: Vec<MonitorId>) {
        if let Ok(mut slot) = self.occluded.lock() {
            if *slot != list {
                debug!("X11 occluded monitors: {list:?}");
            }
            *slot = list;
        }
    }
}

/// True when the window rect (root coordinates) fully covers the monitor.
fn window_occupies(monitor: &MonitorConfig, wx: i32, wy: i32, ww: u32, wh: u32) -> bool {
    ww > 0
        && wh > 0
        && monitor.width > 0
        && monitor.height > 0
        && wx <= monitor.x
        && wy <= monitor.y
        && i64::from(wx) + i64::from(ww) >= i64::from(monitor.x) + i64::from(monitor.width)
        && i64::from(wy) + i64::from(wh) >= i64::from(monitor.y) + i64::from(monitor.height)
}

fn desktop_visible(desktop: Option<u32>, current: u32) -> bool {
    desktop.is_some_and(|d| d == current || d == u32::MAX)
}

/// Interns an atom name; None when the request fails.
fn intern_atom(conn: &XCBConnection, name: &[u8]) -> Option<u32> {
    Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
}

/// Reads a typed 32-bit property; unavailable or mismatched properties are unknown.
fn property_values(
    conn: &XCBConnection,
    window: u32,
    property: u32,
    kind: AtomEnum,
) -> Option<Vec<u32>> {
    let reply = conn
        .get_property(false, window, property, kind, 0, 65536)
        .ok()?
        .reply()
        .ok()?;
    if reply.type_ != u32::from(kind) || reply.bytes_after != 0 {
        return None;
    }
    Some(reply.value32()?.collect())
}

impl SurfaceProvider for X11SurfaceProvider {
    fn connect(&mut self) -> Result<()> {
        X11SurfaceProvider::connect(self)?;
        self.discover_monitors()?;
        self.create_all_windows()?;
        Ok(())
    }

    fn size(&self, monitor: MonitorId) -> Result<(u32, u32)> {
        let mon = self
            .monitors
            .iter()
            .find(|m| m.id == monitor)
            .context("Monitor not found")?;
        Ok((mon.width, mon.height))
    }

    fn display_handle(&self) -> Result<RawDisplayHandle> {
        let conn = self.conn.as_ref().context("Not connected to X11")?;

        // Fetch the raw libxcb pointer securely managed by the crate
        let raw_conn_ptr = conn.get_raw_xcb_connection();
        let non_null_conn =
            NonNull::new(raw_conn_ptr as *mut _).context("Null XCB connection pointer")?;

        let handle = XcbDisplayHandle::new(Some(non_null_conn), self.screen_num as i32);
        Ok(RawDisplayHandle::Xcb(handle))
    }

    fn create_surface(&self, monitor: MonitorId) -> Result<RawWindowHandle> {
        let wid = self
            .get_window_id(monitor.0)
            .context(format!("No X11 window for monitor {}", monitor.0))?;

        // raw-window-handle expects a NonZeroU32 for XCB window IDs
        let window_id = std::num::NonZeroU32::new(wid).context("Window ID must be non-zero")?;
        let handle = XcbWindowHandle::new(window_id);
        Ok(RawWindowHandle::Xcb(handle))
    }

    fn list_monitors(&self) -> Result<Vec<MonitorConfig>> {
        Ok(self.active_monitors())
    }

    fn occluded_monitors(&self) -> Vec<MonitorId> {
        self.occluded.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn apply_monitor_policy(&mut self, disabled_names: &[String]) {
        if let Ok(mut slot) = self.disabled_names.lock()
            && *slot != disabled_names
        {
            *slot = disabled_names.to_vec();
        }
    }

    fn release_retired_surfaces(&mut self) {
        if let Some(conn) = &self.conn {
            for window in self.retired_windows.drain(..) {
                let _ = conn.destroy_window(window);
            }
            let _ = conn.flush();
        }
    }

    fn dispatch(&mut self) -> Result<()> {
        let conn = self.conn.as_ref().context("Not connected to X11")?;

        // Pump events non-blockingly to keep the X11 connection healthy
        let mut layout_changed = false;
        while let Some(event) = conn.poll_for_event()? {
            layout_changed |= matches!(
                event,
                Event::RandrNotify(_) | Event::RandrScreenChangeNotify(_)
            );
        }
        if layout_changed {
            self.reconcile_layout();
        }

        // Refresh the occluded-monitor set at a limited rate (EWMH).
        // Fail-open is handled inside refresh_occlusion.
        let now = std::time::Instant::now();
        if now >= self.last_occlusion_poll + OCCLUSION_POLL_INTERVAL {
            self.last_occlusion_poll = now;
            self.refresh_occlusion();
            self.reconcile_layout();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn inactive_and_unknown_desktops_never_occlude() {
        assert!(super::desktop_visible(Some(3), 3));
        assert!(super::desktop_visible(Some(u32::MAX), 3));
        assert!(!super::desktop_visible(Some(4), 3));
        assert!(!super::desktop_visible(None, 3));
    }

    #[test]
    fn spanning_window_covers_both_monitors() {
        let left = monitor("left", -1920, 0, 1920, 1080);
        let right = monitor("right", 0, 0, 1920, 1080);
        assert!(super::window_occupies(&left, -1920, 0, 3840, 1080));
        assert!(super::window_occupies(&right, -1920, 0, 3840, 1080));
        assert!(!super::window_occupies(&right, 0, 0, 0, 0));
    }

    use super::*;

    fn monitor(name: &str, x: i32, y: i32, w: u32, h: u32) -> MonitorConfig {
        MonitorConfig {
            id: MonitorId(0),
            name: name.into(),
            width: w,
            height: h,
            x,
            y,
            scale: 1.0,
        }
    }

    #[test]
    fn fullscreen_window_covering_monitor_occludes_it() {
        let m = monitor("DP-1", 0, 0, 1920, 1080);
        assert!(window_occupies(&m, 0, 0, 1920, 1080));
    }

    #[test]
    fn larger_window_offscreen_origin_still_covers() {
        let m = monitor("DP-1", 0, 0, 1920, 1080);
        assert!(window_occupies(&m, -50, -50, 2000, 1200));
    }

    #[test]
    fn partial_window_does_not_occlude() {
        let m = monitor("DP-1", 0, 0, 1920, 1080);
        assert!(!window_occupies(&m, 0, 0, 960, 1080));
        assert!(!window_occupies(&m, 960, 0, 960, 1080));
        assert!(!window_occupies(&m, 0, 0, 1920, 540));
    }

    #[test]
    fn window_on_neighbouring_monitor_does_not_occlude() {
        let m = monitor("DP-2", 1920, 0, 1920, 1080);
        assert!(!window_occupies(&m, 0, 0, 1920, 1080));
        assert!(window_occupies(&m, 1920, 0, 1920, 1080));
    }
}
