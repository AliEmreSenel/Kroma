pub mod gravitywm;
pub mod hyprland;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use glam::Vec2;
use log::{debug, info, warn};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::WaylandSurface,
    shell::wlr_layer::{
        Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
        LayerSurfaceConfigure,
    },
};
use wayland_client::{
    Connection, EventQueue, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
};

use kroma_shared::traits::SurfaceProvider;
use kroma_shared::types::{MonitorConfig, MonitorId};

/// Compositor events that affect Kroma rendering policy.
#[derive(Debug, Clone)]
pub enum WaylandEvent {
    /// The active workspace changed.
    WorkspaceChanged { id: i64 },
    /// The active monitor changed.
    MonitorChanged { name: String },
    /// The compositor event source disconnected.
    Disconnected,
}

pub enum WaylandBackend {
    Gravitywm {
        surface: WaylandSurfaceProvider,
        _cursor_handle: JoinHandle<()>,
        cursor_pos: Arc<Mutex<Vec2>>,
        cursor_height: f32,
    },
    Hyprland {
        surface: WaylandSurfaceProvider,
        _handle: JoinHandle<()>,
        _cursor_handle: JoinHandle<()>,
        cursor_pos: Arc<Mutex<Vec2>>,
        cursor_height: f32,
        rx: mpsc::Receiver<WaylandEvent>,
    },
    Generic {
        surface: WaylandSurfaceProvider,
    },
}

impl WaylandBackend {
    pub fn surface(&self) -> Option<&dyn SurfaceProvider> {
        match self {
            WaylandBackend::Gravitywm { surface, .. } => Some(surface),
            WaylandBackend::Hyprland { surface, .. } => Some(surface),
            WaylandBackend::Generic { surface, .. } => Some(surface),
        }
    }

    pub fn surface_mut(&mut self) -> Option<&mut dyn SurfaceProvider> {
        match self {
            WaylandBackend::Gravitywm { surface, .. } => Some(surface),
            WaylandBackend::Hyprland { surface, .. } => Some(surface),
            WaylandBackend::Generic { surface, .. } => Some(surface),
        }
    }

    pub fn cursor_pos(&self) -> Option<Vec2> {
        match self {
            WaylandBackend::Gravitywm {
                cursor_pos,
                cursor_height,
                ..
            }
            | WaylandBackend::Hyprland {
                cursor_pos,
                cursor_height,
                ..
            } => Some(cursor_position(cursor_pos, *cursor_height)),
            WaylandBackend::Generic { .. } => None,
        }
    }

    pub fn new(_session_type: &str, desktop_env: &str, disabled: &[String]) -> Result<Self> {
        info!("Detected Wayland session — using layer shell backend");
        let surface_provider = connected_surface_provider(disabled)?;

        let backend = match desktop_env {
            "GravityWM" | "gravitywm" => {
                let cursor_height = cursor_height(&surface_provider)?;

                match gravitywm::start_state_tracker(Arc::clone(
                    &surface_provider.global_fullscreen,
                )) {
                    Ok((cursor_pos, cursor_handle)) => WaylandBackend::Gravitywm {
                        surface: surface_provider,
                        _cursor_handle: cursor_handle,
                        cursor_pos,
                        cursor_height,
                    },
                    Err(err) => {
                        warn!("GravityWM state tracking disabled: {err}");
                        WaylandBackend::Generic {
                            surface: surface_provider,
                        }
                    }
                }
            }
            "Hyprland" => {
                let cursor_height = cursor_height(&surface_provider)?;
                let (tx, rx) = mpsc::channel();
                let handle = hyprland::start_listener(tx)?;
                let (cursor_pos, cursor_handle) =
                    hyprland::start_cursor_tracker(Arc::clone(&surface_provider.occluded_names))?;
                WaylandBackend::Hyprland {
                    surface: surface_provider,
                    _handle: handle,
                    _cursor_handle: cursor_handle,
                    cursor_pos,
                    cursor_height,
                    rx,
                }
            }
            _ => WaylandBackend::Generic {
                surface: surface_provider,
            },
        };
        Ok(backend)
    }
}

pub fn backend_events(backend: &mut WaylandBackend) -> Option<&mut mpsc::Receiver<WaylandEvent>> {
    match backend {
        WaylandBackend::Hyprland { rx, .. } => Some(rx),
        WaylandBackend::Gravitywm { .. } | WaylandBackend::Generic { .. } => None,
    }
}

fn connected_surface_provider(disabled: &[String]) -> Result<WaylandSurfaceProvider> {
    let mut surface_provider = WaylandSurfaceProvider::new();
    surface_provider.apply_monitor_policy(disabled);
    surface_provider
        .connect()
        .and_then(|_| surface_provider.create_all_surfaces())?;
    Ok(surface_provider)
}

fn cursor_height(surface_provider: &WaylandSurfaceProvider) -> Result<f32> {
    Ok(surface_provider
        .list_monitors()?
        .first()
        .map(|m| m.height as f32)
        .unwrap_or(1080.0))
}

fn cursor_position(cursor_pos: &Arc<Mutex<Vec2>>, cursor_height: f32) -> Vec2 {
    let mut pos = *cursor_pos.lock().unwrap_or_else(|e| e.into_inner());
    pos.y = cursor_height - pos.y;
    pos
}

pub(super) fn start_cursor_tracker<F>(
    name: &'static str,
    query: F,
) -> Result<(Arc<Mutex<Vec2>>, JoinHandle<()>)>
where
    F: FnMut() -> Result<Vec2> + Send + 'static,
{
    let mut query = query;
    let cursor_pos = Arc::new(Mutex::new(Vec2::new(0.5, 0.5)));
    let cursor_clone = Arc::clone(&cursor_pos);

    let handle = std::thread::Builder::new()
        .name(format!("kroma-{name}-cursor"))
        .spawn(move || {
            loop {
                match query() {
                    Ok(pos) => {
                        *cursor_clone.lock().unwrap_or_else(|e| e.into_inner()) = pos;
                    }
                    Err(err) => debug!("Could not query {name} cursor position: {err}"),
                }
                std::thread::sleep(Duration::from_millis(33));
            }
        })
        .with_context(|| format!("Failed to spawn {name} cursor tracker thread"))?;

    Ok((cursor_pos, handle))
}

// ---------------------------------------------------------------------------
// Surface types
// ---------------------------------------------------------------------------

/// A created layer shell surface with its raw pointers and metadata.
pub struct CreatedSurface {
    pub _monitor: MonitorConfig,
    pub _layer_surface: LayerSurface,
    pub wl_surface: wl_surface::WlSurface,
    /// The configured width from the compositor.
    pub width: u32,
    /// The configured height from the compositor.
    pub height: u32,
}

/// Shared, timestamped backend occlusion snapshot.
pub type OcclusionSnapshot = Arc<Mutex<(Instant, Vec<String>)>>;

/// Native surfaces and cached monitor state for Wayland.
pub struct WaylandSurfaceProvider {
    connection: Option<Connection>,
    monitors: Vec<MonitorConfig>,
    /// Created surfaces keyed by monitor id.
    surfaces: HashMap<u32, CreatedSurface>,
    retired_surfaces: Vec<CreatedSurface>,
    /// Event queue/state must be kept alive and pumped; otherwise Wayland/Vulkan
    /// presentation events accumulate while rendering.
    event_queue: Option<EventQueue<WaylandShellState>>,
    shell_state: Option<WaylandShellState>,
    /// `(snapshot time, monitor names currently covered)`; only the
    /// Hyprland tracker populates this. Stale snapshots are ignored
    /// (fail-open), and `occluded_monitors()` maps names to ids.
    pub occluded_names: OcclusionSnapshot,
    global_fullscreen: Arc<Mutex<(Instant, bool)>>,
    /// Monitor names excluded by config (`enabled = false`). The daemon
    /// keeps this in sync; disabled monitors get no surface or target.
    pub disabled_names: Arc<Mutex<Vec<String>>>,
    last_output_poll: Instant,
}

impl WaylandSurfaceProvider {
    pub fn new() -> Self {
        Self {
            connection: None,
            monitors: Vec::new(),
            surfaces: HashMap::new(),
            retired_surfaces: Vec::new(),
            event_queue: None,
            shell_state: None,
            occluded_names: Arc::new(Mutex::new((Instant::now(), Vec::new()))),
            global_fullscreen: Arc::new(Mutex::new((Instant::now(), false))),
            disabled_names: Arc::new(Mutex::new(Vec::new())),
            last_output_poll: Instant::now(),
        }
    }

    /// Get the configured dimensions for a monitor's surface.
    pub fn surface_size(&self, monitor_id: u32) -> Option<(u32, u32)> {
        let surface = self.surfaces.get(&monitor_id)?;
        let configured = self.shell_state.as_ref()?.configured_surfaces.lock().ok()?;
        let &(w, h) = configured.get(&monitor_id)?;
        Some((
            if w == 0 { surface.width } else { w },
            if h == 0 { surface.height } else { h },
        ))
    }

    /// Check if a monitor's surface has been configured by the compositor.
    #[allow(dead_code)]
    pub fn is_configured(&self, monitor_id: u32) -> bool {
        self.surface_size(monitor_id).is_some()
    }

    /// Create layer shell surfaces on all discovered monitors.
    pub fn create_all_surfaces(&mut self) -> Result<()> {
        let conn = self
            .connection
            .as_ref()
            .context("Not connected to Wayland")?;

        let (globals, mut event_queue) =
            registry_queue_init(conn).context("Failed to initialize Wayland registry")?;
        let qh: QueueHandle<WaylandShellState> = event_queue.handle();

        let compositor_state = CompositorState::bind(&globals, &qh)
            .context("Compositor not available — is a Wayland compositor running?")?;

        let layer_shell = LayerShell::bind(&globals, &qh)
            .context("wlr-layer-shell not available — is Hyprland/Sway running?")?;

        let output_state = OutputState::new(&globals, &qh);

        let mut state = WaylandShellState {
            registry_state: RegistryState::new(&globals),
            compositor_state,
            output_state,
            layer_shell,
            configured_surfaces: Arc::new(Mutex::new(HashMap::new())),
            surface_ids: Arc::new(Mutex::new(HashMap::new())),
            surfaces_closed: HashSet::new(),
        };

        // Roundtrip to discover outputs
        event_queue
            .roundtrip(&mut state)
            .context("Wayland roundtrip failed during output discovery")?;

        let disabled = self
            .disabled_names
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default();
        Self::reconcile_outputs(
            &mut self.monitors,
            &mut self.surfaces,
            &mut self.retired_surfaces,
            &disabled,
            &mut event_queue,
            &mut state,
        );
        // Initial setup may roundtrip; the runtime pump never does.
        event_queue.roundtrip(&mut state)?;
        // Keep the event queue and state alive after setup. The render loop calls
        // SurfaceProvider::dispatch() every frame; if this queue is dropped or not
        // pumped, compositor/presentation events can accumulate in the client and
        // in the Vulkan Wayland present path.
        self.event_queue = Some(event_queue);
        self.shell_state = Some(state);

        info!(
            "All {} layer shell surface(s) created and configured",
            self.surfaces.len()
        );
        Ok(())
    }

    /// Creates and commits a layer surface covering `monitor` on `output`.
    fn create_layer_surface_for(
        state: &mut WaylandShellState,
        qh: &QueueHandle<WaylandShellState>,
        monitor: &MonitorConfig,
        output: &wl_output::WlOutput,
    ) -> CreatedSurface {
        info!("Creating layer shell surface for '{}'...", monitor.name);

        let wl_surface = state.compositor_state.create_surface(qh);

        let layer_surface = state.layer_shell.create_layer_surface(
            qh,
            wl_surface.clone(),
            Layer::Background,
            Some("kroma-wallpaper"),
            Some(output),
        );

        {
            let mut surface_ids = state.surface_ids.lock().unwrap_or_else(|e| e.into_inner());
            surface_ids.insert(wl_surface.id().as_ptr() as usize, monitor.id.0);
        }

        // Configure the layer surface to cover the entire monitor
        layer_surface.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer_surface.set_exclusive_zone(-1);
        layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer_surface.set_size(0, 0);
        // An empty input region makes the wallpaper pointer-transparent.
        let region = state.compositor_state.wl_compositor().create_region(qh, ());
        wl_surface.set_input_region(Some(&region));
        region.destroy();

        // Commit to trigger configure event from compositor
        wl_surface.commit();

        CreatedSurface {
            _monitor: monitor.clone(),
            _layer_surface: layer_surface,
            wl_surface,
            width: monitor.width,
            height: monitor.height,
        }
    }

    /// Updates only changed outputs and retains retired handles until GPU teardown.
    fn reconcile_outputs(
        monitors: &mut Vec<MonitorConfig>,
        surfaces: &mut HashMap<u32, CreatedSurface>,
        retired: &mut Vec<CreatedSurface>,
        disabled: &[String],
        event_queue: &mut EventQueue<WaylandShellState>,
        state: &mut WaylandShellState,
    ) {
        let current: Vec<_> = state
            .output_state
            .outputs()
            .filter_map(|output| {
                let info = state.output_state.info(&output)?;
                let id = output.id().protocol_id();
                let name = info.name.clone().unwrap_or_else(|| format!("output-{id}"));
                if disabled.contains(&name) {
                    return None;
                }
                let (w, h) = info.modes.iter().find(|m| m.current)?.dimensions;
                let scale = info.scale_factor.max(1);
                let rotated = matches!(
                    info.transform,
                    wl_output::Transform::_90
                        | wl_output::Transform::_270
                        | wl_output::Transform::Flipped90
                        | wl_output::Transform::Flipped270
                );
                let (w, h) = if rotated { (h, w) } else { (w, h) };
                Some((
                    MonitorConfig {
                        id: MonitorId(id),
                        name,
                        width: (w / scale).max(1) as u32,
                        height: (h / scale).max(1) as u32,
                        x: info.location.0,
                        y: info.location.1,
                        scale: f64::from(scale),
                    },
                    output,
                ))
            })
            .collect();
        let removed: Vec<_> = surfaces
            .keys()
            .copied()
            .filter(|id| {
                state.surfaces_closed.contains(id) || !current.iter().any(|(m, _)| m.id.0 == *id)
            })
            .collect();
        for id in removed {
            if let Some(surface) = surfaces.remove(&id) {
                state
                    .surface_ids
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&(surface.wl_surface.id().as_ptr() as usize));
                state
                    .configured_surfaces
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                retired.push(surface);
            }
        }
        state.surfaces_closed.clear();
        let qh = event_queue.handle();
        for (monitor, output) in &current {
            let surface = surfaces
                .entry(monitor.id.0)
                .or_insert_with(|| Self::create_layer_surface_for(state, &qh, monitor, output));
            surface.width = monitor.width;
            surface.height = monitor.height;
            surface._monitor = monitor.clone();
        }
        *monitors = current.into_iter().map(|(m, _)| m).collect();
        monitors.sort_by(|a, b| a.name.cmp(&b.name));
    }
}

fn flush_pending(
    result: std::result::Result<(), wayland_client::backend::WaylandError>,
) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(wayland_client::backend::WaylandError::Io(error))
            if error.kind() == std::io::ErrorKind::WouldBlock =>
        {
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

impl SurfaceProvider for WaylandSurfaceProvider {
    fn connect(&mut self) -> Result<()> {
        info!("Connecting to Wayland display...");

        let conn = Connection::connect_to_env()
            .context("Failed to connect to Wayland display. Is a compositor running?")?;

        info!("Wayland connection established");
        self.connection = Some(conn);
        Ok(())
    }

    fn size(&self, monitor: MonitorId) -> Result<(u32, u32)> {
        self.surface_size(monitor.0)
            .context("Surface is not configured yet")
    }

    /// Get the raw display pointer (wl_display*) for wgpu.
    fn display_handle(&self) -> Result<RawDisplayHandle> {
        let conn = self
            .connection
            .as_ref()
            .context("Wayland connection not initialized")?;

        let ptr = conn.backend().display_ptr() as *mut std::ffi::c_void;
        let non_null = std::ptr::NonNull::new(ptr).context("Wayland display pointer is null")?;

        Ok(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            non_null,
        )))
    }

    fn create_surface(&self, monitor: MonitorId) -> Result<RawWindowHandle> {
        let surface = self
            .surfaces
            .get(&monitor.0)
            .context(format!("No surface created for monitor {:?}", monitor))?;

        self.size(monitor)?;
        let ptr = surface.wl_surface.id().as_ptr() as *mut std::ffi::c_void;
        let non_null = std::ptr::NonNull::new(ptr).context("Wayland surface pointer is null")?;

        Ok(raw_window_handle::RawWindowHandle::Wayland(
            raw_window_handle::WaylandWindowHandle::new(non_null),
        ))
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

    fn occluded_monitors(&self) -> Vec<MonitorId> {
        // GravityWM's global flag cannot identify the covered output when
        // multiple outputs exist. Only use it when there is exactly one output,
        // including disabled outputs in that count.
        let output_count = self
            .shell_state
            .as_ref()
            .map(|s| s.output_state.outputs().count())
            .unwrap_or(0);
        if output_count == 1
            && let Ok(snapshot) = self.global_fullscreen.lock()
            && snapshot.1
            && hyprland::is_snapshot_fresh(&snapshot.0, &Instant::now())
        {
            return self.monitors.iter().map(|m| m.id).collect();
        }
        let (snapshot_time, names) = self
            .occluded_names
            .lock()
            .map(|slot| (slot.0, slot.1.clone()))
            .unwrap_or((Instant::now(), Vec::new()));
        // Expire stale snapshots so a dead hyprctl socket cannot keep
        // occluding monitors forever.
        if !hyprland::is_snapshot_fresh(&snapshot_time, &Instant::now()) {
            return Vec::new();
        }
        map_occluded_names(&self.monitors, &names)
    }

    fn apply_monitor_policy(&mut self, disabled_names: &[String]) {
        if let Ok(mut slot) = self.disabled_names.lock()
            && *slot != disabled_names
        {
            *slot = disabled_names.to_vec();
        }
    }

    fn release_retired_surfaces(&mut self) {
        self.retired_surfaces.clear();
    }

    fn dispatch(&mut self) -> Result<()> {
        // Policy snapshot read up front so the field borrow stays outside
        // the mutable event-queue/state borrow scope below.
        let disabled: Vec<String> = self
            .disabled_names
            .lock()
            .map(|d| d.clone())
            .unwrap_or_default();

        if let (Some(event_queue), Some(state)) =
            (self.event_queue.as_mut(), self.shell_state.as_mut())
        {
            // Full non-blocking pump: send pending requests, pull in any socket
            // data that is available (the only code path that reads the socket —
            // dispatch_pending alone never does), dispatch everything, then flush
            // any requests the handlers issued (ack_configure, set_size, ...).
            flush_pending(event_queue.flush())?;

            if let Some(guard) = event_queue.prepare_read() {
                match guard.read() {
                    Ok(_) => {}
                    Err(wayland_client::backend::WaylandError::Io(error))
                        if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error.into()),
                }
            }

            while event_queue
                .dispatch_pending(state)
                .context("Failed to dispatch pending Wayland events")?
                > 0
            {}

            flush_pending(event_queue.flush())?;

            // Reconcile the surface inventory at a limited rate (hotplug,
            // disabled monitors, composer-closed surfaces).
            let now = Instant::now();
            if now.duration_since(self.last_output_poll) >= Duration::from_millis(500) {
                self.last_output_poll = now;
                WaylandSurfaceProvider::reconcile_outputs(
                    &mut self.monitors,
                    &mut self.surfaces,
                    &mut self.retired_surfaces,
                    &disabled,
                    event_queue,
                    state,
                );
            }
        }

        if let Some(ref conn) = self.connection {
            flush_pending(conn.flush())?;
        }

        Ok(())
    }
}

// -----------------------------------------------------------------------
// Internal Wayland state for SCTK event dispatch
// -----------------------------------------------------------------------

struct WaylandShellState {
    registry_state: RegistryState,
    compositor_state: CompositorState,
    output_state: OutputState,
    layer_shell: LayerShell,
    configured_surfaces: Arc<Mutex<HashMap<u32, (u32, u32)>>>,
    /// wl_surface pointer -> monitor id, so configure events can be
    /// attributed to the surface they belong to.
    surface_ids: Arc<Mutex<HashMap<usize, u32>>>,
    /// Closed layer surfaces to replace individually on the next reconciliation.
    surfaces_closed: HashSet<u32>,
}

impl CompositorHandler for WaylandShellState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        info!("Surface scale factor changed to {}", new_factor);
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for WaylandShellState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        if let Some(info) = self.output_state.info(&output) {
            info!("New output discovered: {:?}", info.name);
        }
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        warn!("An output was destroyed");
    }
}

impl LayerShellHandler for WaylandShellState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        if let Some(id) = self
            .surface_ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(layer.wl_surface().id().as_ptr() as usize))
        {
            self.surfaces_closed.insert(*id);
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (w, h) = (configure.new_size.0, configure.new_size.1);
        info!("Layer surface configured: {}x{}", w, h);

        let monitor_id = {
            let surface_ids = self.surface_ids.lock().unwrap_or_else(|e| e.into_inner());
            let ptr = layer.wl_surface().id().as_ptr() as usize;
            surface_ids.get(&ptr).copied()
        };
        // Ignore configure events for surfaces we do not manage
        let Some(monitor_id) = monitor_id else {
            return;
        };

        // Store the configure event, keyed by monitor id
        let mut configured = self
            .configured_surfaces
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if configured.get(&monitor_id).copied() != Some((w, h)) {
            configured.insert(monitor_id, (w, h));
            // SCTK acknowledges configure. Keep compositor-selected sizing.
        }
    }
}

impl ProvidesRegistryState for WaylandShellState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers!(OutputState);
}

delegate_compositor!(WaylandShellState);
delegate_output!(WaylandShellState);
delegate_layer!(WaylandShellState);
delegate_registry!(WaylandShellState);

wayland_client::delegate_noop!(WaylandShellState: ignore wayland_client::protocol::wl_region::WlRegion);

fn map_occluded_names(monitors: &[MonitorConfig], names: &[String]) -> Vec<MonitorId> {
    monitors
        .iter()
        .filter(|m| names.contains(&m.name))
        .map(|m| m.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_occlusion_names_never_pause_other_outputs() {
        let monitor = MonitorConfig {
            id: MonitorId(42),
            name: "DP-1".into(),
            width: 100,
            height: 100,
            x: 0,
            y: 0,
            scale: 1.0,
        };
        assert!(map_occluded_names(std::slice::from_ref(&monitor), &["unknown".into()]).is_empty());
        assert_eq!(
            map_occluded_names(
                &[monitor],
                &["unknown".into(), "DP-1".into(), "DP-1".into()]
            ),
            vec![MonitorId(42)]
        );
    }

    #[test]
    fn socket_backpressure_is_not_a_dead_display() {
        assert!(
            flush_pending(Err(wayland_client::backend::WaylandError::Io(
                std::io::ErrorKind::WouldBlock.into()
            )))
            .is_ok()
        );
        assert!(
            flush_pending(Err(wayland_client::backend::WaylandError::Io(
                std::io::ErrorKind::BrokenPipe.into()
            )))
            .is_err()
        );
    }
}
