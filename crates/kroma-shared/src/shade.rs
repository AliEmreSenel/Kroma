//! `.shade` package support — v2 chunked container format.

use std::collections::{HashMap, HashSet};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::types::ShadeConfig;

const SHADE_MAGIC: [u8; 8] = *b"KRMASHD2";
const SHADE_VERSION: u16 = 2;
const HEADER_SIZE: usize = 64;
const DEFAULT_CHUNK_SIZE: u32 = 256 * 1024;
const AUTO_MIN_REDUCTION_BYTES: usize = 4 * 1024;
const AUTO_MIN_REDUCTION_PERCENT: f64 = 0.01;
const AUTO_RATIO_MIN_INPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct AssetEntry {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[repr(u8)]
pub enum CompressionCodec {
    None = 0,
    Zstd = 1,
    Lz4 = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadeEntryKind {
    ConfigToml,
    Preview,
    Asset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    Fast,
    Checksum,
    Decode,
}

#[derive(Debug, Clone, Serialize)]
pub struct PackageMetadata {
    pub entry_count: u32,
    pub index_offset: u64,
    pub index_size: u64,
    pub file_size: u64,
    pub total_chunks: u64,
    pub total_uncompressed: u64,
    pub total_compressed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkDebugInfo {
    pub index: u32,
    pub data_offset: u64,
    pub compressed_size: u32,
    pub uncompressed_size: u32,
    pub checksum: u32,
    pub codec: CompressionCodec,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntryDebugInfo {
    pub path: String,
    pub kind: ShadeEntryKind,
    pub default_codec: CompressionCodec,
    pub default_level: i16,
    pub chunk_size: u32,
    pub chunk_count: u32,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub compression_ratio: f64,
    pub data_start_offset: u64,
    pub data_end_offset: u64,
    pub chunks: Vec<ChunkDebugInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodecCompressionStat {
    pub codec: CompressionCodec,
    pub chunk_count: u64,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub ratio: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkSizeBucket {
    pub label: String,
    pub chunk_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompressionStats {
    pub total_uncompressed: u64,
    pub total_compressed: u64,
    pub compression_ratio: f64,
    pub savings_vs_stored_bytes: i64,
    pub by_codec: Vec<CodecCompressionStat>,
    pub largest_entries: Vec<EntryDebugInfo>,
    pub chunk_size_distribution: Vec<ChunkSizeBucket>,
    pub average_chunk_entropy_bits_per_byte: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyFailure {
    pub path: String,
    pub chunk_index: Option<u32>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    pub mode: VerifyMode,
    pub total_entries: u32,
    pub total_chunks: u64,
    pub checked_chunks: u64,
    pub ok: bool,
    pub failures: Vec<VerifyFailure>,
}

impl CompressionCodec {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::None),
            1 => Some(Self::Zstd),
            2 => Some(Self::Lz4),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressionPolicy {
    Auto,
    None,
    Zstd { level: i32 },
    Lz4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum EntryKind {
    ConfigToml = 0,
    Preview = 2,
    Asset = 3,
}

impl EntryKind {
    fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::ConfigToml),
            2 => Some(Self::Preview),
            3 => Some(Self::Asset),
            _ => None,
        }
    }
}

impl From<EntryKind> for ShadeEntryKind {
    fn from(value: EntryKind) -> Self {
        match value {
            EntryKind::ConfigToml => ShadeEntryKind::ConfigToml,
            EntryKind::Preview => ShadeEntryKind::Preview,
            EntryKind::Asset => ShadeEntryKind::Asset,
        }
    }
}

#[derive(Debug, Clone)]
struct Header {
    entry_count: u32,
    index_offset: u64,
    index_size: u64,
    file_size: u64,
}

#[derive(Debug, Clone)]
struct V2EntryIndex {
    path: String,
    kind: EntryKind,
    default_codec: CompressionCodec,
    default_level: i16,
    chunk_size: u32,
    uncompressed_size: u64,
    chunk_count: u32,
    chunk_table_offset: u64,
}

#[derive(Debug, Clone)]
struct ChunkDescriptor {
    data_offset: u64,
    compressed_size: u32,
    uncompressed_size: u32,
    checksum: u32,
    codec: CompressionCodec,
}

fn safe_ratio(num: u64, den: u64) -> f64 {
    if den == 0 {
        1.0
    } else {
        num as f64 / den as f64
    }
}

fn shannon_entropy_bits_per_byte(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }

    let mut counts = [0usize; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }

    let len = bytes.len() as f64;
    let mut entropy = 0.0f64;
    for count in counts {
        if count == 0 {
            continue;
        }
        let p = count as f64 / len;
        entropy -= p * p.log2();
    }
    entropy
}

fn read_u16(data: &[u8], offset: &mut usize) -> Result<u16> {
    let end = *offset + 2;
    if end > data.len() {
        anyhow::bail!("Unexpected EOF while reading u16");
    }
    let out = u16::from_le_bytes([data[*offset], data[*offset + 1]]);
    *offset = end;
    Ok(out)
}

fn read_u32(data: &[u8], offset: &mut usize) -> Result<u32> {
    let end = *offset + 4;
    if end > data.len() {
        anyhow::bail!("Unexpected EOF while reading u32");
    }
    let out = u32::from_le_bytes([
        data[*offset],
        data[*offset + 1],
        data[*offset + 2],
        data[*offset + 3],
    ]);
    *offset = end;
    Ok(out)
}

fn read_u64(data: &[u8], offset: &mut usize) -> Result<u64> {
    let end = *offset + 8;
    if end > data.len() {
        anyhow::bail!("Unexpected EOF while reading u64");
    }
    let out = u64::from_le_bytes([
        data[*offset],
        data[*offset + 1],
        data[*offset + 2],
        data[*offset + 3],
        data[*offset + 4],
        data[*offset + 5],
        data[*offset + 6],
        data[*offset + 7],
    ]);
    *offset = end;
    Ok(out)
}

fn write_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn parse_header(bytes: &[u8]) -> Result<Header> {
    if bytes.len() < HEADER_SIZE {
        anyhow::bail!("Invalid .shade file: header too small");
    }

    if bytes[0..8] != SHADE_MAGIC {
        anyhow::bail!("Unsupported .shade format (bad magic)");
    }

    let mut off = 8usize;
    let version = read_u16(bytes, &mut off)?;
    if version != SHADE_VERSION {
        anyhow::bail!(
            "Unsupported .shade version {} (expected {})",
            version,
            SHADE_VERSION
        );
    }

    let header_size = read_u16(bytes, &mut off)?;
    if header_size as usize != HEADER_SIZE {
        anyhow::bail!(
            "Invalid .shade header size {} (expected {})",
            header_size,
            HEADER_SIZE
        );
    }

    let _flags = read_u32(bytes, &mut off)?;
    let entry_count = read_u32(bytes, &mut off)?;
    let _default_chunk_size = read_u32(bytes, &mut off)?;
    let index_offset = read_u64(bytes, &mut off)?;
    let index_size = read_u64(bytes, &mut off)?;
    let file_size = read_u64(bytes, &mut off)?;

    if file_size != bytes.len() as u64 {
        anyhow::bail!(
            "Invalid .shade file size in header ({} vs actual {})",
            file_size,
            bytes.len()
        );
    }
    if index_offset + index_size > file_size {
        anyhow::bail!("Invalid .shade index range");
    }

    Ok(Header {
        entry_count,
        index_offset,
        index_size,
        file_size,
    })
}

fn parse_index(bytes: &[u8], header: &Header) -> Result<Vec<V2EntryIndex>> {
    let start = header.index_offset as usize;
    let end = (header.index_offset + header.index_size) as usize;
    let blob = &bytes[start..end];

    let mut off = 0usize;
    let count = read_u32(blob, &mut off)?;
    if count != header.entry_count {
        anyhow::bail!(
            "Invalid .shade index entry count (header {} vs index {})",
            header.entry_count,
            count
        );
    }

    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let path_len = read_u16(blob, &mut off)? as usize;
        if off + path_len > blob.len() {
            anyhow::bail!("Invalid .shade index path length");
        }
        let path = String::from_utf8(blob[off..off + path_len].to_vec())
            .with_context(|| "Invalid UTF-8 path in .shade index")?;
        off += path_len;

        if off >= blob.len() {
            anyhow::bail!("Invalid .shade index entry kind");
        }
        let kind = EntryKind::from_u8(blob[off]).with_context(|| "Invalid entry kind")?;
        off += 1;

        if off >= blob.len() {
            anyhow::bail!("Invalid .shade index codec");
        }
        let default_codec =
            CompressionCodec::from_u8(blob[off]).with_context(|| "Invalid codec in index")?;
        off += 1;

        let default_level = read_u16(blob, &mut off)? as i16;
        let chunk_size = read_u32(blob, &mut off)?;
        let uncompressed_size = read_u64(blob, &mut off)?;
        let chunk_count = read_u32(blob, &mut off)?;
        let chunk_table_offset = read_u64(blob, &mut off)?;

        out.push(V2EntryIndex {
            path,
            kind,
            default_codec,
            default_level,
            chunk_size,
            uncompressed_size,
            chunk_count,
            chunk_table_offset,
        });
    }

    Ok(out)
}

fn parse_chunk_table(bytes: &[u8], entry: &V2EntryIndex) -> Result<Vec<ChunkDescriptor>> {
    let mut out = Vec::with_capacity(entry.chunk_count as usize);
    let mut off = entry.chunk_table_offset as usize;

    for _ in 0..entry.chunk_count {
        if off + 24 > bytes.len() {
            anyhow::bail!("Invalid chunk table offset for '{}'", entry.path);
        }

        let mut local_off = off;
        let data_offset = read_u64(bytes, &mut local_off)?;
        let compressed_size = read_u32(bytes, &mut local_off)?;
        let uncompressed_size = read_u32(bytes, &mut local_off)?;
        let checksum = read_u32(bytes, &mut local_off)?;

        if local_off >= bytes.len() {
            anyhow::bail!("Invalid chunk codec for '{}'", entry.path);
        }
        let codec = CompressionCodec::from_u8(bytes[local_off])
            .with_context(|| format!("Invalid chunk codec for '{}'", entry.path))?;
        local_off += 1;
        local_off += 3;

        if data_offset + compressed_size as u64 > bytes.len() as u64 {
            anyhow::bail!("Chunk data out of bounds for '{}'", entry.path);
        }

        out.push(ChunkDescriptor {
            data_offset,
            compressed_size,
            uncompressed_size,
            checksum,
            codec,
        });

        off = local_off;
    }

    Ok(out)
}

fn encode_with_codec(codec: CompressionCodec, level: i32, data: &[u8]) -> Result<Vec<u8>> {
    match codec {
        CompressionCodec::None => Ok(data.to_vec()),
        CompressionCodec::Zstd => {
            let encoded = zstd::stream::encode_all(data, level)
                .with_context(|| "Failed to encode zstd chunk")?;
            Ok(encoded)
        }
        CompressionCodec::Lz4 => Ok(lz4_flex::compress_prepend_size(data)),
    }
}

fn decode_with_codec(
    codec: CompressionCodec,
    bytes: &[u8],
    expected_size: usize,
) -> Result<Vec<u8>> {
    match codec {
        CompressionCodec::None => Ok(bytes.to_vec()),
        CompressionCodec::Zstd => {
            let decoded =
                zstd::stream::decode_all(bytes).with_context(|| "Failed to decode zstd chunk")?;
            if decoded.len() != expected_size {
                anyhow::bail!(
                    "Decoded zstd chunk size mismatch (got {}, expected {})",
                    decoded.len(),
                    expected_size
                );
            }
            Ok(decoded)
        }
        CompressionCodec::Lz4 => {
            let decoded = lz4_flex::decompress_size_prepended(bytes)
                .with_context(|| "Failed to decode lz4 chunk")?;
            if decoded.len() != expected_size {
                anyhow::bail!(
                    "Decoded lz4 chunk size mismatch (got {}, expected {})",
                    decoded.len(),
                    expected_size
                );
            }
            Ok(decoded)
        }
    }
}

fn decode_chunk_verified(bytes: &[u8], chunk: &ChunkDescriptor) -> Result<Vec<u8>> {
    let start = chunk.data_offset as usize;
    let end = start + chunk.compressed_size as usize;
    let chunk_bytes = &bytes[start..end];

    let mut hasher = crc32fast::Hasher::new();
    hasher.update(chunk_bytes);
    let checksum = hasher.finalize();
    if checksum != chunk.checksum {
        anyhow::bail!("Chunk checksum mismatch: data corruption detected");
    }

    decode_with_codec(chunk.codec, chunk_bytes, chunk.uncompressed_size as usize)
}

fn auto_select_codec(data: &[u8]) -> Result<(CompressionCodec, i16, Vec<u8>)> {
    let zstd_level = 3;
    let zstd_bytes = encode_with_codec(CompressionCodec::Zstd, zstd_level, data)?;
    let lz4_bytes = encode_with_codec(CompressionCodec::Lz4, 0, data)?;

    let mut best_codec = CompressionCodec::None;
    let mut best_level = 0i16;
    let mut best_bytes = data.to_vec();

    if zstd_bytes.len() < best_bytes.len() {
        best_codec = CompressionCodec::Zstd;
        best_level = zstd_level as i16;
        best_bytes = zstd_bytes;
    }
    if lz4_bytes.len() < best_bytes.len() {
        best_codec = CompressionCodec::Lz4;
        best_level = 0;
        best_bytes = lz4_bytes;
    }

    let reduction_bytes = data.len().saturating_sub(best_bytes.len());
    let reduction_ratio = if data.is_empty() {
        0.0
    } else {
        reduction_bytes as f64 / data.len() as f64
    };

    let worth_compressing = reduction_bytes >= AUTO_MIN_REDUCTION_BYTES
        || (data.len() >= AUTO_RATIO_MIN_INPUT_BYTES
            && reduction_ratio >= AUTO_MIN_REDUCTION_PERCENT);

    if worth_compressing {
        Ok((best_codec, best_level, best_bytes))
    } else {
        Ok((CompressionCodec::None, 0, data.to_vec()))
    }
}

fn is_image_or_video(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());

    matches!(
        ext.as_deref(),
        Some("png")
            | Some("jpg")
            | Some("jpeg")
            | Some("webp")
            | Some("gif")
            | Some("bmp")
            | Some("avif")
            | Some("tif")
            | Some("tiff")
            | Some("mp4")
            | Some("webm")
            | Some("mkv")
            | Some("avi")
            | Some("mov")
            | Some("m4v")
            | Some("mpg")
            | Some("mpeg")
            | Some("wmv")
    )
}

fn default_policy_for_path(path: &str) -> CompressionPolicy {
    if is_image_or_video(path) {
        return CompressionPolicy::None;
    }
    CompressionPolicy::Auto
}

fn write_header(file: &mut std::fs::File, header: &Header) -> Result<()> {
    let mut buf = Vec::with_capacity(HEADER_SIZE);
    buf.extend_from_slice(&SHADE_MAGIC);
    write_u16(&mut buf, SHADE_VERSION);
    write_u16(&mut buf, HEADER_SIZE as u16);
    write_u32(&mut buf, 0);
    write_u32(&mut buf, header.entry_count);
    write_u32(&mut buf, DEFAULT_CHUNK_SIZE);
    write_u64(&mut buf, header.index_offset);
    write_u64(&mut buf, header.index_size);
    write_u64(&mut buf, header.file_size);
    while buf.len() < HEADER_SIZE {
        buf.push(0);
    }

    file.seek(SeekFrom::Start(0))?;
    file.write_all(&buf)?;
    Ok(())
}

fn join_chunks(bytes: &[u8], entry: &V2EntryIndex) -> Result<Vec<u8>> {
    let chunks = parse_chunk_table(bytes, entry)?;
    let mut out = Vec::with_capacity(entry.uncompressed_size as usize);

    for chunk in chunks {
        out.extend_from_slice(&decode_chunk_verified(bytes, &chunk)?);
    }

    if out.len() != entry.uncompressed_size as usize {
        anyhow::bail!(
            "Decoded entry '{}' has invalid size (got {}, expected {})",
            entry.path,
            out.len(),
            entry.uncompressed_size
        );
    }

    Ok(out)
}

struct EntryToWrite {
    path: String,
    kind: EntryKind,
    data: Vec<u8>,
}

pub struct AssetByteStream {
    inner: AssetByteStreamInner,
}

enum AssetByteStreamInner {
    Memory(Vec<u8>),
    BaseV2 {
        mmap: Arc<memmap2::Mmap>,
        entry: V2EntryIndex,
        chunks: Vec<ChunkDescriptor>,
    },
}

impl AssetByteStream {
    pub fn len(&self) -> u64 {
        match &self.inner {
            AssetByteStreamInner::Memory(data) => data.len() as u64,
            AssetByteStreamInner::BaseV2 { entry, .. } => entry.uncompressed_size,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }

        match &self.inner {
            AssetByteStreamInner::Memory(data) => {
                let start = offset as usize;
                if start >= data.len() {
                    return Ok(0);
                }
                let end = std::cmp::min(start + out.len(), data.len());
                let len = end - start;
                out[..len].copy_from_slice(&data[start..end]);
                Ok(len)
            }
            AssetByteStreamInner::BaseV2 {
                mmap,
                entry,
                chunks,
            } => {
                if offset >= entry.uncompressed_size {
                    return Ok(0);
                }

                let chunk_size = entry.chunk_size.max(1) as u64;
                let max_read = std::cmp::min(out.len() as u64, entry.uncompressed_size - offset);
                let mut written = 0usize;
                let mut pos = offset;

                while (written as u64) < max_read {
                    let chunk_idx = (pos / chunk_size) as usize;
                    let chunk_start = (chunk_idx as u64) * chunk_size;
                    let in_chunk = (pos - chunk_start) as usize;
                    let chunk = chunks
                        .get(chunk_idx)
                        .with_context(|| "Invalid chunk index while streaming asset")?;
                    let decoded = decode_chunk_verified(mmap.as_ref().as_ref(), chunk)?;

                    if in_chunk >= decoded.len() {
                        anyhow::bail!("Invalid in-chunk streaming offset");
                    }

                    let remaining_out = (max_read as usize) - written;
                    let remaining_chunk = decoded.len() - in_chunk;
                    let take = std::cmp::min(remaining_out, remaining_chunk);

                    out[written..written + take]
                        .copy_from_slice(&decoded[in_chunk..in_chunk + take]);
                    written += take;
                    pos += take as u64;
                }

                Ok(written)
            }
        }
    }

    pub fn read_range(&self, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut out = vec![0u8; len];
        let read = self.read_at(offset, &mut out)?;
        out.truncate(read);
        Ok(out)
    }
}

pub struct LiveShadePackage {
    source_mmap: Option<Arc<memmap2::Mmap>>,
    source_entries: Vec<V2EntryIndex>,
    pub config: ShadeConfig,
    pub preview: Option<Vec<u8>>,
    base_entries: Vec<AssetEntry>,
    memory_assets: Vec<(String, Vec<u8>)>,
    removed_from_base: HashSet<String>,
    compression_overrides: HashMap<String, CompressionPolicy>,
}

impl std::fmt::Debug for LiveShadePackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveShadePackage")
            .field("has_mmap", &self.source_mmap.is_some())
            .field("config", &self.config)
            .field("has_preview", &self.preview.is_some())
            .field("base_assets", &self.base_entries.len())
            .field("memory_assets", &self.memory_assets.len())
            .field("removed", &self.removed_from_base.len())
            .finish()
    }
}

impl LiveShadePackage {
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open shade package: {}", path.display()))?;

        // SAFETY: Read-only mapping for immutable package bytes.
        let mmap = Arc::new(unsafe { memmap2::MmapOptions::new().map(&file)? });
        let bytes: &[u8] = mmap.as_ref().as_ref();
        let header = parse_header(bytes)?;
        let source_entries = parse_index(bytes, &header)?;

        let find_entry = |name: &str| source_entries.iter().find(|e| e.path == name);

        let config_entry = find_entry("config.toml").with_context(|| "Missing config.toml")?;
        let config_bytes = join_chunks(bytes, config_entry)?;
        let config_str =
            String::from_utf8(config_bytes).with_context(|| "config.toml is not valid UTF-8")?;
        let config: ShadeConfig =
            toml::from_str(&config_str).with_context(|| "Failed to parse config.toml")?;

        let preview = ["preview.jpg", "preview.png", "preview.webp"]
            .iter()
            .find_map(|name| find_entry(name).and_then(|e| join_chunks(bytes, e).ok()));

        let mut base_entries = Vec::new();
        for entry in &source_entries {
            if entry.path != "config.toml"
                && entry.path != "preview.jpg"
                && entry.path != "preview.png"
                && entry.path != "preview.webp"
            {
                base_entries.push(AssetEntry {
                    name: entry.path.clone(),
                    size: entry.uncompressed_size,
                });
            }
        }

        Ok(Self {
            source_mmap: Some(mmap),
            source_entries,
            config,
            preview,
            base_entries,
            memory_assets: Vec::new(),
            removed_from_base: HashSet::new(),
            compression_overrides: HashMap::new(),
        })
    }

    pub fn new_empty(config: ShadeConfig) -> Self {
        Self {
            source_mmap: None,
            source_entries: Vec::new(),
            config,
            preview: None,
            base_entries: Vec::new(),
            memory_assets: Vec::new(),
            removed_from_base: HashSet::new(),
            compression_overrides: HashMap::new(),
        }
    }

    pub fn set_entry_compression(&mut self, path: impl Into<String>, policy: CompressionPolicy) {
        self.compression_overrides.insert(path.into(), policy);
    }

    pub fn package_metadata(&self) -> Result<PackageMetadata> {
        let bytes = self.source_bytes()?;
        let header = parse_header(bytes)?;

        let mut total_chunks = 0u64;
        let mut total_uncompressed = 0u64;
        let mut total_compressed = 0u64;
        for entry in &self.source_entries {
            let chunks = parse_chunk_table(bytes, entry)?;
            total_chunks += chunks.len() as u64;
            total_uncompressed += entry.uncompressed_size;
            total_compressed += chunks.iter().map(|c| c.compressed_size as u64).sum::<u64>();
        }

        Ok(PackageMetadata {
            entry_count: header.entry_count,
            index_offset: header.index_offset,
            index_size: header.index_size,
            file_size: header.file_size,
            total_chunks,
            total_uncompressed,
            total_compressed,
        })
    }

    pub fn entry_debug_infos(&self) -> Result<Vec<EntryDebugInfo>> {
        self.build_entry_debug_infos()
    }

    pub fn entry_debug_info(&self, path: &str) -> Result<Option<EntryDebugInfo>> {
        let entries = self.build_entry_debug_infos()?;
        Ok(entries.into_iter().find(|entry| entry.path == path))
    }

    pub fn chunk_debug_infos(&self, path: &str) -> Result<Option<Vec<ChunkDebugInfo>>> {
        let entry = self.entry_debug_info(path)?;
        Ok(entry.map(|e| e.chunks))
    }

    pub fn compression_stats(&self) -> Result<CompressionStats> {
        let bytes = self.source_bytes()?;
        let entries = self.build_entry_debug_infos()?;

        let mut by_codec_map: HashMap<CompressionCodec, (u64, u64, u64)> = HashMap::new();
        let mut dist_4k = 0u64;
        let mut dist_16k = 0u64;
        let mut dist_64k = 0u64;
        let mut dist_256k = 0u64;
        let mut dist_gt_256k = 0u64;
        let mut entropy_sum = 0.0f64;
        let mut entropy_count = 0u64;

        for entry in &self.source_entries {
            let chunks = parse_chunk_table(bytes, entry)?;
            for chunk in &chunks {
                let stat = by_codec_map.entry(chunk.codec).or_insert((0, 0, 0));
                stat.0 += 1;
                stat.1 += chunk.compressed_size as u64;
                stat.2 += chunk.uncompressed_size as u64;

                let size = chunk.uncompressed_size as u64;
                if size <= 4 * 1024 {
                    dist_4k += 1;
                } else if size <= 16 * 1024 {
                    dist_16k += 1;
                } else if size <= 64 * 1024 {
                    dist_64k += 1;
                } else if size <= 256 * 1024 {
                    dist_256k += 1;
                } else {
                    dist_gt_256k += 1;
                }

                let start = chunk.data_offset as usize;
                let end = start + chunk.compressed_size as usize;
                let sample_end = std::cmp::min(end, start + 4096);
                if sample_end > start {
                    let sample = &bytes[start..sample_end];
                    entropy_sum += shannon_entropy_bits_per_byte(sample);
                    entropy_count += 1;
                }
            }
        }

        let total_uncompressed: u64 = entries.iter().map(|e| e.uncompressed_size).sum();
        let total_compressed: u64 = entries.iter().map(|e| e.compressed_size).sum();

        let mut by_codec: Vec<CodecCompressionStat> = by_codec_map
            .into_iter()
            .map(
                |(codec, (chunk_count, compressed_size, uncompressed_size))| CodecCompressionStat {
                    codec,
                    chunk_count,
                    compressed_size,
                    uncompressed_size,
                    ratio: safe_ratio(compressed_size, uncompressed_size),
                },
            )
            .collect();
        by_codec.sort_by_key(|c| c.codec as u8);

        let mut largest_entries = entries;
        largest_entries.sort_by_key(|e| std::cmp::Reverse(e.uncompressed_size));
        largest_entries.truncate(10);
        for entry in &mut largest_entries {
            entry.chunks.clear();
        }

        let chunk_size_distribution = vec![
            ChunkSizeBucket {
                label: "<=4KiB".to_string(),
                chunk_count: dist_4k,
            },
            ChunkSizeBucket {
                label: "4-16KiB".to_string(),
                chunk_count: dist_16k,
            },
            ChunkSizeBucket {
                label: "16-64KiB".to_string(),
                chunk_count: dist_64k,
            },
            ChunkSizeBucket {
                label: "64-256KiB".to_string(),
                chunk_count: dist_256k,
            },
            ChunkSizeBucket {
                label: ">256KiB".to_string(),
                chunk_count: dist_gt_256k,
            },
        ];

        Ok(CompressionStats {
            total_uncompressed,
            total_compressed,
            compression_ratio: safe_ratio(total_compressed, total_uncompressed),
            savings_vs_stored_bytes: total_uncompressed as i64 - total_compressed as i64,
            by_codec,
            largest_entries,
            chunk_size_distribution,
            average_chunk_entropy_bits_per_byte: if entropy_count == 0 {
                0.0
            } else {
                entropy_sum / entropy_count as f64
            },
        })
    }

    pub fn verify(&self, mode: VerifyMode) -> Result<VerifyReport> {
        let bytes = self.source_bytes()?;
        let mut total_chunks = 0u64;
        let mut checked_chunks = 0u64;
        let mut failures = Vec::new();

        for entry in &self.source_entries {
            let chunks = match parse_chunk_table(bytes, entry) {
                Ok(chunks) => chunks,
                Err(err) => {
                    failures.push(VerifyFailure {
                        path: entry.path.clone(),
                        chunk_index: None,
                        message: err.to_string(),
                    });
                    continue;
                }
            };

            total_chunks += chunks.len() as u64;

            if mode == VerifyMode::Fast {
                let total_entry_uncompressed: u64 = chunks
                    .iter()
                    .map(|chunk| chunk.uncompressed_size as u64)
                    .sum();
                if total_entry_uncompressed != entry.uncompressed_size {
                    failures.push(VerifyFailure {
                        path: entry.path.clone(),
                        chunk_index: None,
                        message: format!(
                            "Entry uncompressed size mismatch (chunks {}, index {})",
                            total_entry_uncompressed, entry.uncompressed_size
                        ),
                    });
                }
                continue;
            }

            for (idx, chunk) in chunks.iter().enumerate() {
                checked_chunks += 1;
                let start = chunk.data_offset as usize;
                let end = start + chunk.compressed_size as usize;
                if end > bytes.len() {
                    failures.push(VerifyFailure {
                        path: entry.path.clone(),
                        chunk_index: Some(idx as u32),
                        message: "Chunk data range is out of bounds".to_string(),
                    });
                    continue;
                }

                let chunk_bytes = &bytes[start..end];
                let mut hasher = crc32fast::Hasher::new();
                hasher.update(chunk_bytes);
                let checksum = hasher.finalize();
                if checksum != chunk.checksum {
                    failures.push(VerifyFailure {
                        path: entry.path.clone(),
                        chunk_index: Some(idx as u32),
                        message: format!(
                            "Checksum mismatch (expected {:08x}, got {:08x})",
                            chunk.checksum, checksum
                        ),
                    });
                    continue;
                }

                if mode == VerifyMode::Decode
                    && let Err(err) = decode_with_codec(
                        chunk.codec,
                        chunk_bytes,
                        chunk.uncompressed_size as usize,
                    )
                {
                    failures.push(VerifyFailure {
                        path: entry.path.clone(),
                        chunk_index: Some(idx as u32),
                        message: format!("Decode failed: {}", err),
                    });
                }
            }
        }

        Ok(VerifyReport {
            mode,
            total_entries: self.source_entries.len() as u32,
            total_chunks,
            checked_chunks,
            ok: failures.is_empty(),
            failures,
        })
    }

    pub fn read_entry_bytes(&self, path: &str) -> Result<Option<Vec<u8>>> {
        if let Some((_, data)) = self.memory_assets.iter().find(|(name, _)| name == path) {
            return Ok(Some(data.clone()));
        }
        if self.removed_from_base.contains(path) {
            return Ok(None);
        }

        if let Some(mmap) = &self.source_mmap
            && let Some(entry) = self.source_entries.iter().find(|entry| entry.path == path)
        {
            let bytes: &[u8] = mmap.as_ref().as_ref();
            return Ok(Some(join_chunks(bytes, entry)?));
        }

        let out = match path {
            "config.toml" => Some(
                toml::to_string_pretty(&self.config)
                    .with_context(|| "Failed to serialize config.toml")?
                    .into_bytes(),
            ),
            "preview.jpg" | "preview.png" | "preview.webp" => self.preview.clone(),
            _ => None,
        };

        Ok(out)
    }

    pub fn read_chunk_bytes(
        &self,
        path: &str,
        chunk_index: usize,
        decoded: bool,
    ) -> Result<Option<Vec<u8>>> {
        let bytes = match &self.source_mmap {
            Some(mmap) => mmap.as_ref().as_ref(),
            None => return Ok(None),
        };
        let entry = match self.source_entries.iter().find(|entry| entry.path == path) {
            Some(entry) => entry,
            None => return Ok(None),
        };
        let chunks = parse_chunk_table(bytes, entry)?;
        let chunk = match chunks.get(chunk_index) {
            Some(chunk) => chunk,
            None => return Ok(None),
        };

        let start = chunk.data_offset as usize;
        let end = start + chunk.compressed_size as usize;
        let raw = &bytes[start..end];

        if decoded {
            Ok(Some(decode_chunk_verified(bytes, chunk)?))
        } else {
            Ok(Some(raw.to_vec()))
        }
    }

    pub fn asset_entries(&self) -> Vec<AssetEntry> {
        let mut entries: Vec<AssetEntry> = self
            .base_entries
            .iter()
            .filter(|e| !self.removed_from_base.contains(&e.name))
            .filter(|e| !self.memory_assets.iter().any(|(n, _)| n == &e.name))
            .cloned()
            .collect();

        for (name, data) in &self.memory_assets {
            entries.push(AssetEntry {
                name: name.clone(),
                size: data.len() as u64,
            });
        }

        entries
    }

    pub fn read_asset(&self, name: &str) -> Option<Vec<u8>> {
        if let Some((_, data)) = self.memory_assets.iter().find(|(n, _)| n == name) {
            return Some(data.clone());
        }
        if self.removed_from_base.contains(name) {
            return None;
        }
        self.read_entry_from_base(name)
    }

    pub fn open_asset_stream(&self, name: &str) -> Option<AssetByteStream> {
        if let Some((_, data)) = self.memory_assets.iter().find(|(n, _)| n == name) {
            return Some(AssetByteStream {
                inner: AssetByteStreamInner::Memory(data.clone()),
            });
        }
        if self.removed_from_base.contains(name) {
            return None;
        }

        let mmap = self.source_mmap.as_ref()?.clone();
        let entry = self.source_entries.iter().find(|e| e.path == name)?.clone();
        let chunks = parse_chunk_table(mmap.as_ref().as_ref(), &entry).ok()?;

        Some(AssetByteStream {
            inner: AssetByteStreamInner::BaseV2 {
                mmap,
                entry,
                chunks,
            },
        })
    }

    pub fn add_asset(&mut self, name: String, data: Vec<u8>) {
        self.memory_assets.retain(|(n, _)| n != &name);
        self.memory_assets.push((name, data));
    }

    pub fn remove_asset(&mut self, name: &str) {
        self.memory_assets.retain(|(n, _)| n != name);
        if self.base_entries.iter().any(|e| e.name == name) {
            self.removed_from_base.insert(name.to_string());
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let mut entries = Vec::<EntryToWrite>::new();

        let config_toml = toml::to_string_pretty(&self.config)
            .with_context(|| "Failed to serialize config.toml")?;
        entries.push(EntryToWrite {
            path: "config.toml".to_string(),
            kind: EntryKind::ConfigToml,
            data: config_toml.into_bytes(),
        });

        if let Some(preview) = &self.preview {
            entries.push(EntryToWrite {
                path: "preview.jpg".to_string(),
                kind: EntryKind::Preview,
                data: preview.clone(),
            });
        }

        for entry in self.asset_entries() {
            if let Some(data) = self.read_asset(&entry.name) {
                entries.push(EntryToWrite {
                    path: entry.name,
                    kind: EntryKind::Asset,
                    data,
                });
            }
        }

        let mut file = std::fs::File::create(path)
            .with_context(|| format!("Failed to create shade package: {}", path.display()))?;
        file.write_all(&[0u8; HEADER_SIZE])?;

        let mut index_entries = Vec::<V2EntryIndex>::new();
        let mut all_chunk_tables = Vec::<Vec<ChunkDescriptor>>::new();

        for entry in entries {
            let policy = self
                .compression_overrides
                .get(&entry.path)
                .cloned()
                .unwrap_or_else(|| default_policy_for_path(&entry.path));

            let mut chunks = Vec::<ChunkDescriptor>::new();
            let mut offset = 0usize;
            let mut first_codec = CompressionCodec::None;
            let mut first_level = 0i16;
            let mut first = true;

            while offset < entry.data.len() {
                let end = std::cmp::min(offset + DEFAULT_CHUNK_SIZE as usize, entry.data.len());
                let raw_chunk = &entry.data[offset..end];

                let (codec, level, encoded) = match policy {
                    CompressionPolicy::None => (CompressionCodec::None, 0i16, raw_chunk.to_vec()),
                    CompressionPolicy::Lz4 => {
                        let encoded = encode_with_codec(CompressionCodec::Lz4, 0, raw_chunk)?;
                        (CompressionCodec::Lz4, 0i16, encoded)
                    }
                    CompressionPolicy::Zstd { level } => {
                        let encoded = encode_with_codec(CompressionCodec::Zstd, level, raw_chunk)?;
                        (CompressionCodec::Zstd, level as i16, encoded)
                    }
                    CompressionPolicy::Auto => auto_select_codec(raw_chunk)?,
                };

                if first {
                    first_codec = codec;
                    first_level = level;
                    first = false;
                }

                let data_offset = file.stream_position()?;
                file.write_all(&encoded)?;

                let mut hasher = crc32fast::Hasher::new();
                hasher.update(&encoded);
                let checksum = hasher.finalize();

                chunks.push(ChunkDescriptor {
                    data_offset,
                    compressed_size: encoded.len() as u32,
                    uncompressed_size: raw_chunk.len() as u32,
                    checksum,
                    codec,
                });

                offset = end;
            }

            if entry.data.is_empty() {
                let data_offset = file.stream_position()?;
                chunks.push(ChunkDescriptor {
                    data_offset,
                    compressed_size: 0,
                    uncompressed_size: 0,
                    checksum: 0,
                    codec: CompressionCodec::None,
                });
            }

            index_entries.push(V2EntryIndex {
                path: entry.path,
                kind: entry.kind,
                default_codec: first_codec,
                default_level: first_level,
                chunk_size: DEFAULT_CHUNK_SIZE,
                uncompressed_size: entry.data.len() as u64,
                chunk_count: chunks.len() as u32,
                chunk_table_offset: 0,
            });
            all_chunk_tables.push(chunks);
        }

        for (idx, table) in all_chunk_tables.iter().enumerate() {
            let table_offset = file.stream_position()?;
            for chunk in table {
                file.write_all(&chunk.data_offset.to_le_bytes())?;
                file.write_all(&chunk.compressed_size.to_le_bytes())?;
                file.write_all(&chunk.uncompressed_size.to_le_bytes())?;
                file.write_all(&chunk.checksum.to_le_bytes())?;
                file.write_all(&[chunk.codec as u8, 0, 0, 0])?;
            }
            index_entries[idx].chunk_table_offset = table_offset;
        }

        let index_offset = file.stream_position()?;
        let mut index_blob = Vec::new();
        write_u32(&mut index_blob, index_entries.len() as u32);

        for entry in &index_entries {
            write_u16(&mut index_blob, entry.path.len() as u16);
            index_blob.extend_from_slice(entry.path.as_bytes());
            index_blob.push(entry.kind as u8);
            index_blob.push(entry.default_codec as u8);
            write_u16(&mut index_blob, entry.default_level as u16);
            write_u32(&mut index_blob, entry.chunk_size);
            write_u64(&mut index_blob, entry.uncompressed_size);
            write_u32(&mut index_blob, entry.chunk_count);
            write_u64(&mut index_blob, entry.chunk_table_offset);
        }
        file.write_all(&index_blob)?;

        let file_size = file.seek(SeekFrom::End(0))?;
        let header = Header {
            entry_count: index_entries.len() as u32,
            index_offset,
            index_size: index_blob.len() as u64,
            file_size,
        };
        write_header(&mut file, &header)?;

        Ok(())
    }

    fn read_entry_from_base(&self, name: &str) -> Option<Vec<u8>> {
        let mmap = self.source_mmap.as_ref()?;
        let entry = self.source_entries.iter().find(|e| e.path == name)?;
        join_chunks(mmap.as_ref().as_ref(), entry).ok()
    }

    fn source_bytes(&self) -> Result<&[u8]> {
        let mmap = self
            .source_mmap
            .as_ref()
            .with_context(|| "Inspection is only available for packages loaded from disk")?;
        Ok(mmap.as_ref().as_ref())
    }

    fn build_entry_debug_infos(&self) -> Result<Vec<EntryDebugInfo>> {
        let bytes = self.source_bytes()?;
        let mut entries = Vec::with_capacity(self.source_entries.len());

        for entry in &self.source_entries {
            let chunks = parse_chunk_table(bytes, entry)?;
            let compressed_size: u64 = chunks
                .iter()
                .map(|chunk| chunk.compressed_size as u64)
                .sum();

            let data_start_offset = chunks
                .iter()
                .map(|chunk| chunk.data_offset)
                .min()
                .unwrap_or(0);
            let data_end_offset = chunks
                .iter()
                .map(|chunk| chunk.data_offset + chunk.compressed_size as u64)
                .max()
                .unwrap_or(0);

            let chunk_infos = chunks
                .into_iter()
                .enumerate()
                .map(|(index, chunk)| ChunkDebugInfo {
                    index: index as u32,
                    data_offset: chunk.data_offset,
                    compressed_size: chunk.compressed_size,
                    uncompressed_size: chunk.uncompressed_size,
                    checksum: chunk.checksum,
                    codec: chunk.codec,
                })
                .collect::<Vec<_>>();

            entries.push(EntryDebugInfo {
                path: entry.path.clone(),
                kind: entry.kind.into(),
                default_codec: entry.default_codec,
                default_level: entry.default_level,
                chunk_size: entry.chunk_size,
                chunk_count: entry.chunk_count,
                uncompressed_size: entry.uncompressed_size,
                compressed_size,
                compression_ratio: safe_ratio(compressed_size, entry.uncompressed_size),
                data_start_offset,
                data_end_offset,
                chunks: chunk_infos,
            });
        }

        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{RenderingConfig, ShadeMeta};
    use tempfile::tempdir;

    fn test_config() -> ShadeConfig {
        ShadeConfig {
            meta: ShadeMeta {
                name: "test".into(),
                author: "test".into(),
                version: "1.0".into(),
                description: String::new(),
                tags: Vec::new(),
            },
            rendering: RenderingConfig::default(),
            states: Default::default(),
            transitions: Default::default(),
            transitions_usage: Default::default(),
        }
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut data = vec![0u8; HEADER_SIZE];
        data[0..8].copy_from_slice(b"NOTSHADE");
        assert!(parse_header(&data).is_err());
    }

    #[test]
    fn auto_prefers_none_when_savings_tiny() {
        let bytes = vec![1u8; 16];
        let (codec, _level, _encoded) = auto_select_codec(&bytes).expect("auto codec");
        assert_eq!(codec, CompressionCodec::None);
    }

    #[test]
    fn roundtrip_asset_stream_reads_partial_ranges() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("roundtrip.shade");

        let mut pkg = LiveShadePackage::new_empty(test_config());
        let payload: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        pkg.add_asset("assets/video.bin".into(), payload.clone());
        pkg.save(&path).expect("save");

        let loaded = LiveShadePackage::load(&path).expect("load");
        let stream = loaded
            .open_asset_stream("assets/video.bin")
            .expect("stream");
        assert_eq!(stream.len(), payload.len() as u64);

        let read = stream.read_range(1234, 8192).expect("read range");
        assert_eq!(read, payload[1234..1234 + 8192]);
    }

    #[test]
    fn deterministic_save_produces_identical_bytes() {
        let tmp = tempdir().expect("tempdir");
        let a = tmp.path().join("a.shade");
        let b = tmp.path().join("b.shade");

        let mut pkg = LiveShadePackage::new_empty(test_config());
        pkg.add_asset(
            "shader.frag".into(),
            b"void mainImage(out vec4 c, in vec2 f){c=vec4(1.0);}".to_vec(),
        );
        pkg.add_asset("assets/a.bin".into(), vec![7u8; 8192]);

        pkg.save(&a).expect("save a");
        pkg.save(&b).expect("save b");

        let bytes_a = std::fs::read(&a).expect("read a");
        let bytes_b = std::fs::read(&b).expect("read b");
        assert_eq!(bytes_a, bytes_b);
    }

    #[test]
    fn chunk_checksum_corruption_is_detected_on_stream_read() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("corrupt.shade");

        let mut pkg = LiveShadePackage::new_empty(test_config());
        pkg.add_asset("assets/corrupt.bin".into(), vec![42u8; 128 * 1024]);
        pkg.save(&path).expect("save");

        let mut bytes = std::fs::read(&path).expect("read original");
        let header = parse_header(&bytes).expect("parse header");
        let entries = parse_index(&bytes, &header).expect("parse index");
        let entry = entries
            .iter()
            .find(|e| e.path == "assets/corrupt.bin")
            .expect("asset entry");
        let chunks = parse_chunk_table(&bytes, entry).expect("chunk table");
        let first = chunks.first().expect("first chunk");
        bytes[first.data_offset as usize] ^= 0xFF;
        std::fs::write(&path, &bytes).expect("write corrupted");

        let loaded = LiveShadePackage::load(&path).expect("load");
        let stream = loaded
            .open_asset_stream("assets/corrupt.bin")
            .expect("stream");
        let err = stream.read_range(0, 1024).expect_err("must fail checksum");
        assert!(err.to_string().contains("checksum"));
    }

    #[test]
    fn inspect_metadata_entries_and_stats_are_available() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("inspect.shade");

        let mut pkg = LiveShadePackage::new_empty(test_config());
        pkg.add_asset(
            "shader.frag".into(),
            b"void mainImage(out vec4 c, in vec2 f){c=vec4(1.0);}".to_vec(),
        );
        pkg.preview = Some(vec![1, 2, 3, 4, 5]);
        pkg.add_asset("assets/alpha.bin".into(), vec![7u8; 32 * 1024]);
        pkg.add_asset("assets/beta.bin".into(), vec![11u8; 48 * 1024]);
        pkg.save(&path).expect("save");

        let loaded = LiveShadePackage::load(&path).expect("load");
        let metadata = loaded.package_metadata().expect("metadata");
        assert!(metadata.entry_count >= 4);
        assert!(metadata.total_chunks > 0);
        assert!(metadata.total_uncompressed > 0);

        let entries = loaded.entry_debug_infos().expect("entries");
        assert!(entries.iter().any(|e| e.path == "config.toml"));
        assert!(entries.iter().any(|e| e.path == "shader.frag"));
        assert!(entries.iter().any(|e| e.path == "assets/alpha.bin"));

        let stats = loaded.compression_stats().expect("stats");
        assert!(stats.total_uncompressed >= stats.total_compressed);
        assert!(!stats.by_codec.is_empty());
    }

    #[test]
    fn verify_reports_corruption() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("verify_corrupt.shade");

        let mut pkg = LiveShadePackage::new_empty(test_config());
        pkg.add_asset("assets/verify.bin".into(), vec![55u8; 96 * 1024]);
        pkg.save(&path).expect("save");

        let loaded_ok = LiveShadePackage::load(&path).expect("load");
        let report_ok = loaded_ok.verify(VerifyMode::Checksum).expect("verify ok");
        assert!(report_ok.ok);
        assert!(report_ok.failures.is_empty());

        let mut bytes = std::fs::read(&path).expect("read");
        let header = parse_header(&bytes).expect("header");
        let entries = parse_index(&bytes, &header).expect("index");
        let entry = entries
            .iter()
            .find(|e| e.path == "assets/verify.bin")
            .expect("entry");
        let chunks = parse_chunk_table(&bytes, entry).expect("chunks");
        let first = chunks.first().expect("first chunk");
        bytes[first.data_offset as usize] ^= 0xAA;
        std::fs::write(&path, &bytes).expect("write");

        let loaded_bad = LiveShadePackage::load(&path).expect("load bad");
        let report_bad = loaded_bad.verify(VerifyMode::Checksum).expect("verify bad");
        assert!(!report_bad.ok);
        assert!(!report_bad.failures.is_empty());
    }
}
