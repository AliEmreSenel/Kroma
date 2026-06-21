//! Integration tests for the shared crate.

use kroma_shared::ipc::{
    DaemonCommand, DaemonEvent, DaemonPhase, DaemonWaitReason, LoadRejectCode, UniformValue,
};
use kroma_shared::translator;
use kroma_shared::types::{ShadeConfig, ShaderUniforms, SystemStats};

// -----------------------------------------------------------------------
// IPC serialization tests
// -----------------------------------------------------------------------

#[test]
fn daemon_command_load_roundtrip() {
    let cmd = DaemonCommand::LoadShade {
        path: "/home/user/my_shader.shade".into(),
        force: false,
        transition: None,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    let deserialized: DaemonCommand = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonCommand::LoadShade {
            path,
            force,
            transition,
        } => {
            assert_eq!(path, "/home/user/my_shader.shade");
            assert!(!force);
            assert!(transition.is_none());
        }
        _ => panic!("Expected LoadShade"),
    }
}

#[test]
fn daemon_command_load_roundtrip_with_transition() {
    let cmd = DaemonCommand::LoadShade {
        path: "/home/user/next.shade".into(),
        force: true,
        transition: Some("kroma.fade:0.6".into()),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    let deserialized: DaemonCommand = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonCommand::LoadShade {
            path,
            force,
            transition,
        } => {
            assert_eq!(path, "/home/user/next.shade");
            assert!(force);
            assert_eq!(transition.as_deref(), Some("kroma.fade:0.6"));
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
        current_phase: DaemonPhase::Active,
        pending_request_path: None,
        wait_reason: DaemonWaitReason::Idle,
    };
    let json = serde_json::to_string(&event).unwrap();
    let deserialized: DaemonEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonEvent::Status {
            fps,
            paused,
            loaded_shade,
            current_phase,
            pending_request_path,
            wait_reason,
        } => {
            assert!((fps - 59.8).abs() < 0.01);
            assert!(!paused);
            assert_eq!(loaded_shade.unwrap(), "Cyber Rain");
            assert_eq!(current_phase, DaemonPhase::Active);
            assert!(pending_request_path.is_none());
            assert_eq!(wait_reason, DaemonWaitReason::Idle);
        }
        _ => panic!("Expected Status"),
    }
}

#[test]
fn daemon_event_load_rejected_roundtrip() {
    let event = DaemonEvent::LoadRejected {
        code: LoadRejectCode::BusyRunningUnload,
        message: "unload phase is running".into(),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: DaemonEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonEvent::LoadRejected { code, message } => {
            assert_eq!(code, LoadRejectCode::BusyRunningUnload);
            assert_eq!(message, "unload phase is running");
        }
        _ => panic!("Expected LoadRejected"),
    }
}

#[test]
fn daemon_event_load_rejected_busy_transitioning_roundtrip() {
    let event = DaemonEvent::LoadRejected {
        code: LoadRejectCode::BusyTransitioning,
        message: "transition is running".into(),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: DaemonEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonEvent::LoadRejected { code, message } => {
            assert_eq!(code, LoadRejectCode::BusyTransitioning);
            assert_eq!(message, "transition is running");
        }
        _ => panic!("Expected LoadRejected"),
    }
}

#[test]
fn daemon_event_load_rejected_invalid_transition_spec_roundtrip() {
    let event = DaemonEvent::LoadRejected {
        code: LoadRejectCode::InvalidTransitionSpec,
        message: "transition spec is invalid".into(),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: DaemonEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonEvent::LoadRejected { code, message } => {
            assert_eq!(code, LoadRejectCode::InvalidTransitionSpec);
            assert_eq!(message, "transition spec is invalid");
        }
        _ => panic!("Expected LoadRejected"),
    }
}

#[test]
fn daemon_event_load_rejected_transition_timing_infeasible_roundtrip() {
    let event = DaemonEvent::LoadRejected {
        code: LoadRejectCode::TransitionTimingInfeasible,
        message: "timing infeasible".into(),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: DaemonEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonEvent::LoadRejected { code, message } => {
            assert_eq!(code, LoadRejectCode::TransitionTimingInfeasible);
            assert_eq!(message, "timing infeasible");
        }
        _ => panic!("Expected LoadRejected"),
    }
}

#[test]
fn daemon_event_load_rejected_incoming_preload_failed_roundtrip() {
    let event = DaemonEvent::LoadRejected {
        code: LoadRejectCode::IncomingPreloadFailed,
        message: "incoming preload failed".into(),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: DaemonEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonEvent::LoadRejected { code, message } => {
            assert_eq!(code, LoadRejectCode::IncomingPreloadFailed);
            assert_eq!(message, "incoming preload failed");
        }
        _ => panic!("Expected LoadRejected"),
    }
}

#[test]
fn daemon_event_status_transitioning_phase_roundtrip() {
    let event = DaemonEvent::Status {
        fps: 60.0,
        paused: false,
        loaded_shade: Some("Transition Demo".into()),
        current_phase: DaemonPhase::Transitioning,
        pending_request_path: Some("/tmp/next.shade".into()),
        wait_reason: DaemonWaitReason::Idle,
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: DaemonEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonEvent::Status {
            current_phase,
            pending_request_path,
            ..
        } => {
            assert_eq!(current_phase, DaemonPhase::Transitioning);
            assert_eq!(pending_request_path.as_deref(), Some("/tmp/next.shade"));
        }
        _ => panic!("Expected Status"),
    }
}

#[test]
fn daemon_event_status_waiting_preload_roundtrip() {
    let event = DaemonEvent::Status {
        fps: 30.0,
        paused: false,
        loaded_shade: Some("Queued Demo".into()),
        current_phase: DaemonPhase::Active,
        pending_request_path: Some("/tmp/queued.shade".into()),
        wait_reason: DaemonWaitReason::WaitingIncomingPreload,
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: DaemonEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        DaemonEvent::Status { wait_reason, .. } => {
            assert_eq!(wait_reason, DaemonWaitReason::WaitingIncomingPreload);
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
fn translator_handles_iglobaltime_alias() {
    let src = r#"
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    float t = iGlobalTime;
    fragColor = vec4(sin(t), 0.0, 0.0, 1.0);
}
"#;
    let result = translator::translate(src, "Alias", "Author");
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
    assert!(
        result
            .shader_source
            .contains("sampler2D(kroma_tex_0, kroma_samp_0)")
    );
    assert!(
        result
            .shader_source
            .contains("sampler2D(kroma_tex_1, kroma_samp_1)")
    );
    assert!(
        result
            .shader_source
            .contains("sampler2D(kroma_tex_2, kroma_samp_2)")
    );
    let active = result
        .config
        .states
        .active
        .as_ref()
        .expect("active state must exist");
    assert!(active.textures.contains_key("channel0"));
    assert!(active.textures.contains_key("channel1"));
    assert!(active.textures.contains_key("channel2"));
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

[states.active]
length = 0.0

[states.active.uniforms]
speed = { type = "float", min = 0.1, max = 5.0, default = 1.5 }
enabled = { type = "bool", default = true }

[states.active.textures]
channel0 = { type = "video", source = "assets/loop.mp4", loop = true }
"#;
    let config: ShadeConfig = toml::from_str(toml_str).unwrap();
    let active = config.states.active.as_ref().unwrap();
    assert_eq!(config.meta.name, "Test Wallpaper");
    assert_eq!(config.meta.version, "2.0");
    assert!(active.uniforms.contains_key("speed"));
    assert!(active.uniforms.contains_key("enabled"));
    assert!(active.textures.contains_key("channel0"));
    assert!(active.textures["channel0"].looping);
}

#[test]
fn parse_shader_texture_t_minus_one_input() {
    let toml_str = r#"
[meta]
name = "Feedback"
author = "Tester"

[states.active]
length = 0.0

[states.active.textures.main]
type = "shader"
shader = "assets/main.glsl"
width = 512
height = 512

[states.active.textures.main.textures.history]
type = "image"
input = "t-1"
"#;

    let config: ShadeConfig = toml::from_str(toml_str).unwrap();
    let main_tex = config
        .states
        .active
        .as_ref()
        .unwrap()
        .textures
        .get("main")
        .unwrap();
    let history = main_tex.textures.get("history").unwrap();

    assert_eq!(history.input.as_deref(), Some("t-1"));
}

#[test]
fn parse_slideshow_sources_with_shader_entries() {
    let toml_str = r#"
[meta]
name = "Slideshow Shader Mix"
author = "Tester"

[states.active]
length = 0.0

[states.active.textures.wallpaper]
type = "slideshow"
interval = 5.0

[[states.active.textures.wallpaper.sources]]
type = "image"
source = "assets/base.png"

[[states.active.textures.wallpaper.sources]]
type = "shader"
shader = "assets/fx.frag"
width = 1280
height = 720

[states.active.textures.wallpaper.sources.textures.history]
type = "image"
input = "t-1"

[states.active.textures.wallpaper.sources.textures.motion]
type = "video"
source = "assets/motion.mp4"
loop = true
"#;

    let config: ShadeConfig = toml::from_str(toml_str).unwrap();
    let slideshow = config
        .states
        .active
        .as_ref()
        .unwrap()
        .textures
        .get("wallpaper")
        .unwrap();

    assert_eq!(slideshow.sources.len(), 2);
    assert_eq!(slideshow.sources[1].ty.as_str(), "shader");
    assert!(slideshow.sources[1].textures.contains_key("history"));
    assert_eq!(
        slideshow.sources[1].textures["history"].input.as_deref(),
        Some("t-1")
    );
}

#[test]
fn parse_noise_texture_with_and_without_seed() {
    let toml_str = r#"
[meta]
name = "Noise"
author = "Tester"

[states.active]
length = 0.0

[states.active.textures.base]
type = "noise"
width = 320
height = 180

[states.active.textures.detail]
type = "noise"
seed = 1337
width = 320
height = 180
"#;

    let config: ShadeConfig = toml::from_str(toml_str).unwrap();
    let active = config.states.active.as_ref().unwrap();
    let base = active.textures.get("base").unwrap();
    let detail = active.textures.get("detail").unwrap();

    assert_eq!(base.ty.as_str(), "noise");
    assert_eq!(detail.ty.as_str(), "noise");
    assert_eq!(base.seed, None);
    assert_eq!(detail.seed, Some(1337));
    assert_eq!(base.width, Some(320));
    assert_eq!(base.height, Some(180));
}

#[test]
fn parse_rejects_unknown_transitions_usage_key() {
    let toml_str = r#"
[meta]
name = "Invalid Transition Usage"
author = "Tester"

[states.active]
length = 0.0

[transitions_usage]
on_unload_to_terminal = "kroma.fade:0.5"
"#;

    let parsed = toml::from_str::<ShadeConfig>(toml_str);
    assert!(parsed.is_err());
}
