//! `.shade` package support — reading and writing ZIP-based wallpaper packages.

use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;

use crate::types::ShadeConfig;

// ---------------------------------------------------------------------------
// LiveShadePackage — mmap-backed shade with lazy asset decompression
// ---------------------------------------------------------------------------

/// Metadata about an asset entry (available without decompression).
#[derive(Debug, Clone)]
pub struct AssetEntry {
    /// Relative path within the ZIP (e.g. `assets/texture.png`).
    pub name: String,
    /// Uncompressed size in bytes.
    pub size: u64,
}

/// A `.shade` package backed by a memory-mapped ZIP file.
///
/// The ZIP is mapped into virtual memory and assets are only decompressed on
/// demand via [`read_asset`](LiveShadePackage::read_asset). Small metadata
/// (config, shader source, preview) is loaded eagerly. Added/removed assets
/// are tracked in an in-memory overlay until [`save`](LiveShadePackage::save)
/// is called.
pub struct LiveShadePackage {
    /// Memory-mapped ZIP data (present when loaded from disk).
    source_mmap: Option<memmap2::Mmap>,
    /// Parsed `config.toml` (always loaded eagerly).
    pub config: ShadeConfig,
    /// GLSL fragment shader source (always loaded eagerly).
    pub shader_source: Option<String>,
    /// Preview image bytes (always loaded eagerly).
    pub preview: Option<Vec<u8>>,
    /// Asset entries from the base ZIP (name + uncompressed size).
    base_entries: Vec<AssetEntry>,
    /// In-memory assets (added or modified since load).
    memory_assets: Vec<(String, Vec<u8>)>,
    /// Names of assets removed from the base.
    removed_from_base: HashSet<String>,
}

impl std::fmt::Debug for LiveShadePackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveShadePackage")
            .field("has_mmap", &self.source_mmap.is_some())
            .field("config", &self.config)
            .field("has_shader", &self.shader_source.is_some())
            .field("has_preview", &self.preview.is_some())
            .field("base_assets", &self.base_entries.len())
            .field("memory_assets", &self.memory_assets.len())
            .field("removed", &self.removed_from_base.len())
            .finish()
    }
}

impl LiveShadePackage {
    /// Load a `.shade` file by memory-mapping the ZIP.
    ///
    /// Only the small metadata (config, shader, preview) is decompressed
    /// eagerly. Assets are listed but NOT decompressed until requested.
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open shade package: {}", path.display()))?;

        // SAFETY: The file is read-only and we don't mutate the mmap.
        // The mmap is valid for as long as the file exists on disk.
        let mmap = unsafe { memmap2::MmapOptions::new().map(&file)? };

        let mut archive = zip::ZipArchive::new(Cursor::new(mmap.as_ref()))
            .with_context(|| "Invalid shade package (not a valid ZIP)")?;

        // --- config.toml (required, always decompressed) ---
        let config: ShadeConfig = {
            let mut entry = archive
                .by_name("config.toml")
                .with_context(|| "Missing config.toml in shade package")?;
            let mut buf = String::new();
            entry.read_to_string(&mut buf)?;
            toml::from_str(&buf).with_context(|| "Failed to parse config.toml")?
        };

        // --- shader.frag (optional, always decompressed) ---
        let shader_source = if let Ok(mut entry) = archive.by_name("shader.frag") {
            let mut buf = String::new();
            entry.read_to_string(&mut buf)?;
            Some(buf)
        } else {
            None
        };

        // --- preview image (optional, always decompressed) ---
        let preview = ["preview.jpg", "preview.png", "preview.webp"]
            .iter()
            .find_map(|name| {
                archive.by_name(name).ok().and_then(|mut entry| {
                    let mut buf = Vec::new();
                    entry.read_to_end(&mut buf).ok()?;
                    Some(buf)
                })
            });

        // --- Collect asset entry names + sizes (NO decompression) ---
        let mut base_entries = Vec::new();
        for i in 0..archive.len() {
            let entry = archive.by_index_raw(i)?;
            let name = entry.name().to_string();
            if name.starts_with("assets/") && !entry.is_dir() && !name.contains("..") {
                base_entries.push(AssetEntry {
                    name,
                    size: entry.size(),
                });
            }
        }

        Ok(LiveShadePackage {
            source_mmap: Some(mmap),
            config,
            shader_source,
            preview,
            base_entries,
            memory_assets: Vec::new(),
            removed_from_base: HashSet::new(),
        })
    }

    /// Create a new empty (in-memory) package with no backing file.
    pub fn new_empty(config: ShadeConfig) -> Self {
        LiveShadePackage {
            source_mmap: None,
            config,
            shader_source: None,
            preview: None,
            base_entries: Vec::new(),
            memory_assets: Vec::new(),
            removed_from_base: HashSet::new(),
        }
    }
    /// List all effective asset entries (base − removed + memory overlay).
    ///
    /// Returns (name, size) pairs without decompressing anything.
    pub fn asset_entries(&self) -> Vec<AssetEntry> {
        let mut entries: Vec<AssetEntry> = self
            .base_entries
            .iter()
            .filter(|e| !self.removed_from_base.contains(&e.name))
            // Exclude base entries that have been overridden in memory
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

    /// Decompress and return a single asset's bytes on demand.
    ///
    /// Checks the in-memory overlay first, then decompresses from the base
    /// ZIP if available. Returns `None` if the asset does not exist.
    pub fn read_asset(&self, name: &str) -> Option<Vec<u8>> {
        // 1. Check in-memory overlay (modified/added assets first)
        if let Some((_, data)) = self.memory_assets.iter().find(|(n, _)| n == name) {
            return Some(data.clone());
        }
        // 2. Check if removed from base
        if self.removed_from_base.contains(name) {
            return None;
        }
        // 3. Decompress from base ZIP via mmap
        self.decompress_from_base(name)
    }

    /// Add or replace an asset in the in-memory overlay.
    pub fn add_asset(&mut self, name: String, data: Vec<u8>) {
        // Remove from memory if already present, then add
        self.memory_assets.retain(|(n, _)| n != &name);
        self.memory_assets.push((name, data));
    }

    /// Remove an asset by name.
    pub fn remove_asset(&mut self, name: &str) {
        // Remove from overlay
        self.memory_assets.retain(|(n, _)| n != name);
        // Mark removed from base
        if self.base_entries.iter().any(|e| e.name == name) {
            self.removed_from_base.insert(name.to_string());
        }
    }

    /// Write the effective package to a `.shade` file on disk.
    ///
    /// All assets (base + overlay − removed) are materialized into the new ZIP.
    pub fn save(&self, path: &Path) -> Result<()> {
        let file = std::fs::File::create(path)
            .with_context(|| format!("Failed to create shade package: {}", path.display()))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Zstd);

        // config.toml
        let config_str = toml::to_string_pretty(&self.config)
            .with_context(|| "Failed to serialize config.toml")?;
        zip.start_file("config.toml", options)?;
        zip.write_all(config_str.as_bytes())?;

        // shader.frag
        if let Some(ref shader) = self.shader_source {
            zip.start_file("shader.frag", options)?;
            zip.write_all(shader.as_bytes())?;
        }

        // preview.jpg
        if let Some(ref preview) = self.preview {
            zip.start_file("preview.jpg", options)?;
            zip.write_all(preview)?;
        }

        // Write all effective assets
        for entry in self.asset_entries() {
            if let Some(data) = self.read_asset(&entry.name) {
                zip.start_file(entry.name.as_str(), options)?;
                zip.write_all(&data)?;
            }
        }

        zip.finish()?;
        Ok(())
    }

    /// Decompress a single entry from the mmap-backed base ZIP.
    fn decompress_from_base(&self, name: &str) -> Option<Vec<u8>> {
        let mmap = self.source_mmap.as_ref()?;
        let mut archive = zip::ZipArchive::new(Cursor::new(mmap.as_ref())).ok()?;
        let mut entry = archive.by_name(name).ok()?;
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf).ok()?;
        Some(buf)
    }
}
