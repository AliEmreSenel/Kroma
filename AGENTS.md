# AGENTS.md — Kroma Project

## Project Overview

**Kroma** is a high-performance, modular wallpaper engine for Linux. It renders real-time, data-driven shaders and video streams directly to the desktop background. The primary target is Wayland/Hyprland, with modular support for other compositors.

## Architecture

Kroma is a **Rust workspace** with the following crates:

| Crate | Type | Purpose |
|---|---|---|
| `kroma-shared` | Library | Core traits, types, IPC protocol, .shade format, shader translator |
| `kroma-daemon` | Binary | Headless renderer — holds wgpu context, manages render loop & data aggregation |
| `kroma-cli` | Binary | Import, pack, inspect, migrate, and daemon IPC control |

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

Daemon communication uses Unix domain sockets at `$XDG_RUNTIME_DIR/kroma.sock` with JSON-serialized messages.

## GUI Branch Note

The GUI/editor stack was moved off the default branch. Use `feature/gui` for GUI-specific development.

## Build & Run

```bash
cargo build --workspace          # Build everything
cargo run -p kroma-daemon        # Run the daemon
cargo run -p kroma-cli -- --help # Run CLI commands
cargo test --workspace           # Run all tests
```
