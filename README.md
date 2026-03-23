# Kroma

**A high-performance, modular wallpaper engine for Linux.**

Kroma renders real-time, data-driven shaders and video streams directly to your desktop background. Built for Wayland (Hyprland primary), with a trait-driven architecture designed for portability to KDE, GNOME, and X11.

## Features

- **Real-time shaders** — GLSL fragment shaders running on the GPU via wgpu
- **Data-driven** — Shaders react to system stats (CPU, RAM, battery), audio spectrum, mouse position, and time
- **Shadertoy translator** — Import shaders from Shadertoy with automatic transpilation
- **`.shade` packages** — Distributable ZIP format containing shader, config, previews, and assets
- **Video textures** — Use video files as shader inputs (ffmpeg/gstreamer backend)
- **Compositor awareness** — Auto-pauses on fullscreen apps and inactive workspaces (Hyprland IPC)
- **Modular architecture** — Every subsystem is a swappable Rust trait

## Architecture

```
┌──────────────┐     IPC (Unix Socket)     ┌──────────────┐
│  kroma-gui   │ ◄──────────────────────── │ kroma-daemon │
│  (config UI) │                           │  (renderer)  │
└──────────────┘                           └──────┬───────┘
                                                  │
                              ┌────────────┬──────┴──────┬────────────┐
                              │            │             │            │
                         SurfaceProvider DataProvider VideoDecoder AudioProvider
                         (Wayland/Layer)  (sysinfo)   (ffmpeg)    (pipewire)
```

The system is split into two binaries:

| Binary | Purpose |
|---|---|
| `kroma-daemon` | Headless background renderer — holds GPU context, manages render loop |
| `kroma-gui` | CLI/GUI tool for configuration, shader import, and IPC control |

## Quick Start

### Build

```bash
cargo build --workspace --release
```

### Run the daemon

```bash
cargo run -p kroma-daemon
# or
./target/release/kroma-daemon

# The daemon starts in a neutral fallback state until a `.shade` package is loaded.
```

### Import a Shadertoy shader

```bash
cargo run -p kroma-gui -- import examples/shadertoy/plasma.glsl "Plasma" "Author"
```

### Send commands to the daemon

```bash
kroma-gui load /path/to/shader.shade
kroma-gui pause
kroma-gui resume
kroma-gui shutdown
```

### Install as a systemd user service

```bash
cp contrib/kroma-daemon.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now kroma-daemon
```

## `.shade` Package Format

A `.shade` file is a ZIP containing:

| File | Purpose |
|---|---|
| `shader.frag` | Kroma-compliant GLSL fragment shader |
| `config.toml` | Metadata, uniforms, texture bindings |
| `preview.jpg` | Thumbnail (optional) |
| `assets/` | Video/image resources (optional) |

Example `config.toml`:

```toml
[meta]
name = "Cyber Rain"
author = "Neo"
version = "1.0"

[uniforms]
speed = { type = "float", min = 0.1, max = 5.0, default = 1.0 }
color_shift = { type = "bool", default = false }

[textures]
channel0 = { type = "video", source = "assets/rain_loop.mp4", loop = true, hot_reload = true }
```

## Configuration

Daemon config lives at `$XDG_CONFIG_HOME/kroma/config.toml` (defaults to `~/.config/kroma/config.toml`):

```toml
target_fps = 60
pause_on_fullscreen = true
pause_on_inactive = true
gpu_power = "low"
current_shade = "/path/to/your.shade"

[preview]
default_width = 480
default_height = 270
default_target_fps = 15
max_target_fps = 60

[logging]
fps_log_interval_secs = 5

[runtime]
persist_current_shade = true

[[monitors]]
name = "DP-1"
enabled = true
```

## Development

### Run all tests

```bash
cargo test --workspace
```

### Project structure

```
crates/
├── kroma-shared/     # Core traits, types, IPC, .shade format, translator
├── kroma-daemon/     # The render daemon
└── kroma-gui/        # GUI / CLI client
examples/
├── shaders/          # Kroma-native example shaders
└── shadertoy/        # Shadertoy examples for translator testing
contrib/
└── kroma-daemon.service  # systemd user service
docs/
├── TEXTURES.md       # Texture types, nesting, and composition patterns
└── PRD.md            # Product Requirements Document
```

## License

MIT
