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

    return vec4<f32>(color, 1.0);
}

