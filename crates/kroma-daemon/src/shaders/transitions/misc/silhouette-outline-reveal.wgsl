// Builtin transition: silhouette-outline-reveal.
//
// A pure-shader port of the OUTLINE ("perfil") transition of pocket-dots'
// `wallfx` (https://github.com/diegoMalagrida/pocket-dots — home/.local/bin/wallfx
// and home/.config/quickshell/shaders/wallfx.frag). The stage steps back a
// little, a thin quiet line draws the subject's outline from the top down, the
// subject fills in from that line, and the rest of the world then spreads out of
// it in the same crisp 4 px cells. No glow, no flash: it should look effortless.
//
// The original has no shader of its own for the hard part: a python script cuts
// the character out of the incoming wallpaper with BiRefNet, runs two distance
// transforms over the mask, and parameterises every contour so a pen can draw
// it. None of that is available to a Kroma builtin, which gets nothing but
// prev_tex, next_tex and u_transition_t, so the cut-out is keyed from the image
// itself:
//
//   * the subject is what sits above its own blurred surroundings — the
//     local-contrast key `luma(next) - blur(luma(next))`;
//   * that key divided by its own gradient is the signed distance to the
//     silhouette, in pixels, which is what the original reads off the map;
//   * the pen sweeps from the top of the frame down, which is the same
//     parameter wallfx measures along the contour from its topmost point.
//
// Everything downstream — the phases, their timings, the 4 px cells, the two
// cameras, the stepped-back stage — is the original's choreography.
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

const CELL_PX: f32 = 4.0;    // the same cells the contagion transition eats with
const LINE_W: f32 = 1.5;     // the pen, in px: thin and quiet on purpose
const KEY_R: f32 = 0.055;    // how far the "surroundings" are measured, in heights
const KEY_THR: f32 = 0.035;  // how far a pixel has to stand out to be subject
const INNER_D: f32 = 0.10;   // depth that counts as the middle of the subject
// How far the world spreads out of the silhouette. Must reach the corners of
// the frame — a 16:9 screen puts those ~1.1 heights from a centred subject —
// or the far ground clamps to the end of the queue and all lands at once.
const OUT_D: f32 = 1.15;
// The widest spread, in heights, sd is ever asked to measure.
const MAX_SD: f32 = OUT_D;
// The widest queue position can reach: a distance of 1, plus the noise's reach
// above it. Dividing by it keeps the queue in 0..1, so the last cell arrives
// exactly as the window closes instead of piling up behind it.
const ORDER_MAX: f32 = 1.11;
const JITTER: f32 = 0.22;   // how far the noise drags a cell's queue position
// Key gradient below which the distance is noise — plain ground, where the key
// and its gradient are both ~0 — and the queue hands over to the radius. A
// silhouette edge is two to three orders of magnitude above this.
const G_MIN: f32 = 0.008;
// The spread's window, in t. Linear in t on purpose: an eased ramp spends its
// middle at speed, which is where most of the queue is, and idles at the ends.
const SPREAD_T0: f32 = 0.45;
const SPREAD_T1: f32 = 0.95;
// Half-diagonal of the frame, in the same screen space: the widest radius the
// queue's fallback term has to cover.
const GEO_D: f32 = 1.05;
const PI: f32 = 3.14159265;

fn hash21(p: vec2<f32>) -> f32 {
  let h = dot(p, vec2<f32>(127.1, 311.7));
  return fract(sin(h) * 43758.5453123);
}

fn vnoise(p: vec2<f32>) -> f32 {
  let i = floor(p);
  let f = fract(p);
  let u = f * f * (3.0 - 2.0 * f);
  let a = hash21(i);
  let b = hash21(i + vec2<f32>(1.0, 0.0));
  let c = hash21(i + vec2<f32>(0.0, 1.0));
  let d = hash21(i + vec2<f32>(1.0, 1.0));
  return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn luma(c: vec3<f32>) -> f32 {
  return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

fn nextAt(uv: vec2<f32>) -> vec3<f32> {
  return textureSample(next_tex, next_samp, uv).rgb;
}

// The surroundings: the incoming image's luma, blurred at the scale of a
// subject. Cheap on purpose — a five-tap box at the subject's own size is
// enough to say what a pixel is standing out from.
fn blurLuma(uv: vec2<f32>, r: vec2<f32>) -> f32 {
  var s = 0.36 * luma(textureSampleLevel(next_tex, next_samp, uv, 0.0).rgb);
  s = s + 0.16 * luma(textureSampleLevel(next_tex, next_samp, uv + vec2<f32>(r.x, 0.0), 0.0).rgb);
  s = s + 0.16 * luma(textureSampleLevel(next_tex, next_samp, uv - vec2<f32>(r.x, 0.0), 0.0).rgb);
  s = s + 0.16 * luma(textureSampleLevel(next_tex, next_samp, uv + vec2<f32>(0.0, r.y), 0.0).rgb);
  s = s + 0.16 * luma(textureSampleLevel(next_tex, next_samp, uv - vec2<f32>(0.0, r.y), 0.0).rgb);
  return s;
}

// How much a pixel stands out of its surroundings. Positive is subject.
fn keyAt(uv: vec2<f32>, r: vec2<f32>) -> f32 {
  return luma(textureSampleLevel(next_tex, next_samp, uv, 0.0).rgb) - blurLuma(uv, r);
}

// How far a pixel is from the silhouette, in pixels: negative inside the
// subject. The silhouette is where the key crosses KEY_THR, and a field divided
// by its own gradient turns that crossing into a distance — the way the map's B
// channel carried it. The threshold is not decoration: on flat, featureless
// ground the key and its gradient are both ~0, and the ratio of two nothing is
// 0, which reads as "deep inside the subject" over the whole screen. Measuring
// against the level keeps plain ground firmly outside.
fn sdAt(uv: vec2<f32>, r: vec2<f32>, px: vec2<f32>) -> f32 {
  let k = keyAt(uv, r);
  let gx = (keyAt(uv + vec2<f32>(px.x, 0.0), r) - keyAt(uv - vec2<f32>(px.x, 0.0), r)) * 0.5;
  let gy = (keyAt(uv + vec2<f32>(0.0, px.y), r) - keyAt(uv - vec2<f32>(0.0, px.y), r)) * 0.5;
  // Clamped at both ends. The gradient vanishes far from any edge, and an
  // unclamped ratio reads as a million pixels from the silhouette: the middle
  // of a big subject lands on the far-outside side of the queue and never
  // fills in, and the corners queue up behind it.
  let far = MAX_SD * max(globals.u_resolution.y, 1.0);
  return clamp((KEY_THR - k) / max(length(vec2<f32>(gx, gy)), 1.0e-4), -far, far);
}

// `sdAt` and, beside it, the gradient that divides it — the tell for whether
// the distance means anything. On plain ground both the key and its gradient
// are ~0, so the ratio is the threshold over the epsilon floor: a big number
// that says nothing. As (distance, gradient).
fn sdGradAt(uv: vec2<f32>, r: vec2<f32>, px: vec2<f32>) -> vec2<f32> {
  let k = keyAt(uv, r);
  let gx = (keyAt(uv + vec2<f32>(px.x, 0.0), r) - keyAt(uv - vec2<f32>(px.x, 0.0), r)) * 0.5;
  let gy = (keyAt(uv + vec2<f32>(0.0, px.y), r) - keyAt(uv - vec2<f32>(0.0, px.y), r)) * 0.5;
  let g = length(vec2<f32>(gx, gy));
  let far = MAX_SD * max(globals.u_resolution.y, 1.0);
  return vec2<f32>(clamp((KEY_THR - k) / max(g, 1.0e-4), -far, far), g);
}

struct Palette {
  ink: vec3<f32>,
  // Rough centre of mass of the subject, in aspect-corrected screen space.
  center: vec2<f32>,
};

// The palette's lightest tone — the ink of the line — and the subject's rough
// centre, both off one sixteen-point spiral over the incoming image. The centre
// is the mean position of everything sitting well above the image's average
// luma: crude, deterministic, and free of an extra pass over the texture.
fn paletteOf(aspect: f32) -> Palette {
  var lumas = array<f32, 16>(
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
  );
  var best = vec3<f32>(0.0);
  var bestLuma = -1.0;
  var mean = 0.0;
  for (var i: u32 = 0u; i < 16u; i = i + 1u) {
    let p = fract(vec2<f32>(f32(i) * 0.6180340, f32(i) * 0.7548777));
    let c = textureSampleLevel(next_tex, next_samp, p, 0.0).rgb;
    let l = luma(c);
    lumas[i] = l;
    mean = mean + l;
    if (l > bestLuma) {
      bestLuma = l;
      best = c;
    }
  }
  mean = mean / 16.0;
  var sum = vec2<f32>(0.0);
  var wsum = 0.0;
  for (var i: u32 = 0u; i < 16u; i = i + 1u) {
    let w = max(lumas[i] - mean - 0.12, 0.0);
    let p = fract(vec2<f32>(f32(i) * 0.6180340, f32(i) * 0.7548777));
    sum = sum + (vec2<f32>(0.5, 0.5) - p) * vec2<f32>(aspect, 1.0) * w;
    wsum = wsum + w;
  }
  var center = vec2<f32>(0.0);
  if (wsum > 1.0e-3) {
    center = sum / wsum;
  }
  return Palette(best, center);
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
  let eased = smoothstep(0.0, 1.0, t);
  let aspect = res.x / res.y;
  let px = vec2<f32>(1.0 / res.x, 1.0 / res.y);
  let r = vec2<f32>(KEY_R / aspect, KEY_R);
  // Aspect-corrected space, screen height = 1, y up.
  let sp = (vec2<f32>(0.5, 0.5) - uv) * vec2<f32>(aspect, 1.0);

  // Cells decide by their centre, so the fronts are crisp and stepped; the
  // silhouette itself stays smooth, and the line stays thin.
  let cell = floor(uv * res / CELL_PX);
  let cellUv = (cell + vec2<f32>(0.5, 0.5)) * CELL_PX / res;

  let pal = paletteOf(aspect);
  let sd = sdAt(uv, r, px);
  let cellSdG = sdGradAt(cellUv, r, px);
  let cellSd = cellSdG.x;
  let inside = clamp(0.5 - sd, 0.0, 1.0);

  // The stage steps back: a fifth darker, a quarter greyer.
  let back = smoothstep(0.0, 0.18, t);
  let prev = textureSample(prev_tex, prev_samp, uv).rgb;
  let stage = mix(mix(prev, vec3<f32>(luma(prev)), 0.25 * back), vec3<f32>(0.0), 0.0)
            * (1.0 - 0.2 * back);

  // Two cameras, barely: the subject settles from a touch closer than its
  // background, which is enough to give it a little depth.
  let zCh = 1.0 + 0.03 * (1.0 - eased);
  let zBg = 1.0 + 0.015 * (1.0 - eased);
  let cIn = nextAt((uv - vec2<f32>(0.5, 0.5)) / zCh + vec2<f32>(0.5, 0.5));
  let cOut = nextAt((uv - vec2<f32>(0.5, 0.5)) / zBg + vec2<f32>(0.5, 0.5));

  // The subject fills in from its outline: depth 0 on the silhouette, 1 at its
  // middle, so the deepest part arrives last.
  let inner = clamp(-cellSd / (INNER_D * res.y), 0.0, 1.0);
  let ti = smoothstep(0.32, 0.64, t) * 1.02 - 0.01;
  let filledIn = cellSd < 0.0 && inner <= ti;

  // Then the world spreads out of it, roughened with noise so the front is not
  // a perfect offset of the silhouette, and ranked by area like the contagion.
  //
  // The key alone cannot rank the queue: it is a band detector, and on flat
  // ground it sits at zero at every scale, so its distance saturates and an
  // entire field of plain colour queues at one value — 92% of a dark,
  // featureless wallpaper then lands in the same frame. Where the key runs out
  // of signal the queue falls back to the plain radius from the subject, which
  // is defined everywhere.
  let n = 0.6 * vnoise(sp / 0.11) + 0.4 * vnoise(sp / 0.036);
  let keyS = clamp(cellSd / (OUT_D * res.y), 0.0, 1.0);
  let geoS = clamp(length(sp - pal.center) / GEO_D, 0.0, 1.0);
  // Queue position: distance from the subject, jittered by the noise. Where the
  // key's gradient has died out, its distance is fiction and the queue runs on
  // the radius instead. Both the jitter and the window are linear on purpose:
  // a squared rank over an eased window spends its middle at speed, which is
  // where the queue is, so the transition rushes its centre and idles its tail.
  let order = clamp(mix(keyS, geoS, 1.0 - smoothstep(0.0, G_MIN, cellSdG.y))
                    + (n - 0.5) * JITTER,
                    0.0, ORDER_MAX) / ORDER_MAX;
  let to = (t - SPREAD_T0) / (SPREAD_T1 - SPREAD_T0);
  let spread = order <= to;

  // The world arrives from outside in; where the subject is not filled in yet,
  // what shows through is the world — the stage before the shockwave, the new
  // wallpaper after it has passed. That is what makes the subject read as being
  // painted in from its own outline, and it leaves nothing to uncover at the end.
  let arrived = mix(stage, cOut, select(0.0, 1.0, spread));
  var c = mix(arrived, cIn, select(0.0, 1.0, filledIn) * inside);

  // The line: drawn by two pens from the top of the frame down to the bottom,
  // in the palette's lightest tone, and gone once the subject is in.
  let p = (1.0 - normalize(sp + vec2<f32>(0.0, 1.0e-4)).y) * 0.5;
  let drawP = smoothstep(0.02, 0.35, t);
  let lineFade = (1.0 - smoothstep(0.45, 0.70, t)) * smoothstep(0.015, 0.04, t);
  let core = clamp(0.5 - (abs(sd) - LINE_W * 0.5), 0.0, 1.0);
  if (lineFade > 0.0 && p <= drawP && core > 0.0) {
    c = mix(c, pal.ink, core * 0.75 * lineFade);
  }

  return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
