//! Shadertoy importer — reads a GLSL file or downloads from Shadertoy API
//! and produces a `.shade` package.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use log::info;

use kroma_shared::shade::LiveShadePackage;
use kroma_shared::translator;

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
/// * `api_key` — Optional API key override; checks `SHADERTOY_API_KEY` env var, then tries web scraping.
///
/// Returns `(shade_path, shader_name)`.
pub async fn download_shadertoy(
    url_or_id: &str,
    output_dir: &Path,
    api_key: Option<&str>,
) -> Result<(PathBuf, String)> {
    let shader_id = extract_shader_id(url_or_id);

    // Determine API key: explicit > env var > None
    let env_key = std::env::var("SHADERTOY_API_KEY").ok();
    let key = api_key.map(|s| s.to_string()).or(env_key);

    let client = reqwest::Client::builder()
        .user_agent("Kroma/0.1")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // Try API first if we have a key, then fall back to web scraping
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

    // Sanitize name for filename
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

    // Write the raw GLSL source
    std::fs::create_dir_all(output_dir)?;
    let glsl_path = output_dir.join(format!("{}.glsl", safe_name));
    std::fs::write(&glsl_path, &glsl_source)
        .with_context(|| format!("Failed to write GLSL file: {}", glsl_path.display()))?;

    info!("Downloaded GLSL source: {}", glsl_path.display());

    // Translate and package
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
/// Falls back to this when no API key is available.
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

    // Shadertoy embeds shader JSON via: var gShaderToy = {} with shader data,
    // and the shader code is in a script with: {"renderpass":[{"code":"..."}]}
    // Try to find the JSON blob in the HTML.

    // Method 1: Look for the JSON in a script tag
    let code = extract_shader_from_html(&html, shader_id)?;
    let name = extract_title_from_html(&html).unwrap_or_else(|| shader_id.to_string());
    let author = extract_author_from_html(&html).unwrap_or_else(|| "Unknown".to_string());

    Ok((code, name, author))
}

/// Extract shader code from Shadertoy page HTML.
fn extract_shader_from_html(html: &str, shader_id: &str) -> Result<String> {
    // Shadertoy puts shader code in a JSON blob like:
    // {"info":{},"renderpass":[{"code":"shader code here",...,"type":"image"}]}
    // Often inside: <script>var gShaderToy = { ... mShaderCode: ... }</script>
    // Or: gShaderToy.SetTexture(... and the code inside JS calls

    // Strategy: find "renderpass" and extract the "code" field for "type":"image"
    // Allow one level of nested [...] to handle Shadertoy's "inputs" arrays
    let renderpass_re = regex::Regex::new(
        r#""renderpass"\s*:\s*\[((?:[^\[\]]|\[[^\[\]]*?\])*?"type"\s*:\s*"image"(?:[^\[\]]|\[[^\[\]]*?\])*?)\]"#
    ).expect("valid regex");

    if let Some(caps) = renderpass_re.captures(html) {
        let pass_block = caps.get(1).unwrap().as_str();

        // Extract the code field
        let code_re =
            regex::Regex::new(r#""code"\s*:\s*"((?:[^"\\]|\\.)*)""#).expect("valid regex");
        if let Some(code_caps) = code_re.captures(pass_block) {
            let raw_code = code_caps.get(1).unwrap().as_str();
            // Unescape JSON string escapes
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

    // Fallback: try finding the shader code in a different pattern
    // Some pages use: value="{shader code}" or similar
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
