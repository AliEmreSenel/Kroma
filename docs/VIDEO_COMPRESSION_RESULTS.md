# Video codec and container compression measurements

Measured on 2026-11-04 using the optimized daemon from the runtime profiling
pass, unchanged compiler settings, a Ryzen 5 5600H, and Radeon/RADV graphics.
The two sources are real H.264 wallpapers: `041.mp4` (1920×1080, 120 frames)
and `002.mp4` (3840×2160, 160 frames), both 30 FPS, 8-bit `yuv420p`, limited
range BT.709. Each plays onto an output matching its actual source dimensions.

Container compression preserves the encoded video bytes. It can reduce disk
space and I/O, but FFmpeg must still decode the same video stream. Lossless
codec re-encoding changes that stream and can reduce decoding work while
increasing storage. Neither option is automatically a playback speedup.

## Does wrapping the original video help?

An earlier three-run comparison used identical media bytes, with embedded
previews in the `.shade` package. CPU is percent of one logical CPU; RSS is
resident process memory. The original video was not transcoded.

| Source/output | Raw CPU | `.shade` CPU | Raw RSS | `.shade` RSS | Raw/package startup |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1080p | 15.13% | 14.85% | 201.5 MiB | 203.8 MiB | 285 / 316 ms |
| 4K | 40.33% | 41.60% | 289.5 MiB | 297.5 MiB | 326 / 388 ms |

Wrapping alone gave no consistent CPU benefit and modestly increased startup
and RSS in these runs. Small embedded previews let a chooser avoid opening
the full video; that is a different workload from wallpaper playback.

## Lossless codec candidates

`libx264 -preset ultrafast -tune fastdecode -qp 0` passed the checks below for
both clips. The single-thread encoding jobs took 0.87 seconds at 1080p and
4.09 seconds at 4K. Three isolated direct-file playback runs per variant gave:

| Source/output | Original size | Lossless H.264 size | Original/lossless CPU | Original/lossless RSS |
| --- | ---: | ---: | ---: | ---: |
| 1080p | 2.575 MiB | 3.984 MiB (1.55×) | 15.56 / 13.06% | 202.9 / 187.6 MiB |
| 4K | 7.109 MiB | 15.259 MiB (2.15×) | 40.39 / 38.83% | 290.4 / 239.9 MiB |

That is about 16% and 4% lower CPU, respectively. The smaller 4K CPU difference
is particularly sensitive to host variation. These results describe these two
sources, not all H.264 encoders or wallpapers.

Quality checks compare every decoded frame's native pixel-plane hash and RGBA
hash, frame size, rational presentation timestamp and duration. Width, height,
pixel format, sample aspect ratio, range, matrix, transfer, primaries and field
order also match. Lossless here means preserving the already-decoded source,
including any compression artifacts it already has.

The tested FFV1/NUT candidate preserved YUV hashes but lost color metadata and
changed decoded RGBA. Its files were about 13–15× larger. Ut Video/NUT preserved
RGBA in these clips but lost metadata and grew to 28–46× the original size.
Neither candidate satisfies the full preservation check. These are failures
of the tested recipes/containers, not a claim that those codecs can never
preserve color metadata with another recipe.

## Kroma block compression

Each original and verified lossless H.264 candidate was packaged using `none`,
LZ4, and Zstd level 3. Every embedded video was extracted and compared byte for
byte with its input. Package sizes include the small configuration/container
overhead and omit previews so only the media compression policy varies.

Three process runs per case alternate forward/reverse/forward case order.
Each run measures four seconds active, 0.5 seconds paused, then four seconds
resumed. The table uses the median of six active measurements; paused samples
are excluded. Packing times are medians of seven separate warm-filesystem
invocations, including CLI startup, after playback finished. Applications and
Mesa caches are private per playback process; filesystem cache is warm.

| Source | Blocks | Package | CPU | RSS | Pack time |
| --- | --- | ---: | ---: | ---: | ---: |
| 1080p original | none | 2.576 MiB | 14.23% | 203.2 MiB | 9.3 ms |
| 1080p original | lz4 | 2.390 MiB | 14.60% | 203.0 MiB | 9.0 ms |
| 1080p original | zstd:3 | 2.404 MiB | 14.73% | 205.2 MiB | 10.5 ms |
| 1080p lossless | none | 3.986 MiB | 12.73% | 191.4 MiB | 13.2 ms |
| 1080p lossless | lz4 | 3.802 MiB | 12.73% | 191.4 MiB | 16.2 ms |
| 1080p lossless | zstd:3 | 3.380 MiB | 12.61% | 193.6 MiB | 30.0 ms |
| 4K original | none | 7.111 MiB | 39.58% | 295.0 MiB | 12.7 ms |
| 4K original | lz4 | 6.819 MiB | 40.67% | 294.0 MiB | 13.6 ms |
| 4K original | zstd:3 | 6.867 MiB | 39.82% | 295.0 MiB | 14.3 ms |
| 4K lossless | none | 15.261 MiB | 37.83% | 251.6 MiB | 30.8 ms |
| 4K lossless | lz4 | 13.836 MiB | 38.57% | 249.8 MiB | 50.9 ms |
| 4K lossless | zstd:3 | 11.885 MiB | 38.32% | 251.6 MiB | 116.7 ms |

Size savings below compare each compressed package to its own `none` package:
originals save 7.2%/4.1% with LZ4 and 6.7%/3.4% with Zstd at 1080p/4K.
Lossless H.264 saves 4.6%/9.3% with LZ4 and 15.2%/22.1% with Zstd.
Despite that reduction, Zstd-compressed lossless packages are still 31% and
67% larger than uncompressed original-video packages.

Playback CPU differences between block codecs are small relative to the sample
spread; there is no consistent CPU or RSS improvement. Status remains about
29.4–29.9 FPS. That status counts render iterations, not independently verified
distinct video frames. Startup medians range roughly 255–356 ms and show no
consistent block-codec advantage. These short runs on an interactive host
cannot establish small speedups or cold-disk behavior.

The six 4K cases in a round affected by an unintended additional desktop daemon
and restoration of the existing service were excluded and repeated. Only the
replacement samples enter the table and committed JSON. No compilation,
encoding or allocation profiler ran during the measured playback intervals.

The [machine-readable results](benchmarks/video-compression-2026-11-04.json)
retain input hashes, sizes, encode times, preservation checks, active CPU/RSS
samples, packing samples and startup medians. Raw per-process logs and sampler
data remain external profiling artifacts.

## Reproduction

Use the current `kroma` CLI and a daemon supporting `KROMA_HEADLESS_SIZE`.
The benchmark needs Python and ffprobe. It uses the existing `pack` and
`inspect` commands, so it does not require the media-preview feature.

```bash
python3 tools/benchmark_video_compression.py \
  --cli /path/to/kroma --binary /path/to/kroma-daemon \
  --media /path/to/original-1080p.mp4 --media /path/to/original-4k.mp4 \
  --media /path/to/lossless-1080p.mp4 --media /path/to/lossless-4k.mp4 \
  --output perf-results/video-compression --rounds 3 --pack-rounds 7
```

The script creates fresh artifacts, verifies embedded bytes, chooses each
input's actual dimensions, and launches only private offscreen daemons through
`profile_daemon.py`. It does not create lossless candidates or claim they are
equivalent: supply only candidates that passed a separate quality comparison.
Full-entry verification uses the CLI's JSON/hex dump and temporarily needs
several times the compressed media size in host memory, outside playback.

For the two measured **8-bit, limited-range BT.709, constant-30-FPS** sources,
the tested encoding command was:

```bash
ffmpeg -nostdin -noautorotate -i original.mp4 -map 0:v:0 -map_metadata 0 \
  -an -fps_mode passthrough -c:v libx264 -preset ultrafast -tune fastdecode \
  -qp 0 -pix_fmt yuv420p -threads 1 -color_range tv -colorspace bt709 \
  -color_trc bt709 -color_primaries bt709 -video_track_timescale 30000 \
  lossless-candidate.mp4
```

That recipe selects only the video stream. Preserve the original, inspect the
actual source metadata, and check all frames before using any candidate.
HDR, alpha, other bit depths, variable frame rate, rotation/display metadata,
audio and additional streams were not validated by this experiment. A generic
automatic transcoder would need explicit handling and verification for them.

For a supported source, `framemd5` with `-fps_mode passthrough` and the source
pixel format checks decoded pixel planes; repeat with `-pix_fmt rgba` to catch
color-conversion differences. Compare timestamp/duration values after converting
the reported time base to rational seconds, and compare ffprobe metadata too.
A codec's lossless label or matching YUV hashes alone is insufficient.

## Current choice

Keep original media bytes and uncompressed video entries as the default. LZ4
is a reasonable optional disk-space tradeoff for these originals. Zstd is more
useful for the larger lossless H.264 candidates, with higher packaging cost.
Use verified lossless H.264 only when the measured decode/memory improvement
justifies its larger file. This experiment adds measurement tooling and results;
it does not enable automatic re-encoding or change import defaults.
