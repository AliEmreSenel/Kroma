//! Shadertoy importer — reads a GLSL file or downloads from Shadertoy API
//! and produces a `.shade` package.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::info;

use kroma_shared::shade::ShadePackage;
use kroma_shared::translator;

/// The default Shadertoy API key (public / guest key).
const SHADERTOY_API_KEY: &str = "BdHjRn";

/// Import a Shadertoy GLSL file and write a `.shade` package next to it.
///
/// Returns the path to the produced `.shade` file.
pub fn import_shadertoy_file(
    glsl_path: &Path,
    name: &str,
    author: &str,
) -> Result<PathBuf> {
    let source = std::fs::read_to_string(glsl_path)
        .with_context(|| format!("Failed to read shader file: {}", glsl_path.display()))?;

    let result = translator::translate(&source, name, author);

    for w in &result.warnings {
        log::warn!("Translator warning: {}", w);
    }

    let package = ShadePackage {
        config: result.config,
        shader_source: result.shader_source,
        preview: None,
        assets: Vec::new(),
    };

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
///
/// Accepts:
/// - `https://www.shadertoy.com/view/XsXXDn`
/// - `https://shadertoy.com/view/XsXXDn`
/// - `XsXXDn`
fn extract_shader_id(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    // Try to find /view/ID pattern
    if let Some(pos) = trimmed.rfind("/view/") {
        return trimmed[pos + 6..].to_string();
    }
    // Otherwise treat the whole thing as an ID
    trimmed.to_string()
}

/// Download a shader from the Shadertoy API and produce a `.shade` package.
///
/// # Arguments
/// * `url_or_id` — A Shadertoy URL (`https://www.shadertoy.com/view/...`) or just the shader ID.
/// * `output_dir` — Directory to write the `.shade` and `.glsl` files to.
/// * `api_key` — Optional API key override; uses `BdHjRn` by default.
///
/// Returns `(shade_path, shader_name)`.
pub async fn download_shadertoy(
    url_or_id: &str,
    output_dir: &Path,
    api_key: Option<&str>,
) -> Result<(PathBuf, String)> {
    let shader_id = extract_shader_id(url_or_id);
    let key = api_key.unwrap_or(SHADERTOY_API_KEY);

    let api_url = format!(
        "https://www.shadertoy.com/api/v1/shaders/{}?key={}",
        shader_id, key
    );

    info!("Fetching shader {} from Shadertoy API...", shader_id);

    let client = reqwest::Client::new();
    let resp = client
        .get(&api_url)
        .send()
        .await
        .with_context(|| format!("HTTP request failed for shader {}", shader_id))?;

    if !resp.status().is_success() {
        anyhow::bail!(
            "Shadertoy API returned HTTP {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }

    let body: ShadertoyResponse = resp
        .json()
        .await
        .with_context(|| "Failed to parse Shadertoy API JSON response")?;

    if let Some(err) = body.error {
        anyhow::bail!("Shadertoy API error: {}", err);
    }

    let shader = body
        .shader
        .ok_or_else(|| anyhow::anyhow!("No shader data in API response"))?;

    // Find the "image" renderpass (the main fragment shader)
    let image_pass = shader
        .renderpass
        .iter()
        .find(|p| p.pass_type == "image")
        .ok_or_else(|| anyhow::anyhow!("No 'image' renderpass found in shader"))?;

    let glsl_source = &image_pass.code;
    let shader_name = &shader.info.name;
    let shader_author = &shader.info.username;

    // Sanitize name for filename
    let safe_name: String = shader_name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    let safe_name = if safe_name.is_empty() {
        shader_id.clone()
    } else {
        safe_name
    };

    // Write the raw GLSL source
    std::fs::create_dir_all(output_dir)?;
    let glsl_path = output_dir.join(format!("{}.glsl", safe_name));
    std::fs::write(&glsl_path, glsl_source)
        .with_context(|| format!("Failed to write GLSL file: {}", glsl_path.display()))?;

    info!("Downloaded GLSL source: {}", glsl_path.display());

    // Translate and package
    let result = translator::translate(glsl_source, shader_name, shader_author);

    for w in &result.warnings {
        log::warn!("Translator warning: {}", w);
    }

    let package = ShadePackage {
        config: result.config,
        shader_source: result.shader_source,
        preview: None,
        assets: Vec::new(),
    };

    let shade_path = output_dir.join(format!("{}.shade", safe_name));
    package
        .save(&shade_path)
        .with_context(|| format!("Failed to write shade package: {}", shade_path.display()))?;

    info!(
        "Shade package created from Shadertoy: {} ({} bytes)",
        shade_path.display(),
        std::fs::metadata(&shade_path)?.len()
    );

    Ok((shade_path, shader_name.clone()))
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
