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
    return textureSample(t_texture0, s_texture0, uv);
}
