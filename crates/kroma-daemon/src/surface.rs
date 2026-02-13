//! Wayland Layer Shell surface provider for Hyprland.
//!
//! Implements [`kroma_shared::traits::SurfaceProvider`] using
//! `smithay-client-toolkit` and `wayland-client`. Creates real
//! `zwlr_layer_shell_v1` surfaces on each monitor for wallpaper rendering.
//!
//! ## Pointer access
//!
//! wayland-backend 0.3 intentionally hides raw `wl_display*` / `wl_proxy*`
//! pointers from its public API.  wgpu however requires these raw pointers
//! to create a Vulkan/EGL surface.  We obtain them by reading the known
//! memory layout of the internal `InnerObjectId` struct.  A runtime size
//! assertion guards against layout changes in future wayland-backend versions.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use log::{info, warn};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::WaylandSurface,
    shell::wlr_layer::{
        Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler,
        LayerSurface, LayerSurfaceConfigure,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
    Connection, Proxy, QueueHandle,
};

use kroma_shared::traits::SurfaceProvider;
use kroma_shared::types::{MonitorConfig, MonitorId};

// ---------------------------------------------------------------------------
// Raw pointer extraction from wayland-backend's opaque types
// ---------------------------------------------------------------------------

/// Determine the byte offset of the `ptr` field within an `ObjectId` struct.
///
/// wayland-backend 0.3.x's `ObjectId` wraps `InnerObjectId { id: u32, ptr: *mut
/// wl_proxy, alive: Option<Arc<…>>, interface: &'static Interface }`.  Because
/// the struct has no `#[repr(C)]`, the Rust compiler is free to reorder fields.
///
/// We exploit properties of the **display** ObjectId to locate `ptr`:
/// - `id` = 1  (≤ 0xffff — way below any plausible pointer value)
/// - `alive` = `None` = 0  (display proxy is never tracked with an Arc)
/// - `interface` = known pointer (returned by `ObjectId::interface()`)
/// - `ptr` = the remaining non-null pointer — this is what we want.
///
/// # Safety
///
/// Relies on reading the raw memory of an opaque struct.  A runtime size
/// assertion guards against unreasonable layouts.
unsafe fn find_ptr_offset(conn: &Connection) -> Option<usize> {
    let display_id = conn.backend().display_id();
    let interface_val = display_id.interface() as *const _ as usize;
    let proto_id = display_id.protocol_id();
    let size = std::mem::size_of_val(&display_id);

    info!("ObjectId diagnostics: size={}, protocol_id={}, interface_addr=0x{:x}",
        size, proto_id, interface_val);

    if size < 16 {
        warn!("ObjectId size {} — too small for expected layout", size);
        return None;
    }

    let base = &display_id as *const _ as *const u8;

    // Dump the full struct memory for analysis
    let mut hex = String::new();
    for i in 0..size {
        if i > 0 && i % 8 == 0 { hex.push(' '); }
        let byte = std::ptr::read(base.add(i));
        hex.push_str(&format!("{:02x}", byte));
    }
    info!("ObjectId raw bytes: {}", hex);

    // Print each 8-byte slot
    for off in (0..size).step_by(8) {
        let val = std::ptr::read(base.add(off) as *const usize);
        info!("  offset {:2}: 0x{:016x}", off, val);
    }

    // Strategy: find the non-null, non-interface, non-id pointer.
    // The display ObjectId has alive=None (0), id=1, interface=known, ptr=unknown.
    for off in (0..size).step_by(8) {
        let val = std::ptr::read(base.add(off) as *const usize);
        if val == 0 {
            continue; // None/null  (alive = None, or padding)
        }
        if val == interface_val {
            continue; // the `interface` field
        }
        if val < 0x10000 {
            continue; // the `id` field (u32 in an 8-byte aligned slot)
        }
        // Validate: a real pointer should be page-aligned or at least 8-byte aligned
        // (malloc returns 16-byte aligned on modern glibc)
        if !val.is_multiple_of(8) {
            warn!("  offset {:2}: 0x{:x} — NOT 8-byte aligned, skipping", off, val);
            continue;
        }
        info!("ObjectId ptr field at offset {} (display ptr = 0x{:x})", off, val);
        return Some(off);
    }

    // Fallback: try accepting non-aligned pointers
    for off in (0..size).step_by(8) {
        let val = std::ptr::read(base.add(off) as *const usize);
        if val == 0 || val == interface_val || val < 0x10000 {
            continue;
        }
        warn!("Fallback: using offset {} (0x{:x}) — might not be a real pointer", off, val);
        return Some(off);
    }

    warn!("Could not locate ptr field in ObjectId (size = {})", size);
    None
}

/// Extract the raw `wl_proxy*` from an `ObjectId` at a pre-determined offset.
///
/// # Safety
/// `offset` must be the value returned by [`find_ptr_offset`].
unsafe fn extract_proxy_ptr_at(
    obj_id: &wayland_client::backend::ObjectId,
    offset: usize,
) -> Option<std::ptr::NonNull<std::ffi::c_void>> {
    let base = obj_id as *const _ as *const u8;
    let ptr = std::ptr::read(base.add(offset) as *const *mut std::ffi::c_void);
    std::ptr::NonNull::new(ptr)
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
    /// The raw display pointer for wgpu surface creation.
    display_ptr: Option<std::ptr::NonNull<std::ffi::c_void>>,
    /// Byte offset of the `ptr` field inside `ObjectId` (detected once).
    ptr_offset: Option<usize>,
}

impl WaylandSurfaceProvider {
    pub fn new() -> Self {
        Self {
            connection: None,
            monitors: Vec::new(),
            surfaces: HashMap::new(),
            display_ptr: None,
            ptr_offset: None,
        }
    }

    /// Get the raw display pointer (wl_display*) for wgpu.
    pub fn display_ptr(&self) -> Option<std::ptr::NonNull<std::ffi::c_void>> {
        self.display_ptr
    }

    /// Get the raw surface pointer (wl_surface*) for a given monitor.
    pub fn surface_ptr(&self, monitor_id: u32) -> Option<std::ptr::NonNull<std::ffi::c_void>> {
        let offset = self.ptr_offset?;
        self.surfaces.get(&monitor_id).and_then(|s| {
            let obj_id = s.wl_surface.id();
            unsafe { extract_proxy_ptr_at(&obj_id, offset) }
        })
    }

    /// Get the configured dimensions for a monitor's surface.
    pub fn surface_size(&self, monitor_id: u32) -> Option<(u32, u32)> {
        self.surfaces.get(&monitor_id).map(|s| (s.width, s.height))
    }

    /// Check if a monitor's surface has been configured by the compositor.
    #[allow(dead_code)]
    pub fn is_configured(&self, monitor_id: u32) -> bool {
        self.surfaces.get(&monitor_id).map(|s| s.configured).unwrap_or(false)
    }

    /// Create layer shell surfaces on all discovered monitors.
    pub fn create_all_surfaces(&mut self) -> Result<()> {
        let conn = self.connection.as_ref()
            .context("Not connected to Wayland")?;

        let (globals, mut event_queue) = registry_queue_init(conn)
            .context("Failed to initialize Wayland registry")?;
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
        event_queue.roundtrip(&mut state)
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
                    name: info.name.clone().unwrap_or_else(|| format!("output-{}", idx)),
                    width,
                    height,
                    x,
                    y,
                    scale: info.scale_factor as f64,
                };

                info!("Discovered output: {} ({}x{} @ {},{} scale {})",
                    monitor.name, width, height, x, y, monitor.scale);
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
                    state.output_state.outputs().next()
                        .unwrap_or_else(|| panic!("No Wayland outputs found — is the compositor running?"))
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
        event_queue.roundtrip(&mut state)
            .context("Wayland roundtrip failed during surface configuration")?;
        event_queue.roundtrip(&mut state)
            .context("Second Wayland roundtrip failed")?;

        // Update surface configured state
        let configured = state.configured_surfaces.lock().unwrap_or_else(|e| e.into_inner());
        for (id, surface) in self.surfaces.iter_mut() {
            if let Some(&(w, h)) = configured.get(id) {
                surface.configured = true;
                if w > 0 { surface.width = w; }
                if h > 0 { surface.height = h; }
                info!("Surface for monitor {} configured: {}x{}", id, surface.width, surface.height);
            } else {
                surface.configured = true;
                info!("Surface for monitor {} using default size: {}x{}", id, surface.width, surface.height);
            }
        }

        self.monitors = discovered_monitors.into_iter().map(|(m, _)| m).collect();

        info!("All {} layer shell surface(s) created and configured", self.surfaces.len());
        Ok(())
    }
}

impl SurfaceProvider for WaylandSurfaceProvider {
    fn connect(&mut self) -> Result<()> {
        info!("Connecting to Wayland display...");

        let conn = Connection::connect_to_env()
            .context("Failed to connect to Wayland display. Is a compositor running?")?;

        // Detect the ObjectId layout and extract the raw display pointer
        let ptr_offset = unsafe { find_ptr_offset(&conn) };
        self.ptr_offset = ptr_offset;

        if let Some(offset) = ptr_offset {
            let display_id = conn.backend().display_id();
            self.display_ptr = unsafe { extract_proxy_ptr_at(&display_id, offset) };
        }

        if self.display_ptr.is_none() {
            warn!("Could not extract raw wl_display pointer — GPU surface rendering unavailable");
        } else {
            info!("Wayland connection established (display ptr: {:?})", self.display_ptr);
        }

        self.connection = Some(conn);
        Ok(())
    }

    fn create_surface(
        &self,
        monitor: MonitorId,
    ) -> Result<raw_window_handle::RawWindowHandle> {
        let surface = self.surfaces.get(&monitor.0)
            .context(format!("No surface created for monitor {:?}", monitor))?;

        let offset = self.ptr_offset
            .context("ObjectId layout not detected — call connect() first")?;

        let obj_id = surface.wl_surface.id();
        let ptr = unsafe { extract_proxy_ptr_at(&obj_id, offset) }
            .context("Failed to extract raw wl_surface pointer")?;

        Ok(raw_window_handle::RawWindowHandle::Wayland(
            raw_window_handle::WaylandWindowHandle::new(ptr),
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
    ) {}

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {}

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {}

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {}
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
    ) {}

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
    fn closed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
    ) {
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
        let mut configured = self.configured_surfaces.lock().unwrap_or_else(|e| e.into_inner());
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
