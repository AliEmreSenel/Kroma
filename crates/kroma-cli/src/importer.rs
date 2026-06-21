//! Shadertoy importer — reads a GLSL file or downloads from Shadertoy API
//! and produces a `.shade` package.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::info;

use kroma_shared::compression::{
    parse_default_compression_policy, parse_entry_compression_override,
};
use kroma_shared::shade::{CompressionPolicy, LiveShadePackage};
use kroma_shared::translator;
use kroma_shared::types::{ShadeConfig, ShadeStateDef, TextureDef, TextureType, TransitionDef};

#[derive(Debug, Clone)]
pub struct PackOptions {
    pub default_codec: String,
    pub default_level: String,
    pub entry_compression: Vec<String>,
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
    package.add_asset("shader.frag".to_string(), result.shader_source.into_bytes());

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
/// - files referenced by state blocks are embedded as assets
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

    if !config.states.has_any() {
        anyhow::bail!("config.toml does not define any [states.*] blocks");
    }

    let mut package = LiveShadePackage::new_empty(config);

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

    for state in [
        config.states.load.as_ref(),
        config.states.active.as_ref(),
        config.states.unload.as_ref(),
    ] {
        let Some(state) = state else {
            continue;
        };
        collect_state_refs(state, &mut ordered, &mut first_index);
    }

    for transition in config.transitions.values() {
        collect_transition_refs(transition, &mut ordered, &mut first_index);
    }

    ordered
}

fn collect_transition_refs(
    transition: &TransitionDef,
    ordered: &mut Vec<ReferencedFile>,
    first_index: &mut HashMap<String, usize>,
) {
    push_ref(&transition.shader, true, ordered, first_index);

    for texture in transition.textures.values() {
        collect_texture_refs(texture, true, ordered, first_index);
    }

    for buffer in transition.buffers.values() {
        push_ref(&buffer.shader, true, ordered, first_index);
    }
}

fn collect_state_refs(
    state: &ShadeStateDef,
    ordered: &mut Vec<ReferencedFile>,
    first_index: &mut HashMap<String, usize>,
) {
    if let Some(shader) = &state.shader {
        push_ref(shader, true, ordered, first_index);
    }

    for texture in state.textures.values() {
        collect_texture_refs(texture, true, ordered, first_index);
    }

    for buffer in state.buffers.values() {
        push_ref(&buffer.shader, true, ordered, first_index);
    }
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

fn parse_default_policy(options: &PackOptions) -> Result<Option<CompressionPolicy>> {
    parse_default_compression_policy(&options.default_codec, &options.default_level)
}

fn parse_entry_override(spec: &str) -> Result<(String, CompressionPolicy)> {
    parse_entry_compression_override(spec)
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
            states: Default::default(),
            transitions: Default::default(),
            transitions_usage: Default::default(),
        }
    }

    #[test]
    fn referenced_files_keep_first_order_and_required_upgrade() {
        let mut cfg = base_config();
        cfg.states.active = Some(ShadeStateDef {
            length: 0.0,
            shader: None,
            uniforms: Default::default(),
            textures: Default::default(),
            buffers: Default::default(),
        });

        let active = cfg
            .states
            .active
            .as_mut()
            .expect("active state should exist");

        active.textures.insert(
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
                transition: None,
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

        active.textures.insert(
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
                transition: None,
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

        active.buffers.insert(
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

    #[test]
    fn referenced_files_include_state_phase_assets() {
        let mut cfg = base_config();
        cfg.states.active = Some(ShadeStateDef {
            length: 4.0,
            shader: Some("assets/shaders/active.frag".into()),
            uniforms: Default::default(),
            textures: Default::default(),
            buffers: Default::default(),
        });

        if let Some(active) = cfg.states.active.as_mut() {
            active.textures.insert(
                "channel0".into(),
                TextureDef {
                    ty: TextureType::Image,
                    source: Some("assets/img/background.png".into()),
                    seed: None,
                    input: None,
                    sources: Vec::new(),
                    looping: true,
                    filter: Default::default(),
                    wrap: Default::default(),
                    binding: None,
                    font_size: None,
                    interval: None,
                    transition: None,
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

            active.buffers.insert(
                "A".into(),
                BufferDef {
                    shader: "assets/buffers/a.frag".into(),
                    inputs: Vec::new(),
                    feedback: false,
                },
            );
        }

        let refs = collect_referenced_files_in_order(&cfg);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].path, "assets/shaders/active.frag");
        assert_eq!(refs[1].path, "assets/img/background.png");
        assert_eq!(refs[2].path, "assets/buffers/a.frag");
        assert!(refs.iter().all(|r| r.required));
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
    package.add_asset("shader.frag".to_string(), result.shader_source.into_bytes());

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
