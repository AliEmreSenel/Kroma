# Product Requirements Document: Kroma

**Version:** 2.0  
**Date:** 2026-02-08  
**Status:** Active Development  

---

## 1. Executive Summary

Kroma is a high-performance, modular wallpaper engine for Linux. It renders real-time, data-driven shaders and video streams directly to the desktop background. Kroma prioritizes modularity via traits, zero-config shader translation, multi-desktop support (Hyprland, KDE, XFCE, GNOME, Sway), and a professional shader editing experience.

## 2. Core Philosophy & Constraints

- **Trait-Driven Everything:** Every major subsystem (Windowing, Audio, Video, System Stats) is defined by a Rust trait. No tight coupling to specific backends.
- **Translation, Not Emulation:** Shadertoy GLSL is transpiled (via shaderc + naga) into Kroma's standardized format automatically.
- **Safety First:** `unsafe` code only for FFI and GPU/Wayland/X11 interoperability, always wrapped in safe abstractions.
- **Zero-Interference Input:** The wallpaper never blocks mouse clicks or keyboard focus. It reads input data via IPC.
- **Desktop Audio Only:** Audio capture defaults to desktop/monitor loopback devices. Microphone access requires explicit opt-in.

## 3. System Architecture

### 3.1. The Daemon (`kroma-daemon`)

Headless background renderer holding the wgpu context.

**Backends:**
- **Wayland Layer Shell** — Hyprland, Sway, KDE Plasma Wayland
- **X11 Desktop Window** — KDE Plasma X11, XFCE, MATE, Cinnamon, i3
- **Headless** — Testing mode

**Backend auto-detection** via `XDG_SESSION_TYPE` / `XDG_CURRENT_DESKTOP`.

**Responsibilities:**
- Surface initialization (Wayland or X11)
- GLSL → SPIR-V → WGSL pipeline (shaderc + naga)
- Data aggregation (Audio, System stats)
- Frame rendering
- IPC server with compile error reporting

### 3.2. The CLI (`kroma-cli`)

Command-line tool for package lifecycle and daemon control:

1. **Import** — Convert Shadertoy shaders into `.shade` packages
2. **Pack** — Build v2 `.shade` containers from project folders
3. **Inspect** — Verify, list, and debug container structure/chunks
4. **IPC Control** — Load, pause, resume, reload, and shutdown daemon

## 4. Shader Translation Pipeline

1. Uniform/semantic replacement (iResolution→vec3, iMouse→vec4, etc.)
2. GLSL compatibility fixes (texture2D, precision, mat2, #version)
3. mainImage wrapping with Y-flip
4. GLSL 450 → SPIR-V via shaderc
5. SPIR-V → WGSL via naga

## 5. The `.shade` Format

Runtime format is `.shade` v2 (chunked container):

1. Strict magic/version validation.
2. Per-entry chunk table and independent decoding.
3. 256 KiB chunking with per-chunk checksum.
4. Per-entry compression policy (`auto|none|zstd|lz4`) with optional level.
5. Deterministic packaging order from config declaration order.

### 5.3 Runtime State Lifecycle

Runtime shader config is defined in `config.toml` under fixed keys:

1. `states.load` (optional one-shot)
2. `states.active` (optional loop)
3. `states.unload` (optional one-shot)

Each phase contains full shader structure (`shader`, `uniforms`, `textures`, `buffers`) plus `length`.

1. `active.length = 0` allows immediate interrupt to unload.
2. `active.length > 0` allows unload only on future loop boundaries.
3. Phase-local time/frame counters reset when entering a new phase.
4. After the last defined phase completes, daemon holds the final frame.

### 5.1 Embedded Video Rules

1. Embedded videos are decoded from streamable container reads.
2. Full extraction to temporary files is not allowed for embedded video playback.
3. Unsafe FFmpeg AVIO interoperability is isolated to one dedicated module.

### 5.2 Inspect Command Requirements

`kroma inspect` provides deep diagnostics for v2 `.shade` containers.

1. Nested subcommands only:
- `summary`
- `verify`
- `list`
- `chunks`
- `stats`
- `dump-entry`
- `dump-chunk`
2. Bare `kroma inspect <file>` defaults to `summary`.
3. Verify modes:
- `fast` (structure)
- `checksum` (CRC)
- `decode` (CRC + decompression)
4. Output formats:
- human text
- JSON
5. Chunk debug depth supports levels `1..4`.

## 6. IPC Protocol

Newline-delimited JSON.

Commands include:

1. `LoadShade { path, force, transition }`
2. `UnloadShade`
3. `SetUniform`
4. `Pause`, `Resume`, `Reload`, `LiveReload`, `StatusQuery`, `Shutdown`

Status payload includes lifecycle telemetry:

1. `current_phase` (`load|active|unload|transitioning|terminal|none`)
2. `pending_request_path`
3. `wait_reason` (`waiting_active_boundary|running_unload|running_load|idle`)

Daemon may return `LoadRejected { code, message }` for lifecycle policy rejections
(for example while waiting for active loop boundary and `force=false`, or while a transition
handoff is currently running).

## 7. Implementation Status

### Completed ✅
- Rust workspace (shared, daemon, cli)
- Wayland + X11 surface providers with auto-detection
- wgpu Vulkan rendering, DPI-aware
- shaderc + naga shader pipeline
- Shadertoy import/translation
- IPC client/server
- Audio capture (desktop loopback)
- Shade package tooling (pack, inspect)

### In Progress 🔧
- GLSL ↔ Node sync
- IPC compile error feedback
