# Kroma

**A high-performance, modular wallpaper engine for Linux.**

Kroma renders real-time, data-driven shaders and video streams directly to your desktop background. Built for Wayland (Hyprland and GravityWM primary), with a trait-driven architecture designed for portability to KDE, GNOME, and X11.

## Features

- **Real-time shaders** — GLSL fragment shaders running on the GPU via wgpu
- **Data-driven** — Shaders react to system stats (CPU, RAM, battery), audio spectrum, mouse position, and time
- **Shadertoy translator** — Import shaders from Shadertoy with automatic transpilation
- **`.shade` v2 packages** — Chunked container format with strict magic/version checks and per-entry compression
- **Video textures** — Use video files as shader inputs (ffmpeg/gstreamer backend)
- **Compositor awareness** — Auto-pauses on fullscreen apps and inactive workspaces, with cursor tracking on Hyprland and GravityWM
- **Modular architecture** — Every subsystem is a swappable Rust trait

## Architecture

```
┌──────────────┐     IPC (Unix Socket)     ┌──────────────┐
│  kroma-cli   │ ◄──────────────────────── │ kroma-daemon │
│  (control)   │                           │  (renderer)  │
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
| `kroma-cli` | CLI tool for packaging, importing, inspection, and daemon IPC control |

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
cargo run -p kroma-cli -- import examples/shadertoy/plasma.glsl --name "Plasma" --author "Author"
```

### Send commands to the daemon

```bash
kroma load /path/to/shader.shade
kroma load /path/to/next.shade --force
kroma unload
kroma pause
kroma resume
kroma shutdown
```

### Install as a systemd user service

```bash
cp contrib/kroma-daemon.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now kroma-daemon
```

## `.shade` Package Format

Kroma runtime loads v2 `.shade` packages only.

- v2 uses a chunked container with strict magic/version validation.
- Embedded video assets are read as streamable chunk data (no full temp extraction path).

Pack a folder to v2 `.shade`:

```bash
kroma pack ./my_shade --output my_shade.shade
```

`kroma pack` embeds only files referenced by `config.toml`.
Absolute-path references are not embedded by default.

Compression controls:

```bash
kroma pack ./my_shade --codec zstd --level 8
kroma pack ./my_shade --entry-compression assets/video.mp4=none
kroma pack ./my_shade --entry-compression assets/shaders/active.frag=zstd:12
```

Inspect v2 `.shade` packages:

```bash
# default: summary
kroma inspect ./my_shade.shade

# integrity verification modes
kroma inspect ./my_shade.shade verify --mode fast
kroma inspect ./my_shade.shade verify --mode checksum
kroma inspect ./my_shade.shade verify --mode decode

# deep introspection
kroma inspect ./my_shade.shade list --format json
kroma inspect ./my_shade.shade chunks --depth 4
kroma inspect ./my_shade.shade stats
kroma inspect ./my_shade.shade dump-entry config.toml
kroma inspect ./my_shade.shade dump-chunk assets/video.mp4 0 --decoded
```

`kroma inspect` supports nested subcommands only, text/json output, and
chunk-depth diagnostics from `--depth 1` through `--depth 4`.

Example `config.toml`:

```toml
[meta]
name = "Cyber Rain"
author = "Neo"
version = "1.0"

[states.active]
length = 8.0
shader = "assets/shaders/active.frag"

[states.active.uniforms]
speed = { type = "float", min = 0.1, max = 5.0, default = 1.0 }
color_shift = { type = "bool", default = false }

[states.active.textures]
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

[openrgb]
enabled = false
host = "127.0.0.1"
port = 6742
update_fps = 30
brightness = 1.0
gamma = 1.0

[[openrgb.keyboards]]
name = "*keyboard*"
# serial = "exact-serial-if-needed"
# monitor = "DP-1"

[[monitors]]
name = "DP-1"
enabled = true
```

### OpenRGB keyboard output

Kroma can mirror the exact final wallpaper output—including lifecycle and
shade-to-shade transitions—to per-key OpenRGB keyboard lighting. Enable the SDK
server in OpenRGB, enable the configuration above, and add at least one
`[[openrgb.keyboards]]` selector.

- `name` is a case-insensitive glob supporting `*` and `?`.
- `serial`, when set and non-empty, must match exactly.
- `monitor`, when set, must match the currently rendered primary monitor.
- Only keyboards with OpenRGB matrix metadata are controlled.
- `update_fps` is hard-limited to 30; `0` disables lighting output.
- `brightness` is clamped to `0.0..=1.0`; `gamma` is clamped to `0.1..=4.0`.

OpenRGB I/O runs on a dedicated worker with an overwrite-latest frame queue.
Connection failures do not interrupt rendering and are retried with exponential
backoff. Kroma leaves the last mode and colors unchanged when a shade unloads
or the daemon exits, then closes its SDK connection.

To inspect controller names, serials, and matrix metadata without changing any
colors, run:

```bash
cargo run -p openrgb-client --example list_controllers -- 127.0.0.1 6742
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
└── kroma-cli/        # Command-line import/pack/inspect + daemon control
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
