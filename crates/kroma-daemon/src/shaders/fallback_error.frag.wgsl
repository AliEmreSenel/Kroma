// Fallback error fragment shader.
//
// Renders a dark background with diagonal stripes and vignette, then
// composites text from a "text bitmap" texture whose R channel encodes
// the region type:
//   0.0       → no text (background only)
//   ~0.333    → title (warm red)
//   ~0.667    → subtitle (muted slate)
//   ~1.0      → path / detail (light gray-white)
//
// The text bitmap is generated on the CPU using the font8x8 crate.

struct Globals {
    u_time: f32,
    u_delta_time: f32,
    u_frame: u32,
    _pad0: u32,
    u_resolution: vec2<f32>,
    _pad1: vec2<f32>,
    u_mouse: vec4<f32>,
    u_cpu: f32,
    u_ram: f32,
    u_battery: f32,
    u_audio_level: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var t_text: texture_2d<f32>;
@group(1) @binding(1) var s_text: sampler;

// Text region colors
const TITLE_COLOR: vec3<f32> = vec3<f32>(0.92, 0.34, 0.34);
const SUBTITLE_COLOR: vec3<f32> = vec3<f32>(0.55, 0.57, 0.63);
const PATH_COLOR: vec3<f32> = vec3<f32>(0.71, 0.73, 0.76);

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    // ---- Background ----
    let center = vec2<f32>(0.5, 0.5);
    let dist = distance(uv, center);
    let vignette = 1.0 - smoothstep(0.2, 0.9, dist);

    let stripe = step(0.5, fract((uv.x + uv.y) * 24.0));
    let base = vec3<f32>(0.059, 0.067, 0.086);
    let accent = vec3<f32>(0.016) * stripe;
    var color = base + accent + vec3<f32>(vignette * 0.03);

    // ---- Text sampling (integer coords via textureLoad) ----
    let tex_dims = vec2<f32>(textureDimensions(t_text, 0));
    let px = vec2<i32>(i32(uv.x * tex_dims.x), i32(uv.y * tex_dims.y));

    // Shadow pass: sample at a small offset
    let shadow_off = max(2, i32(tex_dims.y / 540.0));
    let shadow_px = px + vec2<i32>(shadow_off, shadow_off);
    let shadow_region = textureLoad(t_text, shadow_px, 0).r;
    if shadow_region > 0.01 {
        color = mix(color, vec3<f32>(0.0, 0.0, 0.0), 0.45);
    }

    // Foreground text
    let region = textureLoad(t_text, px, 0).r;
    if region > 0.01 {
        var text_color = PATH_COLOR;
        if region < 0.5 {
            text_color = TITLE_COLOR;
        } else if region < 0.8 {
            text_color = SUBTITLE_COLOR;
        }
        color = text_color;
    }

    return vec4<f32>(color, 1.0);
}
