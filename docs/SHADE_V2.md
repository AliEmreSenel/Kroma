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
2. Legacy ZIP `.shade` is not runtime-loadable.
3. Legacy ZIP can be converted using `kroma migrate`.

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
- `kind` (u8): config/shader/preview/asset
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
- root shader: `auto`
- other entries: `auto`
3. Auto compression accepts compressed output only if improvement is at least:
- 4 KiB, or
- 1% (for chunks >= 64 KiB)

## Determinism

1. Ordered map parsing/serialization is used for config-driven traversal.
2. Referenced files are packed in config declaration order.
3. No filesystem metadata (mtime/uid/gid/perms) is written into v2 package format.

## Migration

`kroma migrate` reads legacy ZIP `.shade` and writes v2 `.shade`.

1. Embedded legacy files are copied into v2 entries.
2. Unresolved required references are warned and preserved as references.
3. Runtime loading remains v2-only.

## Inspect Tooling

Use `kroma inspect` for format introspection and validation:

1. `summary` for package-level overview
2. `verify --mode fast|checksum|decode` for integrity checks
3. `list` for entry table metadata
4. `chunks --depth 1..4` for chunk-level diagnostics
5. `stats` for compression analytics
6. `dump-entry` / `dump-chunk` for byte-level debugging
