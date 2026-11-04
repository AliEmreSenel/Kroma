# Initial performance results

Measured on 2026-11-03 against the implementation at `b1ea27b` plus the new
benchmark harness, then against this change. Host: AMD Ryzen 5 5600H (12 logical
CPUs), Linux `7.2.11-arch1-1`, `rustc 1.101.0-nightly (ea137335b 2026-10-05)`.
The benchmark profile uses release optimization and thin LTO with line tables
and symbols retained.

## Focused CPU measurements

Before and after executables ran sequentially after compilation finished.
Criterion used 50 samples, a one-second warmup, and a three-second measurement
period per case. Values below are slope estimates and their 95% confidence
intervals. The last translation run includes deterministic config texture
ordering, which the new regression test exposed as inconsistent with shader
binding order.

| Workload | Before | After | Approximate speedup |
| --- | --- | --- | --- |
| Two-channel translation | 59.75 µs (59.46–60.11) | 5.91 µs (5.86–5.96) | 10.1× |
| Cached system-stat read | 9.73 µs (9.65–9.83) | 10.33 ns (10.30–10.36) | 942× |
| Uncompressed stream, 4 KiB read | 22.25 µs (22.03–22.52) | 18.50 µs (18.44–18.56) | 1.20× |
| Uncompressed stream, 64 KiB read | 27.26 µs (26.55–28.20) | 19.61 µs (19.53–19.70) | 1.39× |

These are isolated CPU-path improvements. Battery polling moves to the
one-second sampler; the stat-read benchmark no longer performs filesystem I/O.
That saves only about 0.58 ms of CPU work per second at 60 reads/second on this
host, despite the large microbenchmark ratio. Translation mainly affects
imports/startup. Stream-read savings depend on video bitrate and demuxer reads.
End-to-end daemon CPU, FPS, and GPU memory improvements require matched desktop
workloads from [the measurement guide](PERFORMANCE.md).

## Allocation evidence

A two-second Heaptrack profile of `stream_read_65536/none` attributed **56,545
temporary chunk allocations** to `decode_chunk_verified` before the change.
The corresponding after trace contains **zero allocations attributed to that
function**. Each read previously allocated/copied the full 256 KiB stored
chunk before copying the requested bytes to FFmpeg's buffer. The new path
borrows the mmap slice after checking its CRC, eliminating that temporary
allocation and copy. Compressed chunks still require decoding storage.

Total allocation calls, including all package-fixture construction, fell from
58,395 to 2,367 in these fixed-duration traces. Iteration counts differ under
instrumentation, so total calls are diagnostic rather than a normalized
allocation rate. Fixture construction dominated peak heap use at about
28.98 MB in both traces; this measurement supports reduced allocation churn,
without establishing a lower steady-state daemon RSS.

Callgrind attributed about 73% of collected instructions to memcpy before the
stream change and about 36% afterward. The checksum became a larger remaining
cost. Both runs completed the selected workload; Valgrind also emitted a brk
segment warning at termination. Treat these instruction profiles as directional
support for removing the copy. Kernel `perf` sampling was denied by this host,
so no sampled CPU-stack comparison is claimed.

The audio worker fix follows from ownership inspection: the old detached FFT
loop had no termination condition, retaining its buffers and waking every
16 ms even after its texture was dropped or initialization failed. Both audio
and system-stat workers now have interruptible waits and are joined on drop.
Worker tests verify that drop releases captured state and interrupts a long
wait. A hardware audio load/unload soak remains useful for measuring total
driver and capture-resource retention.

## Existing desktop session snapshot

The process sampler also observed the already running daemon for ten seconds
without changing its workload:

| Metric | Observed |
| --- | --- |
| Average process CPU | 19.90% of one logical CPU |
| RSS | 75.48 MiB |
| PSS | 61.81 MiB |
| Private resident memory | 57.43 MiB |
| Swap | 62.23 MiB |
| Threads | 10 |
| Major faults during sampling | 0 |

This is an uncontrolled existing process with an unrecorded shader/output
configuration, not a reproducible daemon baseline. CPU, resident memory, and
swap describe different parts of its footprint; GPU memory is additional. The
next high-impact experiment is removing unnecessary intermediate output targets
and their blit pass where lighting capture does not require them. One RGBA8
4K target represents roughly 31.6 MiB per output before driver overhead.

Raw timing logs, estimates, Heaptrack/Callgrind traces, and sampler JSON/CSV are
kept in the ignored `perf-results/` directory for this checkout. Reproduce the
comparison with named Criterion baselines using the commands in
[PERFORMANCE.md](PERFORMANCE.md).

## Follow-up: renderer allocations and retained resources

This pass used the existing dev/test compiler settings and cached dependencies.
No compiler, build-profile, or dependency changes were made. The new ignored
`renderer::monitor_tests::profile_render_loop` workload renders 2,000 frames
after 32 warmup frames to two independent 64×32 outputs. It uses the real wgpu
path and waits for GPU completion every 32 frames. GPU: AMD Radeon Graphics
(RADV RENOIR), Vulkan, Mesa `26.2.5-arch1.1`.

Before/after executables ran alternately three times without Heaptrack. Timing
excludes initialization and warmup. Process CPU seconds were recorded with
`getrusage(RUSAGE_CHILDREN)` and include initialization, warmup, and all process
threads.

| Metric | Before | After | Reduction |
| --- | --- | --- | --- |
| Median frame-loop time | 663.48 µs | 534.67 µs | 19.4% |
| Median process CPU seconds | 1.554 s | 1.287 s | 17.2% |
| Heaptrack allocation calls, whole fixed-work run | 403,379 | 308,587 | 23.5% |
| Allocations attributed to `globals_group` | 81,280 | 37 | >99.9% |
| Allocations attributed to `upload_custom_uniforms` | 18,558 | 8 | >99.9% |

Per-frame timing samples were 663.48/660.69/666.58 µs before and
541.08/533.35/534.67 µs after. Allocation counts come from separate Heaptrack
runs with exactly the same work and include initialization/warmup. Counts
attributed to a function include allocations in its callees. These results
describe a tiny offscreen debug workload, not release desktop FPS.

Implemented changes:

- Cache uniform bind groups per output and role, with bounded storage and
  invalidation when layout/storage-buffer identities change.
- Upload custom uniform values after initialization or edits, retaining
  independent dirty state through transition preflight and lane promotion.
- Release completed transition textures and retired per-monitor shader
  histories during GPU polling even while paused or occluded. Previously the
  cleanup waited for the affected output to render another frame.

The cleanup fixes a retention case rather than changing steady-state target
counts. A completed transition can leave two unnecessary RGBA8 canvas textures
per hidden output until it next renders: about 15.8 MiB at 1080p or 63.3 MiB at
4K, excluding driver overhead. Retired shader histories can add further targets.
GPU tests verify release without a subsequent draw and preserve output colors,
independent feedback histories, live custom edits, buffer replacement, incoming
uniform initialization, and edits after promotion.

Raw artifacts for this pass are `perf-results/render-timings.json`,
`render-heap-{before,after}.zst`, and the filtered `render-bindings-*` and
`render-uploads-*` reports. The reproduction commands use incremental test
builds in [the measurement guide](PERFORMANCE.md#quick-renderer-profiles-with-the-cached-test-build).

## Full-resolution 1080p and 4K comparison

The current profiling workload replaces the 64×32 submission test with the
repository's **animated plasma shader at actual 1920×1080 and 3840×2160 GPU
target sizes**. The earlier 19.4% result describes the small submission
workload; the measurements below describe full-resolution rendering.

Both versions use the identical workload, unchanged cached dev/test compiler
settings, and the same RADV RENOIR GPU/Mesa driver. The baseline renderer is
the implementation before uniform binding reuse, change-driven uniform
uploads, and paused-resource cleanup. Each run warms up 32 frames, then times
600 frames with GPU completion after **every frame**. Five runs per version
per scenario alternate version order: 40 runs and 24,000 measured frames total.

The workload verifies the actual GPU texture dimensions, checks for rendering
errors, and captures downsampled readbacks from the full-resolution targets
outside timing. Initial/final captures prove spatial variation and animation;
before/after checksums matched in every run at each resolution/output count.

Values are medians across the five runs. A frame includes all listed outputs;
for two outputs both render independently at the stated resolution.

| Output configuration | Before mean frame | After mean frame | Frame-time reduction | Before p95 | After p95 |
| --- | --- | --- | --- | --- | --- |
| 1 × 1920×1080 | 0.834 ms | 0.740 ms | 11.2% | 1.560 ms | 1.549 ms |
| 2 × 1920×1080 | 1.232 ms | 1.109 ms | 10.0% | 2.096 ms | 2.027 ms |
| 1 × 3840×2160 | 1.931 ms | 1.755 ms | 9.1% | 2.823 ms | 2.812 ms |
| 2 × 3840×2160 | 3.164 ms | 3.032 ms | 4.2% | 4.143 ms | 3.817 ms |

Process CPU time comes from `/proc/self/stat` deltas over the measured interval
and includes all process threads. Graphics-engine busy time comes from
per-client DRM counter deltas, with duplicate descriptors removed.

| Output configuration | Before CPU / 600 frames | After CPU / 600 frames | Before GPU busy / frame | After GPU busy / frame |
| --- | --- | --- | --- | --- |
| 1 × 1080p | 0.23 s | 0.19 s | 0.423 ms | 0.386 ms |
| 2 × 1080p | 0.40 s | 0.34 s | 0.765 ms | 0.727 ms |
| 1 × 4K | 0.31 s | 0.25 s | 1.296 ms | 1.275 ms |
| 2 × 4K | 0.60 s | 0.51 s | 2.381 ms | 2.359 ms |

CPU ticks have 10 ms granularity on this host. These are descriptive repeated
measurements, without a statistical confidence claim. Some 4K run times overlap:
two-output 4K means ranged 3.019–3.265 ms before and 2.998–3.418 ms after.
The smaller improvement at two-output 4K is consistent with pixel/GPU work
dominating the reduced CPU submission overhead.

Memory snapshots also cover the real target sizes. Median resident GPU GTT
usage was approximately 264.1 MiB for one output and 266.7 MiB for two outputs
at both resolutions, with less than 1 MiB in the VRAM region. Before/after
counters were essentially unchanged. Process RSS varied about 135–202 MiB
between runs of these debug test executables. **This steady-state plasma test
does not demonstrate a resident-memory reduction.** It includes driver buffer
pools and CPU mappings, and integrated-GPU counters must not be added to RSS to
claim a total footprint. The paused-transition cleanup addresses a separate
retention scenario.

These are full-resolution **offscreen rendering** tests, including actual GPU
completion, using the existing debug CPU build. They exclude native surface
acquisition, compositor presentation, and vsync. They measure this plasma shader
on this host; the timings are not a guarantee for other shaders or desktop FPS.

Raw per-run frame statistics, CPU/DRM counters, memory snapshots, and checksums:
`perf-results/render-full-resolution.json`. Aggregate medians:
`perf-results/render-full-resolution-summary.json`. Before/after executable
snapshots use the same full-resolution workload. Reproduce current runs with
the 1080p/4K commands in [the measurement guide](PERFORMANCE.md#quick-renderer-profiles-with-the-cached-test-build).

## Shared scene rendering and smaller GPU allocation pools

This pass removes duplicate full-resolution scene rendering for stateless
multi-output content and requests wgpu's memory-oriented runtime allocation
policy. The baseline includes all earlier binding-cache and uniform-upload
optimizations. Compiler settings, shader code, target resolution, and frame
count remain the same.

Scenes with no output-local shader textures or buffer passes now draw once
into a shared logical canvas. Each output presents and captures independently.
Transitions between eligible scenes also share their two scratch targets.
Active or incoming feedback/history shaders disable sharing and detach the
canvases before rendering. Hotplug, occlusion, incoming feedback, simultaneous
lighting captures, and returning to stateless content have GPU regression
coverage.

The original device allocation policy reserved large backing pools. The
`MemoryHints::MemoryUsage` runtime hint selects smaller pools on supported
backends while continuing to accommodate each resource's required size.
This change is independent of shader compilation and compiler optimization
settings. Some backends may ignore the hint; the measurements below verify its
effect on this host.

The same full-resolution plasma workload ran five alternating repetitions per
version at 600 measured frames each, with GPU completion after every frame.
All 40 before/after runs produced matching initial and final readback checksums.
Values below are medians across repetitions on RADV RENOIR/Mesa 26.2.5.

| Outputs | Before frame time | After frame time | Before resident GPU GTT | After resident GPU GTT |
| --- | --- | --- | --- | --- |
| 1 × 1080p | 0.678 ms | 0.672 ms | 264.1 MiB | 40.1 MiB |
| 2 × 1080p | 1.018 ms | 0.708 ms | 266.7 MiB | 41.4 MiB |
| 1 × 4K | 1.686 ms | 1.649 ms | 264.1 MiB | 56.2 MiB |
| 2 × 4K | 3.024 ms | 1.665 ms | 266.7 MiB | 57.5 MiB |

For dual 4K, frame time fell **44.9%**, process CPU over 600 frames fell from
0.50 s to 0.23 s, and GPU graphics-engine busy time fell from 2.219 ms to
1.137 ms per frame. Resident GPU GTT fell **78.4%**; the reduction was **84.8%**
for single 1080p. Single-output frame time was essentially unchanged. Dual-4K
mean frame times ranged 2.879–3.236 ms before and 1.622–1.737 ms after.

A separate sharing-only comparison kept the original allocator policy. It
reduced dual-4K frame time from 2.930 ms to 1.634 ms and graphics-engine busy
time from 2.227 ms to 1.152 ms. Its resident GTT reduction was only about
1.3 MiB because the larger backing pools remained. This isolates the shared
scene's GPU work reduction from the allocator policy's memory reduction.

Sharing also removes one nominal 7.9 MiB 1080p / 31.6 MiB 4K canvas per
additional stateless output; during eligible transitions, it removes two more
scratch canvases per additional output. Nominal texture bytes and resident
driver memory are different measurements. CPU RSS/PSS varied between runs,
so this report claims the observed GPU GTT reduction rather than an equivalent
drop in CPU RSS or total system RAM. This integrated GPU also reports a small
VRAM region separately; those counters are retained in the raw artifacts.

These remain full-resolution offscreen tests with the existing debug CPU build;
native presentation and compositor/vsync time are excluded. Results describe
this shader and host. Stateful shader histories retain independent rendering,
and more complex/video-heavy packages should be profiled with the same tools.

Raw combined-change runs and medians are
`perf-results/shared-scene/combined-full-resolution.json` and
`perf-results/shared-scene/combined-summary.json`. Sharing-only runs are
`full-resolution.json` and `summary.json` in the same folder. Executable snapshots
and pre-change sources are kept alongside them. The standard profiling commands
in [PERFORMANCE.md](PERFORMANCE.md) exercise the final implementation.

## Deep real-media and lifecycle pass (2026-11-04)

This pass preserves both the workspace's optimized executable before these
changes and the exact installed executable from `/proc/1549/exe`. The live
wallpaper is `041.mp4`: H.264, 1920×1080, 30 FPS, four seconds, on a 1080p
Hyprland output. Read-only sampling found the live process occluded by a
fullscreen workspace. Its idle sample cannot serve as an active-video baseline.
All active comparisons below use private daemon replicas, private sockets and
XDG directories; the live daemon and wallpaper configuration were not changed.

The workspace before/after executables use the same already-cached `profiling`
profile and dependencies. No compiler, flags, build profiles, or toolchain were
changed in this pass. The installed stripped executable has unknown original
compiler settings and is reported separately. GPU/CPU host details remain those
above. Kernel perf sampling/attaching to the live process is restricted on this
host; native Heaptrack and a CPU-only Callgrind capture provide attribution.

### Real workloads, with a 1920×1080 output

Three before/after pairs alternate execution order. Each replica starts with an
empty private application/Mesa cache and runs the same sequence: the actual
1080p wallpaper (six seconds), pause (three), resume (six), actual 4K H.264
`002.mp4` (eight), VP9 WebM (five), a real static WebP, plasma, three shader
lifecycle phases, nested shaders, feedback, and embedded video. Shader/package
rows run for three seconds with three reloads; video rows have two reloads.
Compilation and instrumented runs were finished before these comparisons.

Medians of three process runs follow. CPU is percent of one logical CPU; RSS is
peak sampled resident process memory during that stage. The 4K row in **this**
table decodes 3840×2160 media onto a 1920×1080 output. Actual 4K output is measured
separately below.

| Real source | Before CPU | After CPU | CPU reduction | Before RSS | After RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| Actual 1080p H.264 wallpaper | 30.24% | 18.15% | 40.0% | 213.3 MiB | 194.5 MiB |
| Actual 4K H.264 clip | 63.30% | 47.08% | 25.6% | 356.1 MiB | 289.7 MiB |
| Actual 1080p VP9 clip | 24.98% | 13.58% | 45.6% | 268.8 MiB | 238.2 MiB |
| Embedded 1080p H.264 | 22.94% | 17.62% | 23.2% | 280.9 MiB | 275.0 MiB |

1080p render-loop status remained about 29.7–29.9 FPS; paused CPU had a median
of zero in both versions, as did static-image CPU. Status FPS measures render
iterations, not a count of distinct decoded frames. The 4K source CPU ranged
51.8–67.3% before and 44.6–47.6% after. Interactive-host frequency/driver-cache
variation and short stage durations limit precision; these are observations,
not universal confidence intervals. Mixed-workload memory depends on previous
loads: for example plasma RSS ranged 225.9–318.8 MiB before and 238.1–306.0 MiB
after. Some individual runs retain more memory after video even though the
median falls. RSS, PSS, allocator memory, and DRM resident memory are recorded
separately and must not be summed as independent footprints.

### One-time costs and reloads

Cold-application-cache startup to the render-loop event fell from a median
679.1 ms to 471.5 ms (ranges 562.1–838.1 and 380.4–472.1 ms). Removing the fixed
200 ms system-stat sleep accounts for most of that improvement. Memory and
battery are available immediately; CPU usage becomes meaningful after the
background sampler's first interval. No global OS caches were flushed.

Median reload response latencies include IPC scheduling and render-loop wakeup,
not first visible pixels or complete transition duration:

| Package/source | Before | After |
| --- | ---: | ---: |
| Plasma | 170.9 ms | 17.5 ms |
| Identical shader in load/active/unload phases | 468.5 ms | 17.6 ms |
| Nested shader | 278.1 ms | 18.2 ms |
| Feedback shader | 295.0 ms | 18.0 ms |
| Real 4K H.264 | 163.6 ms | 125.4 ms |
| Real VP9 | 98.3 ms | 63.8 ms |

Shader values aggregate nine reloads per package/version. Successful GLSL→WGSL
translations now use a full-source-keyed LRU cache capped at 16 entries and
2 MiB including keys and outputs. Changed source recompiles, failed compilation
is not cached, and compiler settings remain unchanged. Video pacing now reads
FPS from the loaded worker instead of opening a second codec/scaler/IO context.
Opt-in stage logs separate GPU initialization, builtin validation, compilation,
phase loading, external/embedded opening, and first-frame decode/upload.

### Allocation ownership and GPU upload backpressure

The repeated embedded-video experiment loads the real wallpaper, pauses it,
performs 30 reloads while paused, waits eight seconds, then resumes. Heaptrack
found **4,194,304 bytes in 64 leaked custom AVIO buffers**, plus 17,920 bytes in
64 AVIO contexts, before the fix. There are two opens per load in the old pacing
path. The after trace has **zero leaked AVIO buffers or contexts**. Other
FFmpeg/library exit-retained allocations remain; total process exit-retained
heap fell from about 4.67 MB to 459 KB, so this is not a claim of zero leaks in
all native libraries.

Custom IO now closes the format context, frees FFmpeg's current AVIO buffer
(which FFmpeg can replace), frees the AVIO context, then releases callback state.
Failure cleanup handles FFmpeg freeing/nulling the format context. Regression
tests cover invalid/empty media and repeated valid embedded open/seek/close.

Video uses the existing GPU upload path with a blocking, capacity-one metadata
channel. This bounds worker run-ahead to two frames and stops work when paused.
Uploads are submitted even during paused loads so staging does not accumulate
until a future render. The paused-soak DRM resident counters were unchanged
between the start/end of the measured settled interval. This fixes the earlier
unbounded worker path while removing the FFmpeg-RGBA→Rust-Vec copy. The CPU
fallback retains initialized frame buffers, copies contiguous planes in one
operation, handles row padding, and reuses its native decoded AVFrame. Exhausted
video workers stop instead of waking every five milliseconds.

Shader textures now allocate history only for direct `t-1` channels, upload
immutable custom defaults once, and reuse resource-identity-checked texture bind
groups. The validated shader template moves to the first output instead of
remaining as an extra unused full-resolution instance. Other outputs retain
independent feedback. A stateless 1080p shader texture needs one 7.91 MiB target;
previously one output held four such targets across the template and instance.
Driver pool residency can obscure these resource-count savings in mixed RSS.

### Correctness and remaining CPU costs

Full GPU byte readbacks matched decoded RGBA for actual 1920×1080 H.264,
3840×2160 H.264, 2474×1392 H.264 (row padding), and 1920×1080 VP9 clips. The 4K
check validates all 33,177,600 bytes. Existing GPU tests also preserve independent
feedback histories and verify template transfer and nested binding reuse.

A completed Callgrind run decodes 13 real 4K frames (first frame plus 12 repeated
frames) using the CPU fallback. Within `next_frame_rgba`, the annotated calls
attribute approximately 78% of instructions to packet decoding and 22% to
software scaling. This is an unoptimized test executable and instruction
attribution, not production CPU-time percentages. Whole-harness totals include
expensive debug checksums and initial Vec initialization; they must not be used
as wallpaper hot-path percentages. Hardware decoding is the next substantial
codec experiment; GPU YUV conversion targets the smaller conversion component.
Neither is silently enabled by this change.

Raw artifacts live under `perf-results/deep/`: preserved executables and hashes,
`clean-{before,after}-{0,1,2}`, `clean-installed`, full-resolution output runs,
`heap-{before,after}-soak`, byte checks, Callgrind data, logs and `summary.json`.
The tracked fixture generator and daemon profiler reproduce the scenarios with
user-supplied media; real wallpaper files are not added to the repository.

### Actual 3840×2160 output and the installed daemon

Two additional complete runs use `KROMA_HEADLESS_SIZE=3840x2160`, with the
1920×1080 default explicitly overridden and the requested size checked in logs.
Both start with the real 3840×2160 H.264 clip. Initial playback uses 45.20% and
44.45% CPU at about 29.6 render iterations/second, with sampled peak RSS
284.5/280.2 MiB and PSS 264.5/260.1 MiB. Resume samples use 44.82/44.96% CPU.
Ten-second pauses use 0.20%/0.00% CPU. Plasma renders on the actual 4K canvas at
about 59.6 FPS and 1.66% CPU in both runs. These are paced real-daemon samples,
not unpaced maximum-throughput GPU benchmarks. The earlier full-resolution
renderer tables measure that separate workload.

Cold/warm application-cache startup for these two 4K runs is 318/360 ms; the
warm run being slower illustrates host scheduling/driver variance. Detailed
stage timers distinguish the render-loop readiness event from initial decoding.
Neither this offscreen backend nor the old replica includes Wayland surface
presentation, compositor work, or vsync. The preserved old daemon only supports
its fixed 1080p headless output, so no old-4K-output comparison is claimed.

The separately reproduced installed executable uses 21.97% CPU and 217.8 MiB
RSS on the actual 1080p wallpaper, and 50.20% CPU/355.5 MiB RSS on the 4K source
with a 1080p output. Its 1080p DRM GTT residency is 264.1 MiB, versus 77.4 MiB in
the first current run. This includes improvements from earlier passes and
unknown historical compiler settings; it is a descriptive historical reference,
not the controlled before/after attribution used above.

### Fresh shader memory and real transition handoffs

A fresh-process nested 1080p shader experiment isolates render resources from
video allocator history. End-of-stage DRM GTT residency falls from 140.0 to
74.7 MiB, while RSS is effectively unchanged at about 162 MiB. CPU did not
improve in this individual pair (2.75% before, 3.24–3.48% after); the result
supports reduced GPU residency, not a shader throughput claim.

Separate real handoffs exercise `kroma.fade`, `cross-blur`, `pixelate`,
`wipe-left-right`, and `glitch-datamosh-style`, each with a 0.5-second transition.
Both versions reach the expected incoming active phase, clear the pending path,
finish unload into terminal state, and load video successfully afterward.
Requests use `force=false`, so transitions are actually executed. These single
pairs have request response latencies of 461/174/457/230/1863 ms before and
5/26/10/8/7 ms after, respectively. They primarily demonstrate avoiding repeated
source preparation; response latency excludes full transition completion and
these values are not statistically repeated estimates.

First-use work remains: the cold plasma launch in this extra after run takes
1.10 seconds to render-loop readiness, including 508 ms GPU initialization,
335 ms first GLSL compilation, and 88 ms builtin validation. Cache hits do not
remove first-use compilation or GPU initialization. These are explicit remaining
one-time costs, not hidden inside warmed reload numbers. The extra experiments
are saved in `fresh-nested-{before,after}` and `transitions-{before,after}`.

Final verification: 130 workspace Rust tests pass (three manual profiling tests
remain ignored by default), nine Python profiler tests pass, and workspace
Clippy with `-D warnings`, formatting, and whitespace checks pass. The ignored
real-media decode/GPU-transfer tests were run explicitly for all four clips.
Generated fixtures were imported, packed, and loaded through the real daemon.
A deliberate 4K request to the old fixed-1080p binary was rejected and its private
child process group was cleaned up. Across the retained result sets, 24 isolated
daemon rounds completed; no private replica remained running at the end.
