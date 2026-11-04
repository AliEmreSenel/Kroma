# Media packages and genuine Kroma previews

A v2 `.shade` can contain its original media, a thumbnail (`preview.jpg`,
`preview.png`, or `preview.webp`), and a hover video (`preview.mp4` or
`preview.webm`). Previews are independent of runtime state textures. Playback
loads the small still thumbnail but does not read/decode a preview video unless
an application explicitly requests it. `kroma pack` includes preview files
placed next to `config.toml`.

## Importing media

```bash
kroma media import wallpaper.mp4 --output wallpaper.shade
kroma media import background.webp --output background.shade
kroma media info wallpaper.shade
```

Imports embed the **original bytes**, without transcoding or compressing the
already-encoded media again. They generate a JPEG poster and, for video, a
three-second 640×360 hover clip at 12 FPS. Still images get a poster only.
`--no-video-preview` makes a video package poster-only. Existing destinations
are protected unless `--force` is supplied; package publication is atomic.
Original source files are left intact.

The default preview generator runs **Kroma itself**. Set `KROMA_DAEMON` to the
updated daemon executable if it is not on PATH. FFmpeg and ffprobe executables
are required for media probing and video encoding. The ordinary wallpaper
renderer still uses the FFmpeg libraries directly.

Explicit `--thumbnail PATH` and `--video-preview PATH` accept supplied previews.
`--media-preview` is an opt-in shortcut for plain media using FFmpeg instead of
Kroma; it cannot represent package shaders, effects, feedback, or composition.

## Rendering shader and composed-package previews

```bash
kroma media render-preview scene.shade --embed
kroma media render-preview scene.shade --thumbnail poster.jpg --video hover.mp4
kroma media render-preview scene.shade --video hover.mp4 --seconds 3 --fps 12 \
  --width 640 --height 360 --render-width 1920 --render-height 1080
```

The preview job creates a private XDG runtime/config/cache environment and an
isolated headless output. It uses the daemon's normal package loader, lifecycle
advancement, uniform/data updates, video decoding, nested shader textures,
feedback history, buffer passes, and final composition. It renders a full
1920×1080 canvas by default and downsamples the **final rendered output** for
capture. Output size and logical render size can be chosen separately.

The thumbnail prefers the active phase; video includes the configured load-to-
active progression. Unload occurs only when the normal lifecycle calls for it;
the preview does not force an unload of an indefinitely active wallpaper.
Feedback advances at the daemon's rendering rate. Lower source/render FPS is
represented with duplicate capture frames, preserving preview duration.
Headless cursor input is neutral; audio/system-dependent content uses available
feeds. This is an actual rendering of the package at capture time, not a
promise that time, audio, randomness, or cursor-dependent output will always
match another desktop session.

`--embed` atomically updates the package with rendered previews and a small
`preview.json` provenance record. Without it, exports do not modify the package.
The job never uses the live daemon's socket or changes the wallpaper. It has a
bounded duration and GPU readback wait. The CLI supervises it with a timeout.

## Fast chooser integration

```bash
kroma media info scene.shade
kroma media preview scene.shade --thumbnail /path/to/cache/poster.jpg
kroma media preview scene.shade --video /path/to/cache/hover.mp4
```

`info` returns JSON including media kind, dimensions, source frame rate,
container size, preview availability, and preview provenance. It does not
extract the main media or the hover clip. Cache exports by package modification
time. Hover playback should use the extracted small preview instead of the
full-resolution embedded source. Cache naming must distinguish paths and use
proper file URLs for filenames containing `%`, spaces, or other special bytes.

`media.json` records imported source metadata; `preview.json` records how a
preview was made. These optional entries keep v2 binary compatibility. Older
shader packages without these entries can still be inspected and have genuine
previews generated with `render-preview`.

## Validation

Unit tests cover packing/extracting previews, overwrite protection, and genuine
full-color GPU capture with padded rows. The opt-in integration suite checks
real shader lifecycle colors, low-FPS capture duration, evolving nested feedback,
and byte-for-byte preservation of imported media:

```bash
KROMA_TEST_CLI=/path/to/kroma KROMA_TEST_DAEMON=/path/to/kroma-daemon \
  python3 -m unittest discover -s tools -p test_media_previews.py
```
