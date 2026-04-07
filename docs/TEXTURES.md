# Texture System Guide

This document is the source-of-truth guide for Kroma texture types, how they behave at runtime, and how to combine them through nesting and shader composition.

## 1. Where Textures Are Defined

Textures are configured in `.shade` `config.toml` under `[textures]`.

Example:

```toml
[textures.background]
type = "image"
source = "assets/bg.png"
binding = 0

[textures.music]
type = "audio_spectrum"
source = "desktop"
fft_bands = 512
binding = 1
```

Shader textures can have their own nested textures and uniforms:

```toml
[textures.main]
type = "shader"
shader = "assets/main.frag"
width = 1024
height = 1024
binding = 0

[textures.main.textures.noise]
type = "image"
source = "assets/noise.png"
binding = 0

[textures.main.uniforms.strength]
type = "float"
min = 0.0
max = 2.0
default = 1.0
```

## 2. Shared Texture Fields

All texture entries use the same `TextureDef` schema.

Common fields:

- `type`: Texture type (`image`, `video`, `font`, `slideshow`, `audio_spectrum`, `noise`, `shader`)
- `source`: File path or source id (used by several types)
- `seed`: Explicit seed for `noise` textures
- `binding`: Preferred channel order index
- `optional`: If `true`, load failures degrade to placeholders instead of hard failing
- `hot_reload`: Enable filesystem reload for external disk sources
- `filter`: Declared filter mode (`linear`, `nearest`)
- `wrap`: Declared address mode (`repeat`, `clamp`, `mirror`)

Type-specific fields are documented per texture type below.

## 3. Binding Order And Channel Stability

Kroma sorts textures by `binding` when creating GPU bindings.

- Lower `binding` values come first
- Textures without `binding` are placed after bound textures
- For stable, deterministic channel layout, set `binding` explicitly

The same sorted-by-binding behavior is used for nested shader sub-textures.

## 4. Texture Type Reference

### 4.1 `image`

Purpose:
- Static RGBA texture from PNG/JPEG/WebP/GIF

Relevant fields:
- `source` (required)
- `optional`
- `hot_reload` (external disk files only)
- `binding`, `filter`, `wrap`

Runtime behavior:
- Decoded once, then uploaded
- Returns `Unchanged` after first upload unless hot-reload detects file changes
- If `optional = true` and load fails, uses a transparent 1x1 placeholder

Example:

```toml
[textures.channel0]
type = "image"
source = "assets/palette.png"
binding = 0
optional = true
hot_reload = true
```

### 4.2 `video`

Purpose:
- Decoded video frames uploaded to a texture over time

Relevant fields:
- `source` (required)
- `loop` (serialized as `loop`, stored as `looping`)
- `optional`
- `hot_reload` (external disk files only)
- `binding`, `filter`, `wrap`

Runtime behavior:
- Uses FFmpeg decoder
- Advances via time accumulator and stream frame interval
- On EOF: loops when `loop = true`, otherwise holds
- If `optional = true` and open fails, degrades to transparent placeholder

Example:

```toml
[textures.channel1]
type = "video"
source = "assets/loop.mp4"
loop = true
hot_reload = true
binding = 1
```

### 4.3 `font`

Purpose:
- Rasterized font atlas texture (RGBA)

Relevant fields:
- `source` (required)
- `font_size` (default `32.0`)
- `optional`
- `binding`, `filter`, `wrap`

Runtime behavior:
- Rasterizes printable ASCII range into an atlas
- Uploaded once, then unchanged
- If `optional = true` and load/rasterize fails, falls back to built-in `font8x8` atlas

Example:

```toml
[textures.font_atlas]
type = "font"
source = "assets/JetBrainsMono-Regular.ttf"
font_size = 42.0
binding = 2
optional = true
```

### 4.4 `slideshow`

Purpose:
- Cycles through slide entries over time

Relevant fields:
- `sources` (required array)
- `interval` (seconds, default `30.0`)
- `shuffle` (default `false`)
- `optional`
- `hot_reload` (forwarded to child external sources)
- `binding`, `filter`, `wrap`

Runtime behavior:
- Only current slide is loaded (lazy, low memory)
- On timer expiry, current child is dropped and next child is loaded
- Children can be any texture type (including nested shader textures)
- If `optional = true` and slideshow creation fails, degrades to inert placeholder behavior

Example:

```toml
[textures.gallery]
type = "slideshow"
interval = 12.0
shuffle = true
binding = 3

[[textures.gallery.sources]]
type = "image"
source = "assets/slide1.png"

[[textures.gallery.sources]]
type = "video"
source = "assets/loop2.mp4"
```

### 4.5 `audio_spectrum`

Purpose:
- Real-time FFT audio data texture

Relevant fields:
- `source` (`desktop`, `microphone`, or device name; default `desktop`)
- `fft_bands` (default `512`)
- `optional`
- `binding`

Runtime behavior:
- Output format is `R32Float`, dimensions are `fft_bands x 1`
- Always produces fresh frame data (spectrum)
- Audio level is also exposed to uniforms
- If `optional = true` and audio init fails, degrades to silent/zero spectrum behavior

Example:

```toml
[textures.audio]
type = "audio_spectrum"
source = "desktop"
fft_bands = 1024
binding = 4
optional = true
```

### 4.6 `shader`

Purpose:
- Runs a GLSL fragment shader into its own offscreen texture

Relevant fields:
- `shader` (required, path to fragment shader source)
- `width` (default `512`)
- `height` (default `512`)
- `textures` (nested texture map)
- `uniforms` (custom uniform map)
- `optional`
- `binding`, `filter`, `wrap`

Runtime behavior:
- Compiles GLSL to WGSL and builds a dedicated render pipeline
- Can consume any nested texture type, including nested shader textures
- GPU-managed: parent samples child output view directly (zero-copy path)
- If `optional = true` and compile/load fails, degrades to transparent placeholder shader output

Example:

```toml
[textures.pass_a]
type = "shader"
shader = "assets/pass_a.frag"
width = 1024
height = 1024
binding = 0

[textures.pass_a.textures.src]
type = "video"
source = "assets/nebula.mp4"
binding = 0

[textures.pass_a.uniforms.distortion]
type = "float"
min = 0.0
max = 1.0
default = 0.25
```

### 4.7 `noise`

Purpose:
- Procedural RGBA noise texture generated from a seed

Relevant fields:
- `seed` (optional)
- `width` (default `512`)
- `height` (default `512`)
- `binding`, `filter`, `wrap`

Runtime behavior:
- Generated once on load and uploaded once
- If `seed` is omitted, a process-wide default seed is initialized once and reused
- Omitted-seed noise remains stable across repeated loads in the same daemon process

Example:

```toml
[textures.grain]
type = "noise"
width = 1920
height = 1080
binding = 5

[textures.grain_alt]
type = "noise"
seed = 1337
width = 1920
height = 1080
binding = 6
```

## 5. Nesting Rules

Nesting is supported through `type = "shader"` textures.

- Only shader textures expose nested `[...textures.*]`
- Nested entries use the same schema as root textures
- Nested shader textures can themselves contain nested textures
- In practice, nesting depth is constrained by GPU performance and shader complexity

Conceptual hierarchy example:

- Root shader
- Child shader (distortion)
- Grandchild texture inputs (video + audio)

This allows graph-like composition while keeping each shader pass self-contained.

## 6. `input = "t-1"` Previous-Frame Feedback

Kroma supports per-shader feedback via `input = "t-1"` in shader sub-textures.

Important details:

- `input = "t-1"` is only meaningful inside a shader texture's nested textures
- It binds that shader's own previous frame output
- First frame has no history (treated as transparent/empty history)
- History is per shader instance, not shared globally
- `type` is still required by schema, but `input = "t-1"` drives this special binding path

Example:

```toml
[textures.feedback_pass]
type = "shader"
shader = "assets/feedback.frag"
width = 1280
height = 720
binding = 0

[textures.feedback_pass.textures.history]
type = "image"
input = "t-1"
binding = 0

[textures.feedback_pass.textures.base]
type = "image"
source = "assets/base.png"
binding = 1
```

## 7. Merge And Composition Patterns

"Merge" in Kroma is typically done in shader code by sampling multiple channels and combining them (`mix`, `+`, `screen`, masks, etc.).

### Pattern A: Static + Video Merge

Use image as base and video as animated overlay.

```toml
[textures.base]
type = "image"
source = "assets/base.png"
binding = 0

[textures.overlay]
type = "video"
source = "assets/fx.mp4"
loop = true
binding = 1
```

Shader merges channel0 + channel1.

### Pattern B: Audio-Reactive Merge

Sample audio spectrum texture to drive intensity, color, displacement, or bloom.

```toml
[textures.music]
type = "audio_spectrum"
source = "desktop"
fft_bands = 512
binding = 2
```

### Pattern C: Recursive Shader Pipeline

Use one shader texture as a pre-pass, then sample it in a root shader.

- Pass A: generates masks/noise/distortion
- Root: merges base media with Pass A output

### Pattern D: Feedback Trails

Use `input = "t-1"` in a shader texture and blend previous frame into current frame.

- Good for trails, persistence, fluid-like advection, glow accumulation
- Tune decay factor in shader to avoid full burn-in

### Pattern E: Robust External Assets

Use `optional = true` + `hot_reload = true` for live-edit workflows:

- Missing file does not hard-fail render
- Texture shows placeholder until file appears
- File changes reload automatically

## 8. Current Implementation Notes

These are important when authoring advanced packages:

- `filter` and `wrap` are part of schema but currently not applied to sampler creation in the daemon path.
- Hot reload is intended for external disk sources; embedded package assets are immutable at runtime.
- Slideshow children are loaded lazily, so very large collections are memory-efficient.
- Shader textures are GPU-managed and can be nested for zero-copy pass chaining.

## 9. Design Checklist For Complex Texture Graphs

Before shipping a `.shade` package:

- Set explicit `binding` for deterministic channel layout
- Mark non-critical external assets as `optional = true`
- Use `hot_reload = true` for local iteration on external files
- Keep nested shader resolution (`width`, `height`) proportional to visual need
- Prefer two to three passes first, then increase only when required
- For feedback passes, clamp/decay history in shader code to maintain stability
