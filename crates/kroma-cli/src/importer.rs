//! Shadertoy importer — reads a GLSL file or downloads from Shadertoy API
//! and produces a `.shade` package.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::info;

use kroma_shared::compression::{
    parse_default_compression_policy, parse_entry_compression_override,
};
use kroma_shared::shade::{CompressionPolicy, LiveShadePackage};
use kroma_shared::translator;
use kroma_shared::types::{ShadeConfig, TextureDef, TextureType};

#[derive(Debug, Clone)]
pub struct PackOptions {
    pub default_codec: String,
    pub default_level: String,
    pub entry_compression: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct MigrationReport {
    pub embedded_assets: usize,
    pub warnings: Vec<String>,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            default_codec: "auto".to_string(),
            default_level: "auto".to_string(),
            entry_compression: Vec::new(),
        }
    }
}

/// Import a Shadertoy GLSL file and write a `.shade` package next to it.
///
/// Returns the path to the produced `.shade` file.
pub fn import_shadertoy_file(glsl_path: &Path, name: &str, author: &str) -> Result<PathBuf> {
    let source = std::fs::read_to_string(glsl_path)
        .with_context(|| format!("Failed to read shader file: {}", glsl_path.display()))?;

    let result = translator::translate(&source, name, author);

    for w in &result.warnings {
        log::warn!("Translator warning: {}", w);
    }

    let mut package = LiveShadePackage::new_empty(result.config);
    package.shader_source = Some(result.shader_source);

    let output_path = glsl_path.with_extension("shade");
    package
        .save(&output_path)
        .with_context(|| format!("Failed to write shade package: {}", output_path.display()))?;

    info!(
        "Shade package created: {} ({} bytes)",
        output_path.display(),
        std::fs::metadata(&output_path)?.len()
    );

    Ok(output_path)
}

/// Pack a folder into a `.shade` package.
///
/// Expected layout:
/// - `config.toml` (required)
/// - `shader.frag` (optional)
/// - only files referenced by `config.toml` are embedded as assets
pub fn pack_folder_to_shade(
    folder: &Path,
    output: Option<&Path>,
    options: &PackOptions,
) -> Result<PathBuf> {
    if !folder.is_dir() {
        anyhow::bail!("Input is not a directory: {}", folder.display());
    }

    let config_path = folder.join("config.toml");
    let config_str = std::fs::read_to_string(&config_path).with_context(|| {
        format!(
            "Missing or unreadable config.toml: {}",
            config_path.display()
        )
    })?;
    let config: ShadeConfig =
        toml::from_str(&config_str).with_context(|| "Failed to parse config.toml")?;

    let mut package = LiveShadePackage::new_empty(config);

    let shader_path = folder.join("shader.frag");
    if shader_path.exists() {
        let shader = std::fs::read_to_string(&shader_path)
            .with_context(|| format!("Failed to read shader.frag: {}", shader_path.display()))?;
        package.shader_source = Some(shader);
    }

    let refs = collect_referenced_files_in_order(&package.config);
    let mut required_missing = Vec::new();

    for rf in &refs {
        if Path::new(&rf.path).is_absolute() {
            log::warn!(
                "Skipping absolute path '{}' (not embedded by default)",
                rf.path
            );
            continue;
        }

        let disk_path = folder.join(&rf.path);
        if !disk_path.exists() {
            if rf.required {
                required_missing.push(rf.path.clone());
            } else {
                log::warn!("Optional referenced file missing: {}", rf.path);
            }
            continue;
        }

        let data = std::fs::read(&disk_path).with_context(|| {
            format!(
                "Failed to read referenced file '{}' from {}",
                rf.path,
                disk_path.display()
            )
        })?;
        package.add_asset(rf.path.clone(), data);
    }

    if !required_missing.is_empty() {
        anyhow::bail!(
            "Missing required referenced files: {}",
            required_missing.join(", ")
        );
    }

    if let Some(default_policy) = parse_default_policy(options)? {
        package.set_entry_compression("config.toml", default_policy.clone());
        if package.shader_source.is_some() {
            package.set_entry_compression("shader.frag", default_policy.clone());
        }
        for rf in &refs {
            if Path::new(&rf.path).is_absolute() {
                continue;
            }
            package.set_entry_compression(rf.path.clone(), default_policy.clone());
        }
    }

    for spec in &options.entry_compression {
        let (path, policy) = parse_entry_override(spec)?;
        package.set_entry_compression(path, policy);
    }

    let shade_path = output
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| folder.with_extension("shade"));

    package
        .save(&shade_path)
        .with_context(|| format!("Failed to write shade package: {}", shade_path.display()))?;

    Ok(shade_path)
}

#[derive(Debug, Clone)]
struct ReferencedFile {
    path: String,
    required: bool,
}

fn collect_referenced_files_in_order(config: &ShadeConfig) -> Vec<ReferencedFile> {
    let mut ordered = Vec::<ReferencedFile>::new();
    let mut first_index = HashMap::<String, usize>::new();

    for texture in config.textures.values() {
        collect_texture_refs(texture, true, &mut ordered, &mut first_index);
    }

    for buffer in config.buffers.values() {
        push_ref(&buffer.shader, true, &mut ordered, &mut first_index);
    }

    ordered
}

fn collect_texture_refs(
    def: &TextureDef,
    parent_required: bool,
    ordered: &mut Vec<ReferencedFile>,
    first_index: &mut HashMap<String, usize>,
) {
    let required = parent_required && !def.optional;

    match def.ty {
        TextureType::Image | TextureType::Video | TextureType::Font => {
            if let Some(source) = &def.source {
                push_ref(source, required, ordered, first_index);
            }
        }
        TextureType::Shader => {
            if let Some(shader) = &def.shader {
                push_ref(shader, required, ordered, first_index);
            }
        }
        TextureType::Slideshow | TextureType::AudioSpectrum | TextureType::Noise => {}
    }

    for child in &def.sources {
        collect_texture_refs(child, required, ordered, first_index);
    }
    for child in def.textures.values() {
        collect_texture_refs(child, required, ordered, first_index);
    }
}

fn push_ref(
    path: &str,
    required: bool,
    ordered: &mut Vec<ReferencedFile>,
    first_index: &mut HashMap<String, usize>,
) {
    let normalized = path.replace('\\', "/");
    if let Some(idx) = first_index.get(&normalized) {
        if required {
            ordered[*idx].required = true;
        }
        return;
    }

    let idx = ordered.len();
    ordered.push(ReferencedFile {
        path: normalized.clone(),
        required,
    });
    first_index.insert(normalized, idx);
}

pub fn migrate_legacy_zip_to_v2_with_report(
    input: &Path,
    output: Option<&Path>,
) -> Result<(PathBuf, MigrationReport)> {
    let file = std::fs::File::open(input)
        .with_context(|| format!("Failed to open legacy .shade: {}", input.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .with_context(|| "Input is not a valid legacy ZIP .shade package")?;

    let config: ShadeConfig = {
        let mut cfg = zip
            .by_name("config.toml")
            .with_context(|| "Legacy package is missing config.toml")?;
        let mut config_str = String::new();
        cfg.read_to_string(&mut config_str)?;
        toml::from_str(&config_str).with_context(|| "Failed to parse legacy config.toml")?
    };

    let mut package = LiveShadePackage::new_empty(config);

    if let Ok(mut shader_entry) = zip.by_name("shader.frag") {
        let mut shader = String::new();
        shader_entry
            .read_to_string(&mut shader)
            .with_context(|| "Failed to read legacy shader.frag")?;
        package.shader_source = Some(shader);
    }

    for preview_name in ["preview.jpg", "preview.png", "preview.webp"] {
        if let Ok(mut p) = zip.by_name(preview_name) {
            let mut bytes = Vec::new();
            p.read_to_end(&mut bytes)?;
            package.preview = Some(bytes);
            break;
        }
    }

    let mut embedded_assets = 0usize;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if name == "config.toml"
            || name == "shader.frag"
            || name == "preview.jpg"
            || name == "preview.png"
            || name == "preview.webp"
        {
            continue;
        }

        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        package.add_asset(name, data);
        embedded_assets += 1;
    }

    let warnings = unresolved_reference_warnings_after_migration(&package.config, &package);

    let output_path = output.map(|p| p.to_path_buf()).unwrap_or_else(|| {
        let stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("migrated");
        input.with_file_name(format!("{}_v2.shade", stem))
    });
    package
        .save(&output_path)
        .with_context(|| format!("Failed to write v2 .shade: {}", output_path.display()))?;
    Ok((
        output_path,
        MigrationReport {
            embedded_assets,
            warnings,
        },
    ))
}

fn parse_default_policy(options: &PackOptions) -> Result<Option<CompressionPolicy>> {
    parse_default_compression_policy(&options.default_codec, &options.default_level)
}

fn parse_entry_override(spec: &str) -> Result<(String, CompressionPolicy)> {
    parse_entry_compression_override(spec)
}

fn unresolved_reference_warnings_after_migration(
    config: &ShadeConfig,
    package: &LiveShadePackage,
) -> Vec<String> {
    let refs = collect_referenced_files_in_order(config);
    let mut embedded = HashSet::new();
    let mut warnings = Vec::new();
    for asset in package.asset_entries() {
        embedded.insert(asset.name.replace('\\', "/"));
    }
    if package.shader_source.is_some() {
        embedded.insert("shader.frag".to_string());
    }

    for rf in refs {
        if Path::new(&rf.path).is_absolute() {
            if rf.required {
                warnings.push(format!(
                    "Migration kept required absolute path reference '{}' (not embedded)",
                    rf.path
                ));
            } else {
                warnings.push(format!(
                    "Migration kept optional absolute path reference '{}' (not embedded)",
                    rf.path
                ));
            }
            continue;
        }

        if embedded.contains(&rf.path) {
            continue;
        }

        if rf.required {
            warnings.push(format!(
                "Migration output is missing required referenced file '{}' (reference kept)",
                rf.path
            ));
        } else {
            warnings.push(format!(
                "Migration output is missing optional referenced file '{}'",
                rf.path
            ));
        }
    }

    warnings
}

#[cfg(test)]
mod pack_tests {
    use super::*;
    use kroma_shared::types::{BufferDef, RenderingConfig, ShadeMeta};

    fn base_config() -> ShadeConfig {
        ShadeConfig {
            meta: ShadeMeta {
                name: "test".into(),
                author: "test".into(),
                version: "1.0".into(),
                description: String::new(),
                tags: Vec::new(),
            },
            rendering: RenderingConfig::default(),
            uniforms: Default::default(),
            textures: Default::default(),
            buffers: Default::default(),
        }
    }

    #[test]
    fn referenced_files_keep_first_order_and_required_upgrade() {
        let mut cfg = base_config();

        cfg.textures.insert(
            "a".into(),
            TextureDef {
                ty: TextureType::Image,
                source: Some("assets/dup.png".into()),
                seed: None,
                input: None,
                sources: Vec::new(),
                looping: true,
                filter: Default::default(),
                wrap: Default::default(),
                binding: None,
                font_size: None,
                interval: None,
                shuffle: false,
                fft_bands: None,
                hot_reload: false,
                optional: true,
                shader: None,
                width: None,
                height: None,
                textures: Default::default(),
                uniforms: Default::default(),
            },
        );

        cfg.textures.insert(
            "b".into(),
            TextureDef {
                ty: TextureType::Image,
                source: Some("assets/dup.png".into()),
                seed: None,
                input: None,
                sources: Vec::new(),
                looping: true,
                filter: Default::default(),
                wrap: Default::default(),
                binding: None,
                font_size: None,
                interval: None,
                shuffle: false,
                fft_bands: None,
                hot_reload: false,
                optional: false,
                shader: None,
                width: None,
                height: None,
                textures: Default::default(),
                uniforms: Default::default(),
            },
        );

        cfg.buffers.insert(
            "A".into(),
            BufferDef {
                shader: "buffers/a.frag".into(),
                inputs: Vec::new(),
                feedback: false,
            },
        );

        let refs = collect_referenced_files_in_order(&cfg);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].path, "assets/dup.png");
        assert!(refs[0].required);
        assert_eq!(refs[1].path, "buffers/a.frag");
        assert!(refs[1].required);
    }

    #[test]
    fn parse_entry_override_supports_codecs() {
        let (_, p1) = parse_entry_override("shader.frag=auto").expect("auto");
        assert!(matches!(p1, CompressionPolicy::Auto));

        let (_, p2) = parse_entry_override("assets/v.mp4=none").expect("none");
        assert!(matches!(p2, CompressionPolicy::None));

        let (_, p3) = parse_entry_override("assets/v.mp4=lz4").expect("lz4");
        assert!(matches!(p3, CompressionPolicy::Lz4));

        let (_, p4) = parse_entry_override("shader.frag=zstd:9").expect("zstd");
        assert!(matches!(p4, CompressionPolicy::Zstd { level: 9 }));
    }

    #[test]
    fn parse_default_policy_rejects_invalid_pairings() {
        let opts = PackOptions {
            default_codec: "none".into(),
            default_level: "5".into(),
            entry_compression: Vec::new(),
        };
        assert!(parse_default_policy(&opts).is_err());
    }
}

// ---------------------------------------------------------------------------
// Shadertoy API download
// ---------------------------------------------------------------------------

/// JSON response shape from the Shadertoy API.
#[derive(Debug, serde::Deserialize)]
struct ShadertoyResponse {
    #[serde(rename = "Shader")]
    shader: Option<ShadertoyShader>,
    #[serde(rename = "Error")]
    error: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ShadertoyShader {
    info: ShadertoyInfo,
    renderpass: Vec<ShadertoyRenderpass>,
}

#[derive(Debug, serde::Deserialize)]
struct ShadertoyInfo {
    name: String,
    username: String,
    #[allow(dead_code)]
    description: String,
}

#[derive(Debug, serde::Deserialize)]
struct ShadertoyRenderpass {
    code: String,
    #[serde(rename = "type")]
    pass_type: String,
}

/// Extract a shader ID from a Shadertoy URL or bare ID.
fn extract_shader_id(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    if let Some(pos) = trimmed.rfind("/view/") {
        return trimmed[pos + 6..].to_string();
    }
    trimmed.to_string()
}

/// Download a shader from the Shadertoy API and produce a `.shade` package.
///
/// # Arguments
/// * `url_or_id` — A Shadertoy URL or just the shader ID.
/// * `output_dir` — Directory to write the `.shade` and `.glsl` files to.
/// * `api_key` — Optional API key override; checks `SHADERTOY_API_KEY` env var, then tries web scraping.
///
/// Returns `(shade_path, shader_name)`.
pub async fn download_shadertoy(
    url_or_id: &str,
    output_dir: &Path,
    api_key: Option<&str>,
) -> Result<(PathBuf, String)> {
    let shader_id = extract_shader_id(url_or_id);

    let env_key = std::env::var("SHADERTOY_API_KEY").ok();
    let key = api_key.map(|s| s.to_string()).or(env_key);

    let client = reqwest::Client::builder()
        .user_agent("Kroma/0.1")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let (glsl_source, shader_name, shader_author) = if let Some(ref api_key) = key {
        match fetch_via_api(&client, &shader_id, api_key).await {
            Ok(result) => result,
            Err(api_err) => {
                log::warn!("API fetch failed ({}), trying web scrape...", api_err);
                fetch_via_scrape(&client, &shader_id)
                    .await
                    .with_context(|| {
                        format!(
                            "Both API and scrape failed for shader {}. API error: {}",
                            shader_id, api_err
                        )
                    })?
            }
        }
    } else {
        log::info!("No Shadertoy API key set. Trying web scrape...");
        fetch_via_scrape(&client, &shader_id)
            .await
            .with_context(|| {
                format!(
                    "Web scrape failed for shader {}. Set SHADERTOY_API_KEY env var for API access.",
                    shader_id
                )
            })?
    };

    let safe_name: String = shader_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe_name = if safe_name.is_empty() {
        shader_id.clone()
    } else {
        safe_name
    };

    std::fs::create_dir_all(output_dir)?;
    let glsl_path = output_dir.join(format!("{}.glsl", safe_name));
    std::fs::write(&glsl_path, &glsl_source)
        .with_context(|| format!("Failed to write GLSL file: {}", glsl_path.display()))?;

    info!("Downloaded GLSL source: {}", glsl_path.display());

    let result = translator::translate(&glsl_source, &shader_name, &shader_author);

    for w in &result.warnings {
        log::warn!("Translator warning: {}", w);
    }

    let mut package = LiveShadePackage::new_empty(result.config);
    package.shader_source = Some(result.shader_source);

    let shade_path = output_dir.join(format!("{}.shade", safe_name));
    package
        .save(&shade_path)
        .with_context(|| format!("Failed to write shade package: {}", shade_path.display()))?;

    info!(
        "Shade package created from Shadertoy: {} ({} bytes)",
        shade_path.display(),
        std::fs::metadata(&shade_path)?.len()
    );

    Ok((shade_path, shader_name))
}

/// Fetch shader via the official Shadertoy REST API.
async fn fetch_via_api(
    client: &reqwest::Client,
    shader_id: &str,
    api_key: &str,
) -> Result<(String, String, String)> {
    let api_url = format!(
        "https://www.shadertoy.com/api/v1/shaders/{}?key={}",
        shader_id, api_key
    );

    info!("Fetching shader {} via Shadertoy API...", shader_id);

    let resp = client
        .get(&api_url)
        .send()
        .await
        .with_context(|| format!("HTTP request failed for shader {}", shader_id))?;

    if !resp.status().is_success() {
        anyhow::bail!(
            "Shadertoy API HTTP {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }

    let body: ShadertoyResponse = resp
        .json()
        .await
        .with_context(|| "Failed to parse Shadertoy API JSON")?;

    if let Some(err) = body.error {
        anyhow::bail!("Shadertoy API error: {}", err);
    }

    let shader = body
        .shader
        .ok_or_else(|| anyhow::anyhow!("No shader data in API response"))?;

    let image_pass = shader
        .renderpass
        .iter()
        .find(|p| p.pass_type == "image")
        .ok_or_else(|| anyhow::anyhow!("No 'image' renderpass found"))?;

    Ok((
        image_pass.code.clone(),
        shader.info.name.clone(),
        shader.info.username.clone(),
    ))
}

/// Fetch shader by scraping the Shadertoy web page.
async fn fetch_via_scrape(
    client: &reqwest::Client,
    shader_id: &str,
) -> Result<(String, String, String)> {
    let page_url = format!("https://www.shadertoy.com/view/{}", shader_id);

    info!("Scraping shader {} from web page...", shader_id);

    let resp = client
        .get(&page_url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch page: {}", page_url))?;

    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} fetching {}", resp.status(), page_url);
    }

    let html = resp
        .text()
        .await
        .with_context(|| "Failed to read page body")?;

    let code = extract_shader_from_html(&html, shader_id)?;
    let name = extract_title_from_html(&html).unwrap_or_else(|| shader_id.to_string());
    let author = extract_author_from_html(&html).unwrap_or_else(|| "Unknown".to_string());

    Ok((code, name, author))
}

/// Extract shader code from Shadertoy page HTML.
fn extract_shader_from_html(html: &str, shader_id: &str) -> Result<String> {
    let renderpass_re = regex::Regex::new(
        r#""renderpass"\s*:\s*\[((?:[^\[\]]|\[[^\[\]]*?\])*?"type"\s*:\s*"image"(?:[^\[\]]|\[[^\[\]]*?\])*?)\]"#
    ).expect("valid regex");

    if let Some(caps) = renderpass_re.captures(html) {
        let pass_block = caps.get(1).unwrap().as_str();

        let code_re =
            regex::Regex::new(r#""code"\s*:\s*"((?:[^"\\]|\\.)*)""#).expect("valid regex");
        if let Some(code_caps) = code_re.captures(pass_block) {
            let raw_code = code_caps.get(1).unwrap().as_str();
            let code = raw_code
                .replace("\\\\", "\x00")
                .replace("\\n", "\n")
                .replace("\\t", "\t")
                .replace("\\r", "\r")
                .replace("\\\"", "\"")
                .replace("\x00", "\\");
            return Ok(code);
        }
    }

    let alt_re = regex::Regex::new(
        r#"(?s)\{[^}]*?"code"\s*:\s*"((?:[^"\\]|\\.)*)"[^}]*?"type"\s*:\s*"image""#,
    )
    .expect("valid regex");

    if let Some(caps) = alt_re.captures(html) {
        let raw_code = caps.get(1).unwrap().as_str();
        let code = raw_code
            .replace("\\\\", "\x00")
            .replace("\\n", "\n")
            .replace("\\t", "\t")
            .replace("\\r", "\r")
            .replace("\\\"", "\"")
            .replace("\x00", "\\");
        return Ok(code);
    }

    anyhow::bail!(
        "Could not extract shader code from Shadertoy page for '{}'. \
         Set the SHADERTOY_API_KEY environment variable for reliable access. \
         Get a key at: https://www.shadertoy.com/profile (create App Key)",
        shader_id
    )
}

/// Extract shader title from page HTML.
fn extract_title_from_html(html: &str) -> Option<String> {
    let re = regex::Regex::new(r#""name"\s*:\s*"([^"]+)""#).ok()?;
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Extract author from page HTML.
fn extract_author_from_html(html: &str) -> Option<String> {
    let re = regex::Regex::new(r#""username"\s*:\s*"([^"]+)""#).ok()?;
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_id_from_url() {
        assert_eq!(
            extract_shader_id("https://www.shadertoy.com/view/XsXXDn"),
            "XsXXDn"
        );
        assert_eq!(
            extract_shader_id("https://shadertoy.com/view/4dlGDN/"),
            "4dlGDN"
        );
        assert_eq!(extract_shader_id("MdX3Rr"), "MdX3Rr");
        assert_eq!(
            extract_shader_id("  https://www.shadertoy.com/view/WtdSDs  "),
            "WtdSDs"
        );
    }
}
