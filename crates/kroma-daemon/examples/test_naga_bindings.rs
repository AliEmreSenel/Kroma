// Quick test to check naga's split of combined image samplers
use kroma_shared;

fn main() {
    // Test 1: Single sampler
    test_glsl(
        "single_sampler",
        r#"#version 450
layout(set = 0, binding = 0) uniform Globals {
    float u_time;
};
layout(set = 1, binding = 0) uniform sampler2D tex0;
layout(location = 0) out vec4 fragColor;

void main() {
    fragColor = texture(tex0, vec2(0.5));
}
"#,
    );

    // Test 2: Two samplers
    test_glsl(
        "two_samplers",
        r#"#version 450
layout(set = 0, binding = 0) uniform Globals {
    float u_time;
};
layout(set = 1, binding = 0) uniform sampler2D tex0;
layout(set = 1, binding = 1) uniform sampler2D tex1;
layout(location = 0) out vec4 fragColor;

void main() {
    fragColor = texture(tex0, vec2(0.5)) + texture(tex1, vec2(0.5));
}
"#,
    );

    // Test 3: The actual translated output from Kroma translator
    let translated = kroma_shared::translator::translate(
        r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec4 a = texture(iChannel0, uv);
    vec4 b = texture(iChannel1, uv);
    fragColor = a + b;
}
"#,
        "Test",
        "Author",
    );
    println!("\n=== Translated GLSL (first 50 lines) ===");
    for (i, line) in translated.shader_source.lines().take(50).enumerate() {
        println!("  {:>3}: {}", i + 1, line);
    }
    test_glsl("translated_multichannel", &translated.shader_source);
}

fn test_glsl(label: &str, glsl: &str) {
    println!("\n=== {} ===", label);

    let compiler = shaderc::Compiler::new().unwrap();
    let mut options = shaderc::CompileOptions::new().unwrap();
    options.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_0 as u32,
    );
    options.set_source_language(shaderc::SourceLanguage::GLSL);
    options.set_target_spirv(shaderc::SpirvVersion::V1_0);
    options.set_auto_bind_uniforms(false);

    let binary = match compiler.compile_into_spirv(
        glsl,
        shaderc::ShaderKind::Fragment,
        "test.frag",
        "main",
        Some(&options),
    ) {
        Ok(b) => b,
        Err(e) => {
            println!("  shaderc compile FAILED: {}", e);
            return;
        }
    };

    if binary.get_num_warnings() > 0 {
        println!("  shaderc warnings: {}", binary.get_warning_messages());
    }

    let spirv_bytes = binary.as_binary();

    let spv_options = naga::front::spv::Options {
        adjust_coordinate_space: false,
        strict_capabilities: false,
        block_ctx_dump_prefix: None,
    };

    let module =
        match naga::front::spv::parse_u8_slice(bytemuck::cast_slice(spirv_bytes), &spv_options) {
            Ok(m) => m,
            Err(e) => {
                println!("  naga SPIR-V parse FAILED: {}", e);
                return;
            }
        };

    // Print all global variables with their bindings
    for (_handle, var) in module.global_variables.iter() {
        if let Some(ref binding) = var.binding {
            println!(
                "  var: name={:?}, group={}, binding={}, space={:?}",
                var.name, binding.group, binding.binding, var.space
            );
        }
    }

    // Also generate WGSL
    use naga::valid::{Capabilities, ValidationFlags, Validator};
    let mut validator = Validator::new(ValidationFlags::all(), Capabilities::all());
    let info = match validator.validate(&module) {
        Ok(i) => i,
        Err(e) => {
            println!("  naga validation FAILED: {}", e);
            return;
        }
    };
    let wgsl =
        naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
            .unwrap();
    println!("  --- Generated WGSL ---");
    for line in wgsl.lines() {
        if line.contains("@group") || line.contains("@binding") || line.contains("var") {
            println!("  {}", line);
        }
    }
}
