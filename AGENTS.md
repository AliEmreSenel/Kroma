# AGENTS.md — Kroma Project

## Project Overview

**Kroma** is a high-performance, modular wallpaper engine for Linux. It renders real-time, data-driven shaders and video streams directly to the desktop background. The primary target is Wayland/Hyprland, with modular support for other compositors.

## Architecture

Kroma is a **Rust workspace** with the following crates:

| Crate | Type | Purpose |
|---|---|---|
| `kroma-shared` | Library | Core traits, types, IPC protocol, .shade format, shader translator |
| `kroma-daemon` | Binary | Headless renderer — holds wgpu context, manages render loop & data aggregation |
| `kroma-gui` | Binary | User-facing config tool — browse/import .shade packages, Shadertoy translator UI |

## Core Principles

1. **Trait-Driven Modularity**: Every major subsystem (`SurfaceProvider`, `DataProvider`, `VideoDecoder`) is defined by a Rust trait. Core logic NEVER couples to a specific backend.
2. **Translation, Not Emulation**: Shadertoy code is parsed, sanitized, and transpiled into Kroma format — never executed directly.
3. **Safety First**: `unsafe` is allowed ONLY for FFI and GPU/Wayland interop. It MUST be wrapped in safe abstractions and documented.
4. **Zero-Interference**: The wallpaper layer NEVER captures or blocks input events. Cursor data comes from IPC.
5. **Performance**: Pause rendering on fullscreen/inactive workspaces. Use DMA-BUF zero-copy where possible.

## Key Traits

### `SurfaceProvider` (windowing backend)
- `connect()` → Initialize display server connection
- `create_surface(monitor)` → Create the drawing surface (LayerShell)
- `list_monitors()` → Current monitor layout

### `DataProvider` (data feeds)
- `get_audio_spectrum()` → 512-band normalized audio FFT
- `get_system_stats()` → CPU, RAM, Battery
- `get_cursor_pos()` → Normalized mouse position via IPC

### `VideoDecoder` (video textures)
- `load(path)` → Open video file
- `next_frame()` → Next RGBA frame buffer
- `seek(timestamp)` → Seek/loop control

## .shade Format

A `.shade` file is a ZIP containing:
- `shader.frag` — Kroma-compliant GLSL
- `config.toml` — Metadata, uniforms, texture bindings
- `preview.jpg` — Thumbnail
- `assets/` — Video/image resources

## Coding Standards

- Rust 2021 Edition
- Use `anyhow::Result` for error propagation
- Use `thiserror` for custom error types
- Prefer `log` + `env_logger` for logging
- All public APIs must have doc comments
- Tests for trait implementations and the shader translator
- `cargo clippy` must pass with no warnings
- `cargo fmt` enforced

## IPC Protocol

Daemon ↔ GUI communication uses Unix domain sockets at `$XDG_RUNTIME_DIR/kroma.sock` with JSON-serialized messages.

## GUI v2 Architecture

The GUI is undergoing a full modular rewrite (see `docs/GUI_DESIGN.md` for complete spec).

### Design Decisions (Locked)
- **Layout**: Full IDE-style docking system (drag-split-tab), floating windows deferred
- **Live Preview**: Daemon IPC — low-res JPEG frames over Unix socket
- **Sub-graphs**: Tabbed navigation with breadcrumbs + tree sidebar (UE5 Blueprint-style)
- **Undo/Redo**: Full command-based from day one (Command trait + CommandHistory)
- **Themes**: User-selectable (Tokyo Night, Catppuccin, Nord, Dracula, One Dark)
- **Video Preview**: Full inline playback via FFmpeg in GUI
- **Refactor Strategy**: Incremental — extract modules from monolith, then enhance

### Module Structure
```
kroma-gui/src/
├── app.rs              # Thin orchestrator (update/view dispatch)
├── message.rs          # Namespaced Message enum
├── dock/               # Binary split tree docking system
├── panels/             # Self-contained Panel trait implementations
│   ├── dashboard.rs    # Daemon status, FPS, quick actions
│   ├── node_editor.rs  # Graph canvas + sub-graph breadcrumbs
│   ├── code_editor.rs  # GLSL text editor
│   ├── asset_browser.rs # File tree with drag-to-import
│   ├── asset_preview.rs # Image/video/font/shader preview
│   ├── properties.rs   # Node/asset inspector
│   ├── library.rs      # .shade package grid browser
│   ├── live_preview.rs # Daemon frame stream display
│   ├── import.rs       # Shadertoy import
│   ├── error_log.rs    # Compile errors + messages
│   └── settings.rs     # Theme, directories, API keys
├── editor/             # Node editor internals (canvas, palette, wires, sub-graphs)
├── commands/           # Command pattern for undo/redo
├── ipc/                # Async daemon communication
└── theme/              # Theme trait + 5 built-in themes
```

### Panel Trait
Every dockable panel implements `Panel` with: `id()`, `title()`, `icon()`, `view()`, `update()`, `subscription()`.

### Implementation Phases
1. **Docking System & Module Extraction** — Foundation
2. **Command System & Undo/Redo** — All mutations through commands
3. **Node Editor Enhancements** — Sub-graphs, multi-select, palette popup
4. **Async IPC & Live Preview** — Non-blocking daemon comms + frame streaming
5. **Asset Management & Previews** — Drag-drop import, image/video/font preview
6. **Theme System** — 5 themes with full token coverage
7. **Polish & Error Tolerance** — No unwrap(), toast notifications, robustness
8. **Floating Windows** — Multi-window docking (future)

## Build & Run

```bash
cargo build --workspace          # Build everything
cargo run -p kroma-daemon        # Run the daemon
cargo run -p kroma-gui           # Run the GUI
cargo test --workspace           # Run all tests
```
