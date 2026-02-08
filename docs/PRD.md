# Product Requirements Document: Kroma

**Version:** 1.0  
**Date:** 2026-02-07  
**Status:** Active Development  

---

## 1. Executive Summary

Kroma is a high-performance, modular wallpaper engine for Linux. It renders real-time, data-driven shaders and video streams directly to the desktop background. Unlike existing solutions, Kroma prioritizes modularity via traits, zero-config shader translation, and resource efficiency. It bridges the gap between raw system data (audio, weather, system stats) and artistic visualization.

## 2. Core Philosophy & Constraints

- **Trait-Driven Everything:** Every major subsystem (Windowing, Audio, Video, System Stats) must be defined by a Rust trait. We strictly avoid tight coupling to specific backends (e.g., Hyprland, Pipewire) in the core logic.
- **Translation, Not Emulation:** We do not "run" Shadertoy code directly. We parse it, sanitize it, and transpile it into Kroma's standardized format automatically.
- **Safety First:** `unsafe` code is permitted only for FFI (Foreign Function Interface) and GPU/Wayland interoperability. It must be wrapped in safe abstractions and heavily documented.
- **Zero-Interference Input:** The wallpaper must strictly never block mouse clicks or keyboard focus. It acts as a passive layer that reads input data (via IPC) without consuming input events.

## 3. System Architecture

The system is divided into two binaries to ensure stability.

### 3.1. The Daemon (`kroma-daemon`)

The headless background process. It holds the `wgpu` context and manages the render loop.

**Responsibility:**
- Initialization of the `WallpaperBackend` (Wayland Layer Shell)
- Loading and compiling the `.shade` package
- Managing the `DataAggregator` threads (Audio, System, Weather)
- Decoding video textures (ffmpeg/gstreamer)
- Rendering the frame

**Failure State:** If the daemon crashes, it restarts cleanly without taking down the user's session.

### 3.2. The GUI (`kroma-gui`)

The user-facing configuration tool.

**Responsibility:**
- Browsing/Importing `.shade` packages
- The Translator: Ingesting Shadertoy URLs/code and converting them
- IPC Communication: Sending config updates to the daemon live

## 4. Technical Specifications (The Traits)

### 4.1. The Windowing Backend — `SurfaceProvider`

Allows swapping Hyprland for KDE, GNOME, or even X11 later.

```rust
trait SurfaceProvider {
    /// Initialize the connection to the display server
    fn connect(&mut self) -> Result<()>;
    
    /// Create the drawing surface (e.g., LayerShell surface)
    fn create_surface(&self, monitor: MonitorId) -> RawWindowHandle;
    
    /// Return current monitor layout
    fn list_monitors(&self) -> Vec<MonitorConfig>;
}
```

### 4.2. The Data Provider — `DataProvider`

Allows mocking data for testing or swapping providers.

```rust
trait DataProvider {
    /// Returns normalized audio spectrum (512 bands, 0.0-1.0)
    fn get_audio_spectrum(&self) -> Vec<f32>;
    
    /// Returns system stats (CPU, RAM, Battery)
    fn get_system_stats(&self) -> SystemStats;
    
    /// Returns mouse position (normalized 0.0-1.0 relative to screen)
    fn get_cursor_pos(&self) -> Vec2;
}
```

**Note:** For the Hyprland implementation, `get_cursor_pos` will query the Hyprland IPC socket.

### 4.3. The Video Decoder — `VideoDecoder`

Handles the "Video Texture" requirement.

```rust
trait VideoDecoder {
    /// Opens a video file and prepares the stream
    fn load(path: &Path) -> Result<Self>;
    
    /// Returns the next frame as a raw byte buffer (RGBA)
    fn next_frame(&mut self) -> Option<&[u8]>;
    
    /// Seek/Loop control
    fn seek(&mut self, timestamp: f64);
}
```

**Implementation Strategy:** We will use `ffmpeg-next` or `gstreamer-rs` to implement this trait.

## 5. The "Magic" Translator (Shadertoy → Kroma)

We will build a transpiler step that runs during import.

**Input:** Raw Shadertoy GLSL.

**Process:**

1. **Regex/AST Parsing:** Identify Shadertoy globals (`iTime`, `iResolution`, `iChannel0`).
2. **Mapping:**
   - `iTime` → `u_time`
   - `iResolution` → `u_resolution`
   - `iMouse` → `u_mouse` (derived from IPC)
   - `iChannel0` (Video) → Map to `kroma_video_sampler_0`
   - `iChannel1` (Audio) → Map to `kroma_audio_sampler`
3. **Output:** A clean Kroma-compliant `.frag` file and a generated `config.toml`.

## 6. The Data Format (`.shade`)

The distributable unit of a wallpaper. It is a ZIP file.

| File | Purpose |
|---|---|
| `shader.frag` | The Kroma-compliant GLSL shader |
| `config.toml` | Metadata, default values, and resource paths |
| `preview.jpg` | Thumbnail |
| `assets/` | Directory containing video files (.mp4, .webm) or images |

### Example `config.toml`:

```toml
[meta]
name = "Cyber Rain"
author = "Neo"
version = "1.0"

[uniforms]
speed = { type = "float", min = 0.1, max = 5.0, default = 1.0 }
color_shift = { type = "bool", default = false }

[textures]
channel0 = { type = "video", source = "assets/rain_loop.mp4", loop = true }
```

## 7. Performance & Optimization Strategy

- **Compositor Awareness (Hyprland IPC):**
  - Event: `activewindow(fullscreen)` → Action: Pause Render Loop (0 FPS)
  - Event: `workspace(inactive)` → Action: Pause rendering on the invisible monitor
- **Zero-Copy Rendering:** Use DMA-BUF where possible for video textures to upload directly to GPU memory without CPU staging.

## 8. Implementation Roadmap

### Phase 1: The Core (Skeleton)
- [x] Set up Rust workspace (daemon, gui, shared)
- [x] Implement `SurfaceProvider` trait for Hyprland (using `smithay-client-toolkit`)
- [ ] Get a solid color triangle rendering on the background layer

### Phase 2: The Data (Nerves)
- [x] Implement `DataProvider` trait (System info, Time)
- [x] Implement Hyprland IPC listener for `get_cursor_pos`
- [ ] Create the Shader Uniform buffer struct in wgpu

### Phase 3: The Media (Eyes & Ears)
- [x] Implement `VideoDecoder` trait (ffmpeg integration)
- [ ] Implement Audio FFT analysis (Pipewire/Pulse)

### Phase 4: The Translator (Brain)
- [x] Build the Regex parser to convert Shadertoy syntax to Kroma syntax
- [ ] Build the GUI to import and configure these files
