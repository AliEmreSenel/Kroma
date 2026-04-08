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
@group(1) @binding(0) var t_texture0: texture_2d<f32>;
@group(1) @binding(1) var s_texture0: sampler;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let tex_dims_u = textureDimensions(t_texture0);
    let tex_dims = vec2<f32>(f32(tex_dims_u.x), f32(tex_dims_u.y));
    let screen_dims = max(globals.u_resolution, vec2<f32>(1.0, 1.0));

    let tex_aspect = tex_dims.x / max(tex_dims.y, 1.0);
    let screen_aspect = screen_dims.x / screen_dims.y;

    var sample_uv = uv;
    if (tex_aspect > screen_aspect) {
        // Source is wider than the screen: crop left/right.
        let x_scale = screen_aspect / tex_aspect;
        sample_uv.x = (uv.x - 0.5) * x_scale + 0.5;
    } else {
        // Source is taller than the screen: crop top/bottom.
        let y_scale = tex_aspect / screen_aspect;
        sample_uv.y = (uv.y - 0.5) * y_scale + 0.5;
    }

    return textureSample(t_texture0, s_texture0, clamp(sample_uv, vec2<f32>(0.0), vec2<f32>(1.0)));
}
