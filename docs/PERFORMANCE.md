# Performance measurement

Kroma has CPU-only Criterion benchmarks and a Linux process sampler. Profiling
is opt-in: normal daemon builds do not install an allocator hook or collect
profiling events.

## Benchmarks

```bash
cargo bench -p kroma-shared --bench core -- --save-baseline before
cargo bench -p kroma-daemon --bench hot_paths -- --save-baseline before
# After making a change, on the same host with the same build settings:
cargo bench -p kroma-shared --bench core -- --baseline before
cargo bench -p kroma-daemon --bench hot_paths -- --baseline before
```

The suites cover:

| Benchmark | Workload |
| --- | --- |
| `translate/{plasma,channels,large}` | GLSL translation, including sparse channel bindings and a large source |
| `package_load/{none,lz4,zstd}` | Map and parse an 8 MiB package without eagerly reading its assets |
| `stream_read_{4096,65536}/{none,lz4,zstd}` | Sequential reads into a reused buffer, matching small reads and FFmpeg's 64 KiB AVIO buffer |
| `ipc/load_decode` | Deserialize a load command |
| `data/system_stats` | Read the production data provider on the render thread |
| `render_demand/union` | Combine overlapping output requests from two clients |

Fixtures use deterministic, compressible bytes. Creation and packing happen
outside timing loops. Package and stream results describe a warm filesystem
cache; they do not predict cold disk I/O, video decode cost, GPU time, or daemon
FPS. The data-provider benchmark excludes initialization and includes its
normal background sampler. Translation timings include output allocations
and use warmed regexes. Keep other builds and profilers idle during timing
runs, and repeat close results before drawing conclusions.

Criterion writes estimates and samples beneath the Cargo target directory's
`criterion/` folder. `--save-baseline NAME`, `--baseline NAME`, and a benchmark
name filter work on both suites. For a short smoke check:

```bash
cargo bench -p kroma-shared --bench core -- --test
cargo bench -p kroma-daemon --bench hot_paths -- --test
```

## CPU and process memory

Build an optimized binary retaining line information and symbols:

```bash
cargo build -p kroma-daemon --profile profiling
```

The `profiling` profile inherits release optimizations and thin LTO, retaining
symbols for stack attribution. Normal `release` binaries remain stripped.
The benchmark profile also retains line information. Cargo prints executable
paths; builds honor `CARGO_TARGET_DIR` and Cargo's `build.target-dir` setting.

Run the binary in the desktop session being measured, and sample its PID:

```bash
python3 tools/profile.py --pid PID --warmup 5 --duration 30 \
  --output perf-results/shader-1080p
# Also collect user-space CPU stacks, if the host permits perf events:
python3 tools/profile.py --pid PID --warmup 5 --duration 30 --cpu \
  --output perf-results/shader-1080p-cpu
perf report -i perf-results/shader-1080p-cpu/perf.data
```

Choose a fresh output directory for every run. The sampler attaches read-only
and leaves the daemon running. It saves:

- `metadata.json`: process identity, kernel, architecture, and sampling settings.
- `samples.csv`: CPU time/percentage, RSS, PSS, private resident memory, swap,
  thread counts, and minor/major faults over time.
- `summary.json`: CPU consumption during the measured interval and sampled
  memory/thread peaks. The lifetime RSS high-water mark is labeled separately.
- `perf.data` and `perf.log` when `--cpu` is requested. Stacks use 99 Hz
  user-space `cpu-clock` sampling with DWARF unwinding.

CPU percentage includes all daemon threads: 100% means one fully occupied
logical CPU. PSS apportions shared mappings; RSS counts shared pages in each
process. Private memory sums `Private_Clean` and `Private_Dirty`. PSS/private
fields remain empty/null if access to `smaps_rollup` is denied. Swap is reported
separately. Sampled peaks can miss short-lived allocations. GPU allocations and
native decoder/driver memory require additional accounting; process RSS and
heap size alone cannot describe the wallpaper's total footprint.

If `perf` is missing or blocked, counters are still saved and the command exits
with a diagnostic pointing at `perf.log`. Use the counter-only command on such
hosts. The tool does not change kernel security settings.

## Heap allocation and instruction profiles

Heaptrack tracks allocations in Rust and native libraries. Run the benchmark
executable directly so the trace excludes Cargo and compilation. Get its path
from `cargo bench -p kroma-shared --bench core --no-run`, then:

```bash
core_bench=/absolute/path/printed/by/cargo
heaptrack --record-only --output perf-results/stream-heap "$core_bench" \
  --bench stream_read_65536/none --profile-time 5
heaptrack_print perf-results/stream-heap.zst > perf-results/stream-heap.txt
```

Depending on the installed Heaptrack build the trace may end in `.gz` instead
of `.zst`; use the filename it prints. Criterion's `--profile-time` runs a
selected workload for a fixed duration without statistical analysis. Fixture
setup still appears in the allocation trace; attribute the hot-path allocations
by their call stacks rather than treating total process peak as per-read cost.
Profiler overhead affects throughput, so use regular Criterion runs for timing.

For a daemon heap trace, launch the profiling binary under
`heaptrack --record-only --output perf-results/daemon /path/to/kroma-daemon`,
exercise the workload, and shut it down normally. Use a dedicated desktop
session or stop the existing daemon first to keep IPC socket ownership unique.
Repeat load/unload cycles and inspect whether threads and private memory return
to a stable baseline after warmup. Allocator/driver caches can retain memory;
an RSS plateau alone is insufficient evidence of a leak.

Callgrind works without kernel performance-event access and can attribute
instruction counts to a CPU path:

```bash
valgrind --tool=callgrind --callgrind-out-file=perf-results/stream.callgrind \
  --toggle-collect='*AssetByteStream*read_at*' \
  "$core_bench" --bench stream_read_65536/none --profile-time 5
callgrind_annotate perf-results/stream.callgrind
```

Instruction counts under instrumentation differ from elapsed CPU time. Check
the report contains the intended function; symbol names and inlining can vary
with the compiler. For a wider profile, omit `--toggle-collect` and account for
fixture setup.

## Quick renderer profiles with the cached test build

The ignored GPU workload renders the animated
`examples/shadertoy/plasma.glsl` shader at the requested full resolution,
including real command encoding, queue submissions, and GPU completion. It
defaults to one 1920×1080 output and 600 measured frames. It requires a working
GPU adapter and uses the cached dev/test build without changing compiler flags,
build profiles, or dependencies:

```bash
cargo test -p kroma-daemon profile_render_loop -- \
  --ignored --exact renderer::monitor_tests::profile_render_loop --nocapture
# One full 4K output:
KROMA_PROFILE_WIDTH=3840 KROMA_PROFILE_HEIGHT=2160 \
  cargo test -p kroma-daemon profile_render_loop -- \
  --ignored --exact renderer::monitor_tests::profile_render_loop --nocapture
# Two independent 4K outputs, with more measured frames:
KROMA_PROFILE_WIDTH=3840 KROMA_PROFILE_HEIGHT=2160 \
  KROMA_PROFILE_OUTPUTS=2 KROMA_PROFILE_FRAMES=1200 \
  cargo test -p kroma-daemon profile_render_loop -- \
  --ignored --exact renderer::monitor_tests::profile_render_loop --nocapture
```

The workload asserts that every actual GPU target has the requested dimensions,
warms up 32 frames, and waits for GPU completion after **every frame**. Its
`render_profile_json` line reports mean/p50/p95 frame latency, adapter/driver,
CPU ticks, RSS/PSS, and per-client DRM resident memory and graphics-engine busy
time when the driver exposes those counters. DRM clients are deduplicated by
driver, device, and client ID. Compare graphics-engine counter deltas over the
measurement interval and divide by frame count; memory is an instantaneous
snapshot taken before/after timing, not a peak. CPU tick deltas convert to
seconds using `getconf CLK_TCK`.

Before/after downsampled readbacks verify that the full-resolution plasma scene
contains varying pixels and animates. Compare their checksums between versions
using the same resolution, frame count, and output count. All outputs share the
logical canvas size and have independent uniform buffers. Stateless scenes
render once into a shared canvas; scenes containing output-local shader textures
or buffer passes keep private canvases and histories. Lighting readbacks remain
independent per output. No live wallpaper or IPC socket is involved.

For allocation attribution, pass the executable path printed by Cargo directly
to Heaptrack after building once:

```bash
renderer_test=/absolute/test/executable/printed/by/cargo
heaptrack --record-only --output perf-results/render-heap "$renderer_test" \
  --ignored --exact renderer::monitor_tests::profile_render_loop --nocapture
heaptrack_print perf-results/render-heap.zst --filter-bt-function globals_group
heaptrack_print perf-results/render-heap.zst --filter-bt-function upload_custom_uniforms
```

Heap profiles include initialization and warmup. Keep shader, resolution,
output count, and frame count identical
for comparisons. Time the executable without Heaptrack, alternate before/after
runs, and compare several repetitions. These full-resolution offscreen timings
include CPU submission and GPU frame completion. They use a debug CPU build and
exclude compositor presentation/vsync; report that scope when comparing them
with release desktop behavior. Integrated-GPU memory can overlap process/system
memory, so do not add RSS and DRM counters to claim a total footprint.

## Representative daemon workloads

Record the commit, toolchain, GPU/driver, monitor sizes/count, target FPS,
package, source codec/FPS, and active lighting/preview consumers with each run.
Use identical settings before and after a change. Measure startup separately
from warmed steady state.

| Scenario | What to check |
| --- | --- |
| Fallback and a static image | CPU and wakeups after the first frame |
| Animated shader at 30 and 60 FPS | CPU per second and achieved FPS |
| Plain 1080p/4K video | Decode/upload CPU, RSS/PSS, source-rate pacing |
| Audio-reactive shader | FFT CPU and thread lifetime after unload/replacement |
| Repeated load/transition/unload | Heap peaks and retained resources after each cycle |
| Paused/fullscreen/inactive | Rendering suppression and background worker cost |
| Multiple monitors | Per-output render-target cost and CPU scaling |
| OpenRGB/external demand enabled | Readback, extra rendering, and worker cost |

For RGBA8 targets, one 1920×1080 image occupies about 7.9 MiB and one 3840×2160
image about 31.6 MiB before alignment/driver overhead. Stateless scenes use one
intermediate final-output target across all monitors, and a transition adds
two shared scene targets temporarily. Scenes with output-local shader histories
use those targets per monitor. Count these separately from decoded video buffers
and surface swapchain images; driver pools can keep backing allocations reserved
after individual textures are released.

## Changes and next experiments

This change removes per-frame sysfs reads by publishing one-second system-stat
snapshots, borrows verified mmap bytes for uncompressed chunks, and uses a
single cached regex pass for texture-channel translation. Periodic workers are
owned and joined: dropping/replacing an audio texture or failing audio setup
releases its FFT buffers and thread, and dropping a system provider stops its
sampler. FFT scratch storage follows the plan's actual required length.

The renderer also reuses up to three uniform bind groups per monitor (outgoing,
incoming, and transition composite), rebuilding them when the layout or custom
storage buffer changes. Custom values upload only after initialization or an
IPC edit; each transition lane preserves its own upload state. Paused/occluded
outputs release retired shader histories, completed transition scratch targets,
and stale cached bindings during the regular GPU poll instead of waiting for a
new frame. GPU readback tests cover edits, buffer replacement, incoming-lane
uploads/promotion, and cleanup without rendering another frame.

Stateless multi-output rendering also shares the final logical canvas and
transition scratch targets. The scene draws once per frame when at least one
output is renderable; every visible output still presents independently and
has its own lighting readback. Active or incoming shader textures/buffer passes
disable sharing before their per-output histories advance. Hotplug and returning
to stateless content rejoin the shared canvas, including during paused cleanup.

The GPU device requests `MemoryHints::MemoryUsage`. This is a runtime allocator
policy: resources can still grow to their required sizes, while supported
backends start with smaller backing blocks instead of the default performance
pools. The 1080p/4K reports include per-client resident GPU counters so allocator
changes are checked against actual resident memory as well as frame time.

Further experiments, in priority order:

1. **Output target and blit cost.** Compare direct presentation when no lighting
   capture is needed against the current intermediate target. At 4K this could
   save one roughly 31.6 MiB target per output and a full-screen pass. Preserve
   exact lighting/transition output and validate across surface formats.
2. **Remaining GPU object creation.** Profile monitor texture bind groups and
   nested shader texture bindings. The monitor globals cache already removes
   repeated uniform bind-group creation. Texture caches must detect changed
   views, especially feedback ping-pong, video replacement, and hot reload.
3. **Compressed streaming.** Compare a bounded one-chunk decoded cache against
   repeated decompression for small reads. Budget roughly 256 KiB per stream;
   the default uncompressed video path already avoids the temporary chunk copy.
4. **Audio work.** Precompute logarithmic FFT band boundaries and measure
   capture/FFT cost while outputs are paused. Preserve frequency mapping and
   avoid changing playback behavior without lifecycle tests.
5. **Video conversion.** Attribute software decoding and RGBA conversion
   separately before adding GPU YUV conversion or hardware decoder backends.
   Preserve color, source timing, and bounded uploads when evaluating either.

See [the initial measurements](PERFORMANCE_RESULTS.md) for the measured scope
and host-specific results.

## Isolated real-daemon and one-time profiling

`tools/profile_daemon.py` launches a real daemon with private XDG config,
cache, and runtime directories and a private IPC socket. It selects Kroma's
KDE offscreen backend without connecting to the desktop. Existing wallpaper,
settings, and IPC clients are unaffected. The default output is 1920×1080;
`--size 3840x2160` creates an actual 4K output. The tool checks the startup log
for the requested size, so older binaries cannot silently report a 1080p
output as 4K.

Use existing executable paths; neither script changes compiler settings:

```bash
python3 tools/profile_daemon.py --binary /path/to/kroma-daemon \
  --media /path/to/real-wallpaper.mp4 --output perf-results/real-video \
  --duration 10 --pause-duration 10 --rounds 2
python3 tools/profile_daemon.py --binary /path/to/kroma-daemon \
  --media /path/to/real-4k-video.mp4 --size 3840x2160 \
  --output perf-results/real-video-4k --duration 10 --rounds 2
```

Each round measures active, paused, and resumed playback. The first round has
an empty application/Mesa cache; subsequent rounds reuse it. This does **not**
flush the OS page cache. Readiness is the render-loop log event, not merely a
created socket, and does not imply that the first video frame is ready.
`video.first_frame` records the initial decode/upload duration separately.

`results.json` contains per-stage CPU/RSS/PSS/private-memory samples,
per-thread CPU deltas, deduplicated DRM resident-memory/engine counters,
status/FPS, resident memory grouped by mapping, startup latency, load/reload
response latency, and optional stage timings. CPU percentage is relative to one logical CPU. Status FPS comes from
the daemon's rolling window; use several-second samples for settled values.
Load/reload response timings include IPC/render-loop scheduling and are not a
measurement of first visible pixels or completed transitions.

Create modern v2 packages for repeated shader compilation, three lifecycle
phases using the same source, nested rendering, independent feedback, and an
embedded copy of real video:

```bash
python3 tools/make_profile_fixtures.py --cli /path/to/kroma \
  --video /path/to/real-wallpaper.mp4 --output perf-results/fixtures
python3 tools/profile_daemon.py --binary /path/to/kroma-daemon \
  --media /path/to/real-wallpaper.mp4 \
  --sequence perf-results/fixtures/sequence.json \
  --output perf-results/lifecycle --rounds 2
```

A sequence is a JSON list. A row can load a path, specify `seconds`, `reloads`,
and a transition spec such as `kroma.fade:0.5`, or send a command such as Pause,
Resume, or UnloadShade:

```json
[
  {"label":"4k-video","path":"/path/to/4k.mp4","seconds":10,"reloads":3},
  {"label":"pause","command":{"type":"Pause"},"seconds":10},
  {"label":"paused-reloads","path":"/path/to/embedded.shade","seconds":2,"reloads":30},
  {"label":"settled","command":{"type":"Pause"},"seconds":10},
  {"label":"resume","command":{"type":"Resume"},"seconds":5}
]
```

Add `--heaptrack` for native allocation and leak attribution. Instrumented
runs are separate from latency/CPU comparisons. In particular, check both
process memory and DRM memory after paused reloads; queued GPU uploads can
retain memory outside the Rust heap.

```bash
heaptrack_print perf-results/lifecycle/heap-0.zst \
  --filter-bt-function open_from_stream --print-leaks 1
```

For an existing Cargo test executable, the ignored tests
`textures::video::tests::profile_decode` and `profile_gpu_transfer` accept
`KROMA_VIDEO_PROFILE_PATH=/path/to/video` and optional
`KROMA_VIDEO_PROFILE_FRAMES=120`. Run with `--ignored --exact` and `--nocapture`.
The decoder test separates opening, first-frame decode, and fixed-work decode;
the GPU test verifies every uploaded RGBA byte via GPU readback, including row
padding. The decode test also works under Callgrind without a compositor or
GPU driver:

```bash
KROMA_VIDEO_PROFILE_PATH=/path/to/4k.mp4 KROMA_VIDEO_PROFILE_FRAMES=12 \
  valgrind --tool=callgrind --callgrind-out-file=perf-results/decode.callgrind \
  /path/to/test-executable --ignored --exact \
  textures::video::tests::profile_decode --nocapture
```

`KROMA_PROFILE_TIMINGS=1 RUST_LOG=info,kroma_profile=info` enables stage logs
for normal launches too. Disabled timing scopes avoid clock reads and logs.
Timings cover startup, GPU/system-stat initialization, transition validation,
package preparation, shader compilation/cache hits, phase/texture loading,
external/embedded video opening, decoder creation, and first-frame work.

## Tool references

- [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html)
- [Criterion CLI and baseline options](https://bheisler.github.io/criterion.rs/book/user_guide/command_line_options.html)
- [Linux procfs memory and CPU counters](https://www.kernel.org/doc/html/latest/filesystems/proc.html)
- [Linux DRM client usage counters](https://www.kernel.org/doc/html/latest/gpu/drm-usage-stats.html)
- [Heaptrack](https://github.com/KDE/heaptrack)
- [wgpu memory allocation hints](https://docs.rs/wgpu/latest/wgpu/enum.MemoryHints.html)

Validate the sampler with `python3 -m unittest discover -s tools -p 'test_*.py'`.
