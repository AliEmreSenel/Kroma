# Kroma Development Guide

## Architecture

```
kroma-shared/     — Types, traits, translator, shade package handling
kroma-daemon/     — Headless renderer (wgpu, Wayland, X11, audio, IPC server)
kroma-cli/        — Import, inspect, pack, and daemon IPC control
```

## Key Design Decisions

### Shader Pipeline
- **shaderc** for GLSL→SPIR-V (handles ALL valid GLSL, unlike naga's GLSL frontend)
- **naga** for SPIR-V→WGSL (robust SPIR-V frontend)
- Translator preprocesses Shadertoy → Kroma GLSL 450 before compilation

### Surface Backends
- Auto-detected via `XDG_SESSION_TYPE` environment variable
- Wayland: smithay-client-toolkit + wlr-layer-shell
- X11: x11rb + `_NET_WM_WINDOW_TYPE_DESKTOP` windows
- wgpu surface creation via raw-window-handle 0.6

### IPC
- Unix socket at `/tmp/kroma-daemon.sock`
- Newline-delimited JSON with `#[serde(tag = "type")]`
- Client must use `BufReader::read_line()` (NOT `read_to_string()`)

### Uniform Layout
- `ShaderUniforms` is `#[repr(C)]`, 64 bytes, Pod + Zeroable
- `u_mouse` is `vec4` (Shadertoy iMouse semantics: pixel coords)
- `u_resolution` is `vec2` (translator wraps as `vec3(u_resolution, 1.0)`)

## Build & Run

```bash
cargo build --workspace          # Build all crates
cargo test --workspace           # Run all tests
cargo run --bin kroma-daemon     # Start renderer daemon
cargo run --bin kroma -- --help  # Run CLI commands
```

## File Types in .shade Packages
- `.frag` — GLSL fragment shader (editable as code or node graph)
- `config.toml` — Package configuration (editable as code or settings UI)
- `assets/*.jpg|png|mp4` — Textures and videos (viewable)
- `textures/*.frag` — Programmatic texture generators
