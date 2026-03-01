pub mod hyprland;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use glam::Vec2;
use log::{info, warn};
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
    Connection, Proxy, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
};

use kroma_shared::traits::SurfaceProvider;
use kroma_shared::types::{MonitorConfig, MonitorId};

use crate::backend::wayland::hyprland::HyprlandEvent;

pub enum WaylandBackend {
    Hyprland {
        surface: WaylandSurfaceProvider,
        _handle: JoinHandle<()>,
        _cursor_handle: JoinHandle<()>,
        cursor_pos: Arc<Mutex<Vec2>>,
        cursor_height: f32,
        rx: mpsc::Receiver<HyprlandEvent>,
    },
    Generic {
        surface: WaylandSurfaceProvider,
    },
}

impl WaylandBackend {
    pub fn surface(&self) -> Option<&dyn SurfaceProvider> {
        match self {
            WaylandBackend::Hyprland { surface, .. } => Some(surface),
            WaylandBackend::Generic { surface, .. } => Some(surface),
        }
    }

    pub fn surface_mut(&mut self) -> Option<&mut dyn SurfaceProvider> {
        match self {
            WaylandBackend::Hyprland { surface, .. } => Some(surface),
            WaylandBackend::Generic { surface, .. } => Some(surface),
        }
    }

    pub fn cursor_pos(&self) -> Option<Vec2> {
        match self {
            WaylandBackend::Hyprland {
                cursor_pos,
                cursor_height,
                ..
            } => {
                let mut pos = *cursor_pos.lock().unwrap_or_else(|e| e.into_inner());
                pos.y = *cursor_height - pos.y;
                Some(pos)
            }
            WaylandBackend::Generic { .. } => None,
        }
    }

    pub fn new(_session_type: &str, desktop_env: &str) -> Result<Self> {
        info!("Detected Wayland session — using layer shell backend");
        let backend = match desktop_env {
            "Hyprland" => {
                let mut surface_provider = WaylandSurfaceProvider::new();
                surface_provider
                    .connect()
                    .and_then(|_| surface_provider.create_all_surfaces())?;
                let cursor_height = surface_provider
                    .list_monitors()?
                    .first()
                    .map(|m| m.height as f32)
                    .unwrap_or(1080.0);

                let (hypr_tx, hypr_rx) = mpsc::channel();
                let handle = hyprland::start_listener(hypr_tx)?;
                let (cursor_pos, cursor_handle) = hyprland::start_cursor_tracker()?;
                WaylandBackend::Hyprland {
                    surface: surface_provider,
                    _handle: handle,
                    _cursor_handle: cursor_handle,
                    cursor_pos,
                    cursor_height,
                    rx: hypr_rx,
                }
            }
            _ => {
                let mut surface_provider = WaylandSurfaceProvider::new();
                surface_provider
                    .connect()
                    .and_then(|_| surface_provider.create_all_surfaces())?;
                WaylandBackend::Generic {
                    surface: surface_provider,
                }
            }
        };
        Ok(backend)
    }
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
    /// Whether the compositor has sent a configure event.
    pub configured: bool,
}

/// Wayland-based surface provider targeting Hyprland's Layer Shell.
pub struct WaylandSurfaceProvider {
    connection: Option<Connection>,
    monitors: Vec<MonitorConfig>,
    /// Created surfaces keyed by monitor id.
    surfaces: HashMap<u32, CreatedSurface>,
}

impl WaylandSurfaceProvider {
    pub fn new() -> Self {
        Self {
            connection: None,
            monitors: Vec::new(),
            surfaces: HashMap::new(),
        }
    }

    /// Get the configured dimensions for a monitor's surface.
    pub fn surface_size(&self, monitor_id: u32) -> Option<(u32, u32)> {
        self.surfaces.get(&monitor_id).map(|s| (s.width, s.height))
    }

    /// Check if a monitor's surface has been configured by the compositor.
    #[allow(dead_code)]
    pub fn is_configured(&self, monitor_id: u32) -> bool {
        self.surfaces
            .get(&monitor_id)
            .map(|s| s.configured)
            .unwrap_or(false)
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
        };

        // Roundtrip to discover outputs
        event_queue
            .roundtrip(&mut state)
            .context("Wayland roundtrip failed during output discovery")?;

        // Enumerate monitors from output state
        let outputs: Vec<_> = state.output_state.outputs().collect();
        let mut discovered_monitors = Vec::new();

        for (idx, output) in outputs.iter().enumerate() {
            if let Some(info) = state.output_state.info(output) {
                let (width, height) = info
                    .modes
                    .iter()
                    .find(|m| m.current)
                    .map(|m| (m.dimensions.0 as u32, m.dimensions.1 as u32))
                    .unwrap_or((1920, 1080));

                let (x, y) = info.location;

                let monitor = MonitorConfig {
                    id: MonitorId(idx as u32),
                    name: info
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("output-{}", idx)),
                    width,
                    height,
                    x,
                    y,
                    scale: info.scale_factor as f64,
                };

                info!(
                    "Discovered output: {} ({}x{} @ {},{} scale {})",
                    monitor.name, width, height, x, y, monitor.scale
                );
                discovered_monitors.push((monitor, output.clone()));
            }
        }

        if discovered_monitors.is_empty() {
            warn!("No outputs discovered — creating default surface");
            discovered_monitors.push((
                MonitorConfig {
                    id: MonitorId(0),
                    name: "default".into(),
                    width: 1920,
                    height: 1080,
                    x: 0,
                    y: 0,
                    scale: 1.0,
                },
                outputs.first().cloned().unwrap_or_else(|| {
                    state.output_state.outputs().next().unwrap_or_else(|| {
                        panic!("No Wayland outputs found — is the compositor running?")
                    })
                }),
            ));
        }

        // Create layer shell surfaces for each monitor
        for (monitor, output) in &discovered_monitors {
            info!("Creating layer shell surface for '{}'...", monitor.name);

            let wl_surface = state.compositor_state.create_surface(&qh);

            let layer_surface = state.layer_shell.create_layer_surface(
                &qh,
                wl_surface.clone(),
                Layer::Background,
                Some("kroma-wallpaper"),
                Some(output),
            );

            // Configure the layer surface to cover the entire monitor
            layer_surface.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
            layer_surface.set_exclusive_zone(-1);
            layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer_surface.set_size(monitor.width, monitor.height);

            // Commit to trigger configure event from compositor
            wl_surface.commit();

            let created = CreatedSurface {
                _monitor: monitor.clone(),
                _layer_surface: layer_surface,
                wl_surface,
                width: monitor.width,
                height: monitor.height,
                configured: false,
            };

            self.surfaces.insert(monitor.id.0, created);
        }

        // Roundtrip to receive configure events
        event_queue
            .roundtrip(&mut state)
            .context("Wayland roundtrip failed during surface configuration")?;
        event_queue
            .roundtrip(&mut state)
            .context("Second Wayland roundtrip failed")?;

        // Update surface configured state
        let configured = state
            .configured_surfaces
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for (id, surface) in self.surfaces.iter_mut() {
            if let Some(&(w, h)) = configured.get(id) {
                surface.configured = true;
                if w > 0 {
                    surface.width = w;
                }
                if h > 0 {
                    surface.height = h;
                }
                info!(
                    "Surface for monitor {} configured: {}x{}",
                    id, surface.width, surface.height
                );
            } else {
                surface.configured = true;
                info!(
                    "Surface for monitor {} using default size: {}x{}",
                    id, surface.width, surface.height
                );
            }
        }

        self.monitors = discovered_monitors.into_iter().map(|(m, _)| m).collect();

        info!(
            "All {} layer shell surface(s) created and configured",
            self.surfaces.len()
        );
        Ok(())
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
        Ok(self
            .surface_size(monitor.0)
            .expect("Error getting surface size"))
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

        let ptr = surface.wl_surface.id().as_ptr() as *mut std::ffi::c_void;
        let non_null = std::ptr::NonNull::new(ptr).context("Wayland surface pointer is null")?;

        Ok(raw_window_handle::RawWindowHandle::Wayland(
            raw_window_handle::WaylandWindowHandle::new(non_null),
        ))
    }

    fn list_monitors(&self) -> Result<Vec<MonitorConfig>> {
        Ok(self.monitors.clone())
    }

    fn dispatch(&mut self) -> Result<()> {
        if let Some(ref conn) = self.connection {
            conn.flush().context("Failed to flush Wayland connection")?;
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
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        info!("Layer surface closed by compositor");
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

        // Store the configure event
        let mut configured = self
            .configured_surfaces
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let id = configured.len() as u32;
        configured.insert(id, (w, h));

        // Acknowledge the configure by committing the surface
        layer.wl_surface().commit();
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
