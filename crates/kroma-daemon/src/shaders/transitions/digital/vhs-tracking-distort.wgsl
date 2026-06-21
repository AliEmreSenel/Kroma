// Auto-generated builtin transition: vhs-tracking-distort
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
  u_transition_t: f32,
  _pad2_0: f32,
  _pad2_1: f32,
  _pad2_2: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var prev_tex: texture_2d<f32>;
@group(1) @binding(1) var prev_samp: sampler;
@group(1) @binding(2) var next_tex: texture_2d<f32>;
@group(1) @binding(3) var next_samp: sampler;

const STYLE: f32 = 78.0;

fn hash21(p: vec2<f32>) -> f32 {
  let h = dot(p, vec2<f32>(127.1, 311.7));
  return fract(sin(h) * 43758.5453123);
}

fn rotate2d(a: f32) -> mat2x2<f32> {
  let c = cos(a);
  let s = sin(a);
  return mat2x2<f32>(vec2<f32>(c, -s), vec2<f32>(s, c));
}

@vertex
fn vs_main(@builtin(vertex_index) vtx: u32) -> @builtin(position) vec4<f32> {
  var pos = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -3.0),
    vec2<f32>(-1.0, 1.0),
    vec2<f32>(3.0, 1.0)
  );
  return vec4<f32>(pos[vtx], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) frag_pos: vec4<f32>) -> @location(0) vec4<f32> {
  let res = max(globals.u_resolution, vec2<f32>(1.0, 1.0));
  let uv = frag_pos.xy / res;
  let t = clamp(globals.u_transition_t, 0.0, 1.0);
  let line = sin((uv.y + t * 0.2) * (120.0 + STYLE));
  let rand = hash21(vec2<f32>(floor(uv.y * 300.0), STYLE));
  let jitter = (line * 0.003 + (rand - 0.5) * 0.006) * (1.0 - t);
  let prev = textureSample(prev_tex, prev_samp, clamp(uv + vec2<f32>(jitter, 0.0), vec2<f32>(0.0), vec2<f32>(1.0)));
  let n = textureSample(next_tex, next_samp, uv);
  let c = 0.002 * (1.0 - t);
  let r = textureSample(next_tex, next_samp, clamp(uv + vec2<f32>(-c, 0.0), vec2<f32>(0.0), vec2<f32>(1.0))).r;
  let g = n.g;
  let b = textureSample(next_tex, next_samp, clamp(uv + vec2<f32>(c, 0.0), vec2<f32>(0.0), vec2<f32>(1.0))).b;
  let next = vec4<f32>(r, g, b, n.a);
  return mix(prev, next, t);
}
