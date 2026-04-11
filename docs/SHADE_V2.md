# SHADE V2 Container

This document describes the runtime `.shade` v2 container used by Kroma.

## Goals

1. Keep `.shade` extension.
2. Enforce strict magic/version validation.
3. Allow per-entry independent decoding.
4. Support chunked compression with per-chunk checksums.
5. Support deterministic package creation.

## Runtime Rules

1. Daemon runtime loads v2 `.shade` only.
2. Runtime shader behavior is driven by `config.toml` `states.*` blocks.

## Runtime Config Schema

Runtime config uses fixed phase keys under `[states]`:

1. `[states.load]` optional one-shot phase with `length` seconds.
2. `[states.active]` optional loop phase with `length` as loop period.
3. `[states.unload]` optional one-shot phase with `length` seconds.

Each phase may define a full independent shader structure:

1. `length` (f64 seconds, non-negative)
2. `shader` (optional asset path)
3. `uniforms`
4. `textures`
5. `buffers`

Behavioral rules:

1. `active.length = 0` means unload can interrupt active immediately.
2. `active.length > 0` means unload begins only at first future loop boundary.
3. Phase-local shader time/frame counters reset on phase entry.
4. After the last defined phase completes, daemon holds final rendered frame.

## File Header

Header size: 64 bytes

1. `magic` (8 bytes): `KRMASHD2`
2. `version` (u16 LE): `2`
3. `header_size` (u16 LE): `64`
4. `flags` (u32 LE): reserved, currently `0`
5. `entry_count` (u32 LE)
6. `default_chunk_size` (u32 LE)
7. `index_offset` (u64 LE)
8. `index_size` (u64 LE)
9. `file_size` (u64 LE)
10. reserved padding to 64 bytes

## Entry Index

Index blob starts at `index_offset` and begins with:

1. `entry_count` (u32 LE)
2. Repeated per entry:
- `path_len` (u16 LE)
- `path` (`path_len` bytes, UTF-8)
- `kind` (u8): config/preview/asset
- `default_codec` (u8): none/zstd/lz4
- `default_level` (u16 LE as i16 payload)
- `chunk_size` (u32 LE)
- `uncompressed_size` (u64 LE)
- `chunk_count` (u32 LE)
- `chunk_table_offset` (u64 LE)

## Chunk Table

Each entry has `chunk_count` descriptors at `chunk_table_offset`.

Descriptor size: 24 bytes

1. `data_offset` (u64 LE)
2. `compressed_size` (u32 LE)
3. `uncompressed_size` (u32 LE)
4. `checksum` (u32 LE, CRC32 of stored chunk bytes)
5. `codec` (u8)
6. 3 bytes padding

## Compression Policy

1. Default chunk size: 256 KiB.
2. Default entry behavior:
- image/video: stored (`none`)
- other entries: `auto`
3. Auto compression accepts compressed output only if improvement is at least:
- 4 KiB, or
- 1% (for chunks >= 64 KiB)

## Determinism

1. Ordered map parsing/serialization is used for config-driven traversal.
2. Referenced files are packed in config declaration order.
3. No filesystem metadata (mtime/uid/gid/perms) is written into v2 package format.

## Inspect Tooling

Use `kroma inspect` for format introspection and validation:

1. `summary` for package-level overview
2. `verify --mode fast|checksum|decode` for integrity checks
3. `list` for entry table metadata
4. `summary`/`list` include lifecycle phase metadata (`load|active|unload`, lengths, shader path)
5. `chunks --depth 1..4` for chunk-level diagnostics
6. `stats` for compression analytics
7. `dump-entry` / `dump-chunk` for byte-level debugging
