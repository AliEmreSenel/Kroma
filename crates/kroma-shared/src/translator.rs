//! Shadertoy → Kroma shader translator.
//!
//! Transpiles raw Shadertoy GLSL into Kroma-compliant `.frag` format by:
//! 1. Replacing Shadertoy built-in uniforms with Kroma equivalents.
//! 2. Replacing texture channel references with Kroma sampler names.
//! 3. Wrapping the `mainImage` entry point into a standard `main()`.
//! 4. Generating a skeleton `config.toml`.

use std::collections::HashSet;

use regex::Regex;

use crate::types::{ShadeConfig, ShadeMeta, TextureDef, UniformDef};

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
const UNIFORM_MAP: &[(&str, &str)] = &[
    ("iTime", "u_time"),
    ("iGlobalTime", "u_time"), // legacy alias
    ("iTimeDelta", "u_delta_time"),
    ("iResolution", "u_resolution"),
    ("iMouse", "u_mouse"),
    ("iFrame", "u_frame"),
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
    // Step 1: Replace Shadertoy uniforms with Kroma equivalents
    // ------------------------------------------------------------------
    for &(shadertoy_name, kroma_name) in UNIFORM_MAP {
        let re = Regex::new(&format!(r"\b{}\b", regex::escape(shadertoy_name)))
            .expect("valid regex");
        if re.is_match(&output) {
            output = re.replace_all(&output, kroma_name).to_string();
        }
    }

    // ------------------------------------------------------------------
    // Step 2: Detect and replace iChannelN texture samplers
    // ------------------------------------------------------------------
    let channel_re = Regex::new(r"\biChannel(\d+)\b").expect("valid regex");
    for cap in channel_re.captures_iter(source) {
        if let Some(m) = cap.get(1) {
            let idx: u32 = m.as_str().parse().unwrap_or(0);
            detected_channels.insert(idx);
        }
    }

    // Replace iChannelN with kroma sampler names
    for idx in &detected_channels {
        let from = format!("iChannel{}", idx);
        let to = if *idx == 0 {
            "kroma_video_sampler_0".to_string()
        } else {
            format!("kroma_sampler_{}", idx)
        };
        let re = Regex::new(&format!(r"\b{}\b", regex::escape(&from))).expect("valid regex");
        output = re.replace_all(&output, to.as_str()).to_string();
    }

    // ------------------------------------------------------------------
    // Step 3: Replace texture() / texture2D() calls referencing old names
    //         (already handled by uniform replacement above)
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // Step 4: Wrap mainImage() → main()
    // ------------------------------------------------------------------
    let has_main_image = Regex::new(r"\bvoid\s+mainImage\s*\(")
        .expect("valid regex")
        .is_match(&output);

    if has_main_image {
        // Replace the mainImage signature
        let main_image_re =
            Regex::new(r"void\s+mainImage\s*\(\s*out\s+vec4\s+(\w+)\s*,\s*in\s+vec2\s+(\w+)\s*\)")
                .expect("valid regex");

        if let Some(caps) = main_image_re.captures(&output) {
            let frag_color_name = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or_else(|| "fragColor".into());
            let frag_coord_name = caps.get(2).map(|m| m.as_str().to_string()).unwrap_or_else(|| "fragCoord".into());

            // Replace signature with void kroma_main()
            output = main_image_re
                .replace(&output, "void kroma_main()")
                .to_string();

            // Prepend local variable declarations inside the function body
            let locals = format!(
                "    vec2 {} = gl_FragCoord.xy;\n    vec4 {} = vec4(0.0);\n",
                frag_coord_name, frag_color_name
            );

            // Find the opening brace of kroma_main and inject locals
            if let Some(brace_pos) = output.find("void kroma_main()") {
                if let Some(open) = output[brace_pos..].find('{') {
                    let abs_open = brace_pos + open + 1;
                    let (before, after) = output.split_at(abs_open);
                    output = format!("{}\n{}{}", before, locals, after);
                }

                // Append output assignment before the closing brace
                if let Some(last_brace) = output.rfind('}') {
                    let writeback =
                        format!("    kroma_out_color = {};\n", frag_color_name);
                    output.insert_str(last_brace, &writeback);
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
    vec2  u_mouse;
    float u_cpu;
    float u_ram;
    float u_battery;
    float u_audio_level;
};

// Fragment output
layout(location = 0) out vec4 kroma_out_color;
"#;

    // Add sampler uniforms for detected channels (set = 1, binding = N)
    let mut sampler_decls = String::new();
    let mut sorted_channels: Vec<u32> = detected_channels.iter().copied().collect();
    sorted_channels.sort();
    for (binding, idx) in sorted_channels.iter().enumerate() {
        if *idx == 0 {
            sampler_decls.push_str(&format!(
                "layout(set = 1, binding = {}) uniform sampler2D kroma_video_sampler_0;\n",
                binding
            ));
        } else {
            sampler_decls.push_str(&format!(
                "layout(set = 1, binding = {}) uniform sampler2D kroma_sampler_{};\n",
                binding, idx
            ));
        }
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
                ty: if *idx == 0 {
                    "video".into()
                } else {
                    "image".into()
                },
                source: Some(format!("assets/channel{}.mp4", idx)),
                looping: true,
            },
        );
    }

    let config = ShadeConfig {
        meta: ShadeMeta {
            name: name.to_string(),
            author: author.to_string(),
            version: "1.0".into(),
        },
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
        assert!(result.shader_source.contains("kroma_video_sampler_0"));
        assert!(result.config.textures.contains_key("channel0"));
    }

    #[test]
    fn generates_uniform_header() {
        let result = translate(SIMPLE_SHADERTOY, "Test", "T");
        assert!(result.shader_source.contains("layout(set = 0, binding = 0) uniform Globals"));
        assert!(result.shader_source.contains("layout(location = 0) out vec4 kroma_out_color"));
        assert!(result.shader_source.contains("float u_time;"));
        assert!(result.shader_source.contains("void main()"));
    }
}
