//! Integration tests for the shared crate.

use kroma_shared::ipc::{DaemonCommand, DaemonEvent, UniformValue};
use kroma_shared::translator;
use kroma_shared::types::{ShadeConfig, ShaderUniforms, SystemStats};

// -----------------------------------------------------------------------
// IPC serialization tests
// -----------------------------------------------------------------------

#[test]
fn daemon_command_load_roundtrip() {
    let cmd = DaemonCommand::LoadShade {
        path: "/home/user/my_shader.shade".into(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    let deserialized: DaemonCommand = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonCommand::LoadShade { path } => {
            assert_eq!(path, "/home/user/my_shader.shade");
        }
        _ => panic!("Expected LoadShade"),
    }
}

#[test]
fn daemon_command_set_uniform_float() {
    let cmd = DaemonCommand::SetUniform {
        name: "speed".into(),
        value: UniformValue::Float(2.5),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("speed"));
    assert!(json.contains("2.5"));
}

#[test]
fn daemon_command_pause_resume() {
    let pause = serde_json::to_string(&DaemonCommand::Pause).unwrap();
    let resume = serde_json::to_string(&DaemonCommand::Resume).unwrap();
    assert!(pause.contains("Pause"));
    assert!(resume.contains("Resume"));
}

#[test]
fn daemon_event_status_roundtrip() {
    let event = DaemonEvent::Status {
        fps: 59.8,
        paused: false,
        loaded_shade: Some("Cyber Rain".into()),
    };
    let json = serde_json::to_string(&event).unwrap();
    let deserialized: DaemonEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonEvent::Status {
            fps,
            paused,
            loaded_shade,
        } => {
            assert!((fps - 59.8).abs() < 0.01);
            assert!(!paused);
            assert_eq!(loaded_shade.unwrap(), "Cyber Rain");
        }
        _ => panic!("Expected Status"),
    }
}

// -----------------------------------------------------------------------
// Shader uniforms tests
// -----------------------------------------------------------------------

#[test]
fn shader_uniforms_apply_system_stats() {
    let mut u = ShaderUniforms::default();
    let stats = SystemStats {
        cpu_usage: 50.0,
        ram_total: 16_000_000_000,
        ram_used: 12_000_000_000,
        battery: Some(80.0),
    };
    u.apply_system_stats(&stats);
    assert!((u.u_cpu - 0.5).abs() < 0.001);
    assert!((u.u_ram - 0.75).abs() < 0.001);
    assert!((u.u_battery - 0.8).abs() < 0.001);
}

#[test]
fn shader_uniforms_no_battery() {
    let mut u = ShaderUniforms::default();
    let stats = SystemStats {
        cpu_usage: 10.0,
        ram_total: 8_000_000_000,
        ram_used: 4_000_000_000,
        battery: None,
    };
    u.apply_system_stats(&stats);
    assert!(u.u_battery < 0.0); // -1.0 signals no battery
}

// -----------------------------------------------------------------------
// Translator tests (extended)
// -----------------------------------------------------------------------

#[test]
fn translator_preserves_non_shadertoy_code() {
    let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    float x = 42.0;
    vec2 custom = vec2(1.0, 2.0);
    fragColor = vec4(x, custom, 1.0);
}
"#;
    let result = translator::translate(src, "Custom", "Author");
    // Custom code should be preserved
    assert!(result.shader_source.contains("42.0"));
    assert!(result.shader_source.contains("custom"));
}

#[test]
fn translator_handles_legacy_iglobaltime() {
    let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    float t = iGlobalTime;
    fragColor = vec4(sin(t), 0.0, 0.0, 1.0);
}
"#;
    let result = translator::translate(src, "Legacy", "Author");
    assert!(result.shader_source.contains("u_time"));
    assert!(!result.shader_source.contains("iGlobalTime"));
}

#[test]
fn translator_multiple_channels() {
    let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec4 a = texture(iChannel0, uv);
    vec4 b = texture(iChannel1, uv);
    vec4 c = texture(iChannel2, uv);
    fragColor = a + b + c;
}
"#;
    let result = translator::translate(src, "Multi Channel", "Author");
    // Should use separate texture2D + sampler with sampler2D() combinator
    assert!(result
        .shader_source
        .contains("sampler2D(kroma_tex_0, kroma_samp_0)"));
    assert!(result
        .shader_source
        .contains("sampler2D(kroma_tex_1, kroma_samp_1)"));
    assert!(result
        .shader_source
        .contains("sampler2D(kroma_tex_2, kroma_samp_2)"));
    assert!(result.config.textures.contains_key("channel0"));
    assert!(result.config.textures.contains_key("channel1"));
    assert!(result.config.textures.contains_key("channel2"));
}

// -----------------------------------------------------------------------
// Shade config parsing tests
// -----------------------------------------------------------------------

#[test]
fn parse_shade_config() {
    let toml_str = r#"
[meta]
name = "Test Wallpaper"
author = "Tester"
version = "2.0"

[uniforms]
speed = { type = "float", min = 0.1, max = 5.0, default = 1.5 }
enabled = { type = "bool", default = true }

[textures]
channel0 = { type = "video", source = "assets/loop.mp4", loop = true }
"#;
    let config: ShadeConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.meta.name, "Test Wallpaper");
    assert_eq!(config.meta.version, "2.0");
    assert!(config.uniforms.contains_key("speed"));
    assert!(config.uniforms.contains_key("enabled"));
    assert!(config.textures.contains_key("channel0"));
    assert!(config.textures["channel0"].looping);
}
