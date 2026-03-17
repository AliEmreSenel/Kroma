//! Shadertoy → Kroma shader translator.
//!
//! Transpiles raw Shadertoy GLSL into Kroma-compliant `.frag` format by:
//! 1. Replacing Shadertoy built-in uniforms with Kroma equivalents.
//! 2. Replacing texture channel references with Kroma sampler names.
//! 3. Wrapping the `mainImage` entry point into a standard `main()`.
//! 4. Generating a skeleton `config.toml`.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::types::{ShadeConfig, ShadeMeta, TextureDef, TextureType, UniformDef};

/// Pre-compiled regex for a word-boundary match.
fn word_regex(word: &str) -> Regex {
    Regex::new(&format!(r"\b{}\b", regex::escape(word))).expect("valid regex")
}

/// Statically compiled regexes for UNIFORM_MAP and SEMANTIC_MAP replacements.
static UNIFORM_REGEXES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    UNIFORM_MAP
        .iter()
        .map(|&(from, to)| (word_regex(from), to))
        .collect()
});

static SEMANTIC_REGEXES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    SEMANTIC_MAP
        .iter()
        .map(|&(from, to)| (word_regex(from), to))
        .collect()
});

static TEXTURE_FIX_REGEXES: LazyLock<Vec<(Regex, &'static str, &'static str, &'static str)>> =
    LazyLock::new(|| {
        [
            ("texture2D", "texture"),
            ("textureCube", "texture"),
            ("texture2DLod", "textureLod"),
            ("textureCubeLod", "textureLod"),
        ]
        .iter()
        .map(|&(old, new)| (word_regex(old), new, old, new))
        .collect()
    });

static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"#version\s+\d+(\s+\w+)?\s*\n?").expect("valid regex"));

static PRECISION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bprecision\s+(lowp|mediump|highp)\s+\w+\s*;\s*\n?").expect("valid regex")
});

static HAS_MAIN_IMAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bvoid\s+mainImage\s*\(").expect("valid regex"));

static MAIN_IMAGE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"void\s+mainImage\s*\(\s*out\s+vec4\s+(\w+)\s*,\s*(?:in\s+)?vec2\s+(\w+)\s*\)")
        .expect("valid regex")
});

static CHANNEL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\biChannel(\d+)\b").expect("valid regex"));

/// Result of a successful translation.
#[derive(Debug, Clone)]
pub struct TranslationResult {
    /// The Kroma-compliant fragment shader source.
    pub shader_source: String,
    /// Auto-generated `config.toml` for the shade package.
    pub config: ShadeConfig,
    /// Warnings emitted during translation (non-fatal).
    pub warnings: Vec<String>,
}

/// Mapping from Shadertoy globals to Kroma uniforms.
/// These are simple 1:1 name replacements (same type or compatible).
const UNIFORM_MAP: &[(&str, &str)] = &[
    ("iTime", "u_time"),
    ("iGlobalTime", "u_time"), // legacy alias
    ("iTimeDelta", "u_delta_time"),
    ("iMouse", "u_mouse"),
];

/// Replacements that require wrapping (type mismatch between Shadertoy & Kroma).
/// * `iResolution` is vec3 in Shadertoy, but vec2 in Kroma → wrap as `vec3(u_resolution, 1.0)`
/// * `iFrame` is int in Shadertoy, but uint in Kroma → wrap as `int(u_frame)`
const SEMANTIC_MAP: &[(&str, &str)] = &[
    ("iResolution", "vec3(u_resolution, 1.0)"),
    ("iFrame", "int(u_frame)"),
];

/// Translate raw Shadertoy GLSL into a Kroma-compliant fragment shader.
///
/// # Arguments
/// * `source` — Raw Shadertoy GLSL code.
/// * `name` — Display name for the shader (used in `config.toml`).
/// * `author` — Author name.
pub fn translate(source: &str, name: &str, author: &str) -> TranslationResult {
    let mut warnings = Vec::new();
    let mut output = source.to_string();
    let mut detected_channels = HashSet::new();

    // ------------------------------------------------------------------
    // Early-out: detect already-translated Kroma shaders
    // ------------------------------------------------------------------
    if source.contains("kroma_main") || source.contains("kroma_out_color") {
        warnings
            .push("Shader appears to already be in Kroma format — skipping translation.".into());
        return TranslationResult {
            shader_source: source.to_string(),
            config: ShadeConfig {
                meta: ShadeMeta {
                    name: name.to_string(),
                    author: author.to_string(),
                    description: String::new(),
                    version: "1.0".to_string(),
                    tags: vec![],
                },
                textures: std::collections::HashMap::new(),
                uniforms: std::collections::HashMap::new(),
                rendering: Default::default(),
                buffers: Default::default(),
            },
            warnings,
        };
    }

    // ------------------------------------------------------------------
    // Step 1: Replace Shadertoy uniforms with Kroma equivalents
    // ------------------------------------------------------------------
    for (re, kroma_name) in UNIFORM_REGEXES.iter() {
        if re.is_match(&output) {
            output = re.replace_all(&output, *kroma_name).to_string();
        }
    }
    // Semantic replacements (type-wrapping)
    for (re, replacement) in SEMANTIC_REGEXES.iter() {
        if re.is_match(&output) {
            output = re.replace_all(&output, *replacement).to_string();
        }
    }

    // ------------------------------------------------------------------
    // Step 1.5: GLSL compatibility fixes
    // ------------------------------------------------------------------
    output = fix_mat_constructors(&output, &mut warnings);

    // Replace legacy GLSL texture functions with GLSL 450 equivalents
    for (re, _new, old, new) in TEXTURE_FIX_REGEXES.iter() {
        if re.is_match(&output) {
            output = re.replace_all(&output, *new).to_string();
            warnings.push(format!("Replaced {} with {} for GLSL 450.", old, new));
        }
    }

    // Remove any #version directives from the source (we prepend our own)
    output = VERSION_RE.replace_all(&output, "").to_string();

    // Remove any precision qualifiers (not valid in GLSL 450 with Vulkan)
    output = PRECISION_RE.replace_all(&output, "").to_string();

    // ------------------------------------------------------------------
    // Step 2: Detect and replace iChannelN texture samplers
    // ------------------------------------------------------------------
    let channel_re = &*CHANNEL_RE;
    for cap in channel_re.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            let idx: u32 = m.as_str().parse().unwrap_or(0);
            detected_channels.insert(idx);
        }
    }

    // Sort detected channels so binding indices are deterministic
    let mut sorted_channels: Vec<u32> = detected_channels.iter().copied().collect();
    sorted_channels.sort();

    // Replace iChannelN with sampler2D(kroma_tex_I, kroma_samp_I) expressions
    // where I is the sequential binding index (position in sorted channels).
    for (binding_idx, idx) in sorted_channels.iter().enumerate() {
        let from = format!("iChannel{}", idx);
        let to = format!(
            "sampler2D(kroma_tex_{}, kroma_samp_{})",
            binding_idx, binding_idx
        );
        let re = word_regex(&from);
        output = re.replace_all(&output, to.as_str()).to_string();
    }

    // ------------------------------------------------------------------
    // Step 3: Replace texture() / texture2D() calls referencing old names
    //         (already handled by uniform replacement above)
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // Step 4: Wrap mainImage() → main()
    // ------------------------------------------------------------------
    let has_main_image = HAS_MAIN_IMAGE_RE.is_match(&output);

    if has_main_image {
        // Replace the mainImage signature
        // Note: `in` qualifier is optional — many Shadertoy shaders omit it
        let main_image_re = &*MAIN_IMAGE_RE;

        if let Some(caps) = main_image_re.captures(&output) {
            let frag_color_name = caps
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "fragColor".into());
            let frag_coord_name = caps
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "fragCoord".into());

            // Replace signature with void kroma_main()
            output = main_image_re
                .replace(&output, "void kroma_main()")
                .to_string();

            // Prepend local variable declarations inside the function body
            // Y-flip: Vulkan/wgpu has top-left origin, Shadertoy expects bottom-left
            let locals = format!(
                "    vec2 {} = vec2(gl_FragCoord.x, u_resolution.y - gl_FragCoord.y);\n    vec4 {} = vec4(0.0);\n",
                frag_coord_name, frag_color_name
            );

            // Find the opening brace of kroma_main and inject locals
            if let Some(brace_pos) = output.find("void kroma_main()") {
                if let Some(open) = output[brace_pos..].find('{') {
                    let abs_open = brace_pos + open + 1;
                    let (before, after) = output.split_at(abs_open);
                    output = format!("{}\n{}{}", before, locals, after);
                }

                // Append output assignment before the closing brace of kroma_main.
                // Find the matching closing brace by counting depth from the opening brace.
                if let Some(main_pos) = output.find("void kroma_main()")
                    && let Some(open_rel) = output[main_pos..].find('{')
                {
                    let open_abs = main_pos + open_rel;
                    let mut depth = 0;
                    let mut close_pos = None;
                    for (i, ch) in output[open_abs..].char_indices() {
                        match ch {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    close_pos = Some(open_abs + i);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some(pos) = close_pos {
                        let writeback = format!("    kroma_out_color = {};\n", frag_color_name);
                        output.insert_str(pos, &writeback);
                    }
                }
            }
        } else {
            warnings.push("Found mainImage but could not parse its signature.".into());
        }
    } else {
        warnings.push("No mainImage() found — shader may already be in standard format.".into());
    }

    // ------------------------------------------------------------------
    // Step 5: Prepend Kroma uniform declarations (GLSL 450 compatible)
    // ------------------------------------------------------------------
    let header = r#"// =========================================
// Auto-generated by Kroma Shader Translator
// =========================================
#version 450

// Kroma uniform block — matches ShaderUniforms layout
layout(set = 0, binding = 0) uniform Globals {
    float u_time;
    float u_delta_time;
    uint  u_frame;
    uint  _pad0;
    vec2  u_resolution;
    vec2  _pad1;
    vec4  u_mouse;
    float u_cpu;
    float u_ram;
    float u_battery;
    float u_audio_level;
};

// Custom uniform storage buffer — access via custom_data[index]
layout(set = 0, binding = 1) readonly buffer CustomUniforms {
    float custom_data[32];
};

// Fragment output
layout(location = 0) out vec4 kroma_out_color;
"#;

    // Add separate texture2D + sampler declarations for each channel.
    // Using (binding_idx*2, binding_idx*2+1) pairs to match the renderer's
    // build_texture_bind_group() layout.
    let mut sampler_decls = String::new();
    for (binding_idx, _idx) in sorted_channels.iter().enumerate() {
        let tex_binding = binding_idx * 2;
        let samp_binding = binding_idx * 2 + 1;
        sampler_decls.push_str(&format!(
            "layout(set = 1, binding = {}) uniform texture2D kroma_tex_{};\n",
            tex_binding, binding_idx
        ));
        sampler_decls.push_str(&format!(
            "layout(set = 1, binding = {}) uniform sampler kroma_samp_{};\n",
            samp_binding, binding_idx
        ));
    }

    output = format!("{}{}\n{}", header, sampler_decls, output);

    // Append main() entry point that calls kroma_main()
    if has_main_image {
        output.push_str("\nvoid main() {\n    kroma_main();\n}\n");
    }

    // ------------------------------------------------------------------
    // Step 6: Generate config.toml
    // ------------------------------------------------------------------
    let mut textures = std::collections::HashMap::new();
    for idx in &detected_channels {
        let channel_name = format!("channel{}", idx);
        textures.insert(
            channel_name,
            TextureDef {
                ty: TextureType::Image,
                source: Some(format!("assets/channel{}.png", idx)),
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
                textures: std::collections::HashMap::new(),
                uniforms: std::collections::HashMap::new(),
            },
        );
    }

    let config = ShadeConfig {
        meta: ShadeMeta {
            name: name.to_string(),
            author: author.to_string(),
            version: "1.0".into(),
            description: String::new(),
            tags: Vec::new(),
        },
        rendering: Default::default(),
        uniforms: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "speed".into(),
                UniformDef {
                    ty: "float".into(),
                    min: Some(0.1),
                    max: Some(5.0),
                    default: Some(toml::Value::Float(1.0)),
                },
            );
            m
        },
        textures,
        buffers: Default::default(),
    };

    TranslationResult {
        shader_source: output,
        config,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_SHADERTOY: &str = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    fragColor = vec4(uv, 0.5 + 0.5 * sin(iTime), 1.0);
}
"#;

    #[test]
    fn translates_simple_shader() {
        let result = translate(SIMPLE_SHADERTOY, "Test Shader", "Tester");
        assert!(result.shader_source.contains("u_time"));
        assert!(result.shader_source.contains("u_resolution"));
        assert!(result.shader_source.contains("kroma_main"));
        assert!(!result.shader_source.contains("iTime"));
        assert!(!result.shader_source.contains("iResolution"));
    }

    #[test]
    fn detects_channels() {
        let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec4 tex = texture(iChannel0, fragCoord / iResolution.xy);
    fragColor = tex;
}
"#;
        let result = translate(src, "Channel Test", "Tester");
        // Should use separate texture2D + sampler, combined via sampler2D()
        assert!(result.shader_source.contains("kroma_tex_0"));
        assert!(result.shader_source.contains("kroma_samp_0"));
        assert!(
            result
                .shader_source
                .contains("sampler2D(kroma_tex_0, kroma_samp_0)")
        );
        assert!(
            result
                .shader_source
                .contains("uniform texture2D kroma_tex_0")
        );
        assert!(
            result
                .shader_source
                .contains("uniform sampler kroma_samp_0")
        );
        assert!(result.config.textures.contains_key("channel0"));
    }

    #[test]
    fn generates_uniform_header() {
        let result = translate(SIMPLE_SHADERTOY, "Test", "T");
        assert!(
            result
                .shader_source
                .contains("layout(set = 0, binding = 0) uniform Globals")
        );
        assert!(
            result
                .shader_source
                .contains("layout(set = 0, binding = 1) readonly buffer CustomUniforms")
        );
        assert!(result.shader_source.contains("float custom_data[32]"));
        assert!(
            result
                .shader_source
                .contains("layout(location = 0) out vec4 kroma_out_color")
        );
        assert!(result.shader_source.contains("float u_time;"));
        assert!(result.shader_source.contains("void main()"));
    }
}

// ---------------------------------------------------------------------------
// GLSL compatibility fixups for naga
// ---------------------------------------------------------------------------

/// Fix `mat2(single_vec4_expr)` → `mat2(v.x, v.y, v.z, v.w)` with a
/// helper variable.  naga's GLSL frontend cannot construct mat2 from a
/// single vec4 argument.
fn fix_mat_constructors(src: &str, warnings: &mut Vec<String>) -> String {
    let mut output = src.to_string();
    let mut counter = 0u32;
    let mut search_from = 0usize;

    let mat2_needle = "mat2(";

    while let Some(p) = output[search_from..].find(mat2_needle) {
        let pos = search_from + p;

        // Make sure this isn't part of a longer identifier (e.g. imat2)
        if pos > 0 {
            let prev = output.as_bytes()[pos - 1];
            if prev.is_ascii_alphanumeric() || prev == b'_' {
                search_from = pos + mat2_needle.len();
                continue;
            }
        }

        let inner_start = pos + mat2_needle.len();

        // Find the matching closing paren by counting depth
        let mut depth = 1i32;
        let mut end = inner_start;
        let mut found_close = false;
        for (i, ch) in output[inner_start..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = inner_start + i;
                        found_close = true;
                        break;
                    }
                }
                _ => {}
            }
        }

        if !found_close {
            break; // unbalanced parens, stop
        }

        let inner = output[inner_start..end].to_string();

        // Count top-level commas (commas at depth 0 inside the mat2 args)
        let mut comma_depth = 0i32;
        let mut top_commas = 0;
        for ch in inner.chars() {
            match ch {
                '(' => comma_depth += 1,
                ')' => comma_depth -= 1,
                ',' if comma_depth == 0 => top_commas += 1,
                _ => {}
            }
        }

        if top_commas == 0 && !inner.trim().is_empty() {
            // Single argument — likely a vec4 expression.
            // Rewrite: mat2(expr) → mat2(_kmN_[0], _kmN_[1], _kmN_[2], _kmN_[3])
            // with `vec4 _kmN_ = expr;` inserted before the statement.
            let var = format!("_km{}_", counter);
            counter += 1;
            let replacement = format!("mat2({v}[0], {v}[1], {v}[2], {v}[3])", v = var);

            // Find the statement start (work backwards to find ; or { or newline)
            let stmt_start = output[..pos]
                .rfind([';', '{', '\n'])
                .map(|p| p + 1)
                .unwrap_or(0);

            let indent: String = output[stmt_start..pos]
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect();

            let helper = format!("{}vec4 {} = {};\n", indent, var, inner.trim());

            // Build the new output
            let before = &output[..stmt_start];
            let between = &output[stmt_start..pos];
            let after = &output[end + 1..];
            output = format!("{}{}{}{}{}", before, helper, between, replacement, after);

            warnings.push("Rewrote mat2(vec4) for naga compatibility.".into());

            // Reset search to beginning — indices shifted after insertion
            search_from = 0;
            continue;
        }

        // This mat2() has multiple args — skip past it
        search_from = end + 1;
    }

    output
}
