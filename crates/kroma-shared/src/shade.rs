//! `.shade` package support — reading and writing ZIP-based wallpaper packages.

use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;

use crate::types::ShadeConfig;

/// A loaded `.shade` package in memory.
#[derive(Debug, Clone)]
pub struct ShadePackage {
    /// Parsed `config.toml`.
    pub config: ShadeConfig,
    /// The raw GLSL fragment shader source (optional — not needed for image/video wallpapers).
    pub shader_source: Option<String>,
    /// Optional preview image bytes (JPEG/PNG/WebP).
    pub preview: Option<Vec<u8>>,
    /// Asset files: `(relative_path, bytes)`.
    pub assets: Vec<(String, Vec<u8>)>,
}

impl ShadePackage {
    /// Load a `.shade` file (ZIP) from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open shade package: {}", path.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| "Invalid shade package (not a valid ZIP)")?;

        // --- config.toml (required) ---
        let config: ShadeConfig = {
            let mut entry = archive
                .by_name("config.toml")
                .with_context(|| "Missing config.toml in shade package")?;
            let mut buf = String::new();
            entry.read_to_string(&mut buf)?;
            toml::from_str(&buf).with_context(|| "Failed to parse config.toml")?
        };

        // --- shader.frag (optional — not required for image/video modes) ---
        let shader_source = if let Ok(mut entry) = archive.by_name("shader.frag") {
            let mut buf = String::new();
            entry.read_to_string(&mut buf)?;
            Some(buf)
        } else {
            None
        };

        // --- preview image (optional, any common format) ---
        let preview = ["preview.jpg", "preview.png", "preview.webp"]
            .iter()
            .find_map(|name| {
                archive.by_name(name).ok().and_then(|mut entry| {
                    let mut buf = Vec::new();
                    entry.read_to_end(&mut buf).ok()?;
                    Some(buf)
                })
            });

        // --- assets/ (optional) ---
        let mut assets = Vec::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_string();
            if name.starts_with("assets/") && !entry.is_dir() {
                // Reject path-traversal attempts (e.g., `assets/../../etc/passwd`)
                if name.contains("..") {
                    continue;
                }
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                assets.push((name, buf));
            }
        }

        Ok(ShadePackage {
            config,
            shader_source,
            preview,
            assets,
        })
    }

    /// Write this package to a `.shade` file on disk.
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

        // assets/
        for (name, data) in &self.assets {
            zip.start_file(name.as_str(), options)?;
            zip.write_all(data)?;
        }

        zip.finish()?;
        Ok(())
    }
}

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

    /// Create from an existing eager `ShadePackage` (e.g. from import).
    pub fn from_package(pkg: ShadePackage) -> Self {
        LiveShadePackage {
            source_mmap: None,
            config: pkg.config,
            shader_source: pkg.shader_source,
            preview: pkg.preview,
            base_entries: Vec::new(),
            memory_assets: pkg.assets,
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

    /// Materialize into an eager `ShadePackage` (decompresses all assets).
    pub fn to_package(&self) -> Result<ShadePackage> {
        let mut assets = Vec::new();
        for entry in self.asset_entries() {
            if let Some(data) = self.read_asset(&entry.name) {
                assets.push((entry.name, data));
            }
        }
        Ok(ShadePackage {
            config: self.config.clone(),
            shader_source: self.shader_source.clone(),
            preview: self.preview.clone(),
            assets,
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ShadeConfig, ShadeMeta};

    #[test]
    fn round_trip_shade_package() {
        let pkg = ShadePackage {
            config: ShadeConfig {
                meta: ShadeMeta {
                    name: "Test".into(),
                    author: "Tester".into(),
                    version: "1.0".into(),
                    description: String::new(),
                    tags: Vec::new(),
                },
                mode: Default::default(),
                rendering: Default::default(),
                audio: Default::default(),
                uniforms: Default::default(),
                textures: Default::default(),
                slideshow: Default::default(),
                fonts: Default::default(),
                buffers: Default::default(),
            },
            shader_source: Some("void main() { gl_FragColor = vec4(1.0); }".into()),
            preview: Some(vec![0xFF, 0xD8, 0xFF]),
            assets: vec![("assets/test.txt".into(), b"hello".to_vec())],
        };

        let dir = tempfile::tempdir().unwrap();
        let shade_path = dir.path().join("test.shade");
        pkg.save(&shade_path).unwrap();

        let loaded = ShadePackage::load(&shade_path).unwrap();
        assert_eq!(loaded.config.meta.name, "Test");
        assert_eq!(loaded.shader_source, pkg.shader_source);
        assert!(loaded.preview.is_some());
        assert_eq!(loaded.assets.len(), 1);
    }

    #[test]
    fn live_shade_mmap_lazy_decompression() {
        // First, create a .shade file via the eager ShadePackage
        let pkg = ShadePackage {
            config: ShadeConfig {
                meta: ShadeMeta {
                    name: "LiveTest".into(),
                    author: "Tester".into(),
                    version: "1.0".into(),
                    description: String::new(),
                    tags: Vec::new(),
                },
                mode: Default::default(),
                rendering: Default::default(),
                audio: Default::default(),
                uniforms: Default::default(),
                textures: Default::default(),
                slideshow: Default::default(),
                fonts: Default::default(),
                buffers: Default::default(),
            },
            shader_source: Some("void main() {}".into()),
            preview: Some(vec![0xFF, 0xD8]),
            assets: vec![
                ("assets/img.png".into(), vec![0x89, 0x50, 0x4E, 0x47]),
                ("assets/data.bin".into(), vec![1, 2, 3, 4, 5]),
            ],
        };

        let dir = tempfile::tempdir().unwrap();
        let shade_path = dir.path().join("live.shade");
        pkg.save(&shade_path).unwrap();

        // Load via LiveShadePackage (mmap-backed)
        let live = LiveShadePackage::load(&shade_path).unwrap();

        // Metadata loaded eagerly
        assert_eq!(live.config.meta.name, "LiveTest");
        assert_eq!(live.shader_source.as_deref(), Some("void main() {}"));
        assert!(live.preview.is_some());

        // Asset entries available without decompression
        let entries = live.asset_entries();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.name == "assets/img.png"));
        assert!(entries.iter().any(|e| e.name == "assets/data.bin"));

        // Lazy decompression on demand
        let img_data = live.read_asset("assets/img.png").unwrap();
        assert_eq!(img_data, vec![0x89, 0x50, 0x4E, 0x47]);
        let bin_data = live.read_asset("assets/data.bin").unwrap();
        assert_eq!(bin_data, vec![1, 2, 3, 4, 5]);

        // Non-existent asset
        assert!(live.read_asset("assets/nope.txt").is_none());
    }

    #[test]
    fn live_shade_add_remove_save() {
        let dir = tempfile::tempdir().unwrap();
        let shade_path = dir.path().join("modify.shade");

        // Create initial package
        let pkg = ShadePackage {
            config: ShadeConfig {
                meta: ShadeMeta {
                    name: "Modify".into(),
                    author: "A".into(),
                    version: "1.0".into(),
                    description: String::new(),
                    tags: Vec::new(),
                },
                mode: Default::default(),
                rendering: Default::default(),
                audio: Default::default(),
                uniforms: Default::default(),
                textures: Default::default(),
                slideshow: Default::default(),
                fonts: Default::default(),
                buffers: Default::default(),
            },
            shader_source: None,
            preview: None,
            assets: vec![("assets/old.txt".into(), b"old data".to_vec())],
        };
        pkg.save(&shade_path).unwrap();

        // Load as live package, modify
        let mut live = LiveShadePackage::load(&shade_path).unwrap();
        live.add_asset("assets/new.txt".into(), b"new data".to_vec());
        live.remove_asset("assets/old.txt");

        let entries = live.asset_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "assets/new.txt");

        // Save modified package
        let new_path = dir.path().join("modified.shade");
        live.save(&new_path).unwrap();

        // Reload and verify
        let reloaded = LiveShadePackage::load(&new_path).unwrap();
        assert_eq!(reloaded.asset_entries().len(), 1);
        let data = reloaded.read_asset("assets/new.txt").unwrap();
        assert_eq!(data, b"new data");
        assert!(reloaded.read_asset("assets/old.txt").is_none());
    }

    #[test]
    fn live_shade_new_empty_add_asset() {
        // Simulates: user creates "New Shader", then adds an asset
        let config = ShadeConfig {
            meta: ShadeMeta {
                name: "Fresh".into(),
                author: "User".into(),
                version: "1.0".into(),
                description: String::new(),
                tags: Vec::new(),
            },
            mode: Default::default(),
            rendering: Default::default(),
            audio: Default::default(),
            uniforms: Default::default(),
            textures: Default::default(),
            slideshow: Default::default(),
            fonts: Default::default(),
            buffers: Default::default(),
        };
        let mut live = LiveShadePackage::from_package(ShadePackage {
            config,
            shader_source: Some("void main() {}".into()),
            preview: None,
            assets: Vec::new(),
        });
        assert_eq!(live.asset_entries().len(), 0);

        // Simulate adding an asset (like "+ Asset" button)
        live.add_asset("assets/texture.png".into(), vec![0x89, 0x50, 0x4E, 0x47]);
        assert_eq!(live.asset_entries().len(), 1);
        assert_eq!(live.asset_entries()[0].name, "assets/texture.png");
        assert_eq!(live.asset_entries()[0].size, 4);

        // Read it back
        let data = live.read_asset("assets/texture.png").unwrap();
        assert_eq!(data, vec![0x89, 0x50, 0x4E, 0x47]);

        // Save and reload
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.shade");
        live.save(&path).unwrap();

        let reloaded = LiveShadePackage::load(&path).unwrap();
        assert_eq!(reloaded.asset_entries().len(), 1);
        let data = reloaded.read_asset("assets/texture.png").unwrap();
        assert_eq!(data, vec![0x89, 0x50, 0x4E, 0x47]);
    }
}
