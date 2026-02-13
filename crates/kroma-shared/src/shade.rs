//! `.shade` package support — reading and writing ZIP-based wallpaper packages.

use std::io::{Read, Write};
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
            toml::from_str(&buf)
                .with_context(|| "Failed to parse config.toml")?
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
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ShadeMeta, ShadeConfig};

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
}
