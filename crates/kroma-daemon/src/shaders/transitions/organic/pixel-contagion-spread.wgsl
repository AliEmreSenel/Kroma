// Builtin transition: pixel-contagion-spread.
//
// A pure-shader port of the CONTAGION transition of pocket-dots' `wallfx`
// (https://github.com/diegoMalagrida/pocket-dots — home/.local/bin/wallfx and
// home/.config/quickshell/shaders/wallfx.frag). A handful of colonies land at
// random places and random moments and eat through the old wallpaper along a
// noise field, in crisp 4 px cells, each one burning for an instant in the
// incoming image's most vivid colour while a thin glow rides the front.
//
// The original does not run in a shader: a python script runs Dijkstra over a
// noise cost grid and writes the arrival times into a texture that its shader
// reads. A Kroma builtin gets nothing but prev_tex, next_tex and
// u_transition_t, so the arrival field is built here instead:
//
//   * colonies are a jittered Voronoi grid, each with its own start moment;
//   * a front pays the noise cost it crosses on the way to a cell, which is the
//     integral Dijkstra accumulates, sampled at the middle of the ray;
//   * arrival is ranked by AREA (cover = arrival squared), so the covered area
//     grows evenly with t — what the original's _ranked() does on the CPU.
//
// The field is fixed per id, as with every builtin. The seed only keeps the
// sparks from repeating between changes, so it steps every ten minutes of
// uptime: far slower than one transition lasts, so it can never pop halfway
// through a change.
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

const CELL_PX: f32 = 4.0;      // the pixels that eat through the old wallpaper
const BURN_LIFE: f32 = 0.05;   // how long a flipped cell burns, in covered area
const GLOW: f32 = 0.9;         // strength of the light riding the front
const RANK_C: f32 = 4.5;       // how arrival ranks to area; see arrivalOf
const PI: f32 = 3.14159265;

fn hash21(p: vec2<f32>) -> f32 {
  let h = dot(p, vec2<f32>(127.1, 311.7));
  return fract(sin(h) * 43758.5453123);
}

fn hash22(p: vec2<f32>) -> vec2<f32> {
  let h = vec2<f32>(dot(p, vec2<f32>(127.1, 311.7)), dot(p, vec2<f32>(269.5, 183.3)));
  return fract(sin(h) * vec2<f32>(43758.5453123, 28001.0123));
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

// Chroma, as the rice's picker ranks its palette: saturation, damped at the
// ends of the range so a near-white cream cannot win.
fn chromaOf(c: vec3<f32>) -> f32 {
  let s = max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b);
  return s * (1.0 - abs(2.0 * luma(c) - 1.0));
}

fn hsvOf(c: vec3<f32>) -> vec3<f32> {
  let mx = max(max(c.r, c.g), c.b);
  let mn = min(min(c.r, c.g), c.b);
  let d = mx - mn;
  var h = 0.0;
  if (mx == c.r) {
    h = (c.g - c.b) / d;
  } else if (mx == c.g) {
    h = (c.b - c.r) / d + 2.0;
  } else {
    h = (c.r - c.g) / d + 4.0;
  }
  return vec3<f32>(fract(h * (1.0 / 6.0)), select(0.0, d / mx, mx > 1.0e-5), mx);
}

fn rgbOf(hsv: vec3<f32>) -> vec3<f32> {
  let p = abs(hsv.z * hsv.y * fract(vec3<f32>(hsv.x) * 6.0 + vec3<f32>(0.0, 4.0, 2.0)) - 3.0);
  return hsv.z * clamp(p - vec3<f32>(1.0), vec3<f32>(0.0), vec3<f32>(1.0));
}

// A palette colour can be dark; the burning edge has to light up.
fn lifted(c: vec3<f32>) -> vec3<f32> {
  let hsv = hsvOf(c);
  return rgbOf(vec3<f32>(hsv.x, max(hsv.y, 0.55), clamp(hsv.z, 0.62, 0.92)));
}

// The noise field the colonies eat through: the three octaves of wallfx's
// _noise3, with its periods (as a fraction of screen height) and a steepness
// that decides how hard the cheap channels are.
struct Field {
  k: f32,
  grain: f32,      // per-cell grain: what makes the fronts ragged at 4 px
  p1: f32,
  p2: f32,
  p3: f32,
  o1: f32,
  o2: f32,
  o3: f32,
  cmed: f32,   // the cost of the MEDIAN ground, so a plain way costs its length
  reach: f32,  // the radius one colony eats before its fronts meet the next
  lamPi: f32,  // colony density, for ranking arrival by area
  t0max: f32,  // the latest a colony may land
  tiles: vec2<f32>,
  aspect: f32,
};

fn fieldFor(seed: f32, aspect: f32) -> Field {
  let r = vec4<f32>(
    hash21(vec2<f32>(seed, 1.0)),
    hash21(vec2<f32>(seed, 2.0)),
    hash21(vec2<f32>(seed, 3.0)),
    hash21(vec2<f32>(seed, 4.0)),
  );
  // Steepness of the cost curve. Measured against the coverage a transition has
  // to reach, a curve steeper than this pushes the colonies behind the walls of
  // expensive noise too far: the last of the screen bunches up at the end and
  // the front is dead for the third of the change that should show it most.
  let k = 2.0 + r.x * 1.5;
  let grain = 0.15 + r.y * 0.5;
  let tiles = vec2<f32>(4.0, 4.0 / max(aspect, 0.5));
  // The screen is a rectangle of `aspect` (its height is 1) shared by N colonies.
  let n = tiles.x * tiles.y;
  // The disc one colony eats before its fronts meet the next ones.
  let reach = sqrt(aspect / (n * PI));
  // Area grows with the square of how far the fronts have come, so ranking by
  // area means CDF(arrival) = 1 - exp(-density * pi * arrival^2). Feeding that
  // back as the mask is what makes t mean area, evenly, to the last cell —
  // which is the service the original's _ranked() does with a sort.
  let lamPi = n * PI / aspect;
  // The cost curve is centred on n = 0.5, where it is exactly 1, so the MEDIAN
  // ground costs its own length: its mean is many times that (a wall of
  // expensive noise is rare, and Dijkstra goes round it) and normalising by the
  // mean is what makes every colony reach the far edge in the first breath of t.
  let cmed = 1.12 + 0.5 * grain;
  var f: Field;
  f.k = k;
  f.grain = grain;
  f.p1 = 0.10 + 0.10 * r.x;
  f.p2 = 0.028 + 0.030 * r.z;
  f.p3 = 0.008 + 0.008 * r.w;
  f.o1 = r.w * 91.7;
  f.o2 = r.z * 57.3;
  f.o3 = r.y * 33.1;
  f.cmed = cmed;
  f.reach = reach;
  f.lamPi = lamPi;
  f.t0max = 0.12 * reach;
  f.tiles = tiles;
  f.aspect = aspect;
  return f;
}

fn costOf(f: Field, sp: vec2<f32>) -> f32 {
  let n = 0.52 * vnoise(sp / f.p1 + vec2<f32>(f.o1, f.o1))
        + 0.30 * vnoise(sp / f.p2 + vec2<f32>(f.o2, f.o2))
        + 0.18 * vnoise(sp / f.p3 + vec2<f32>(f.o3, f.o3));
  return 0.12 + exp(f.k * (n - 0.5));
}

// When the front reaches this point: the colony that owns it, its start moment,
// and the cost of the way in between. `grain` is the cell's own grain (0 for
// the smooth twin), which is what makes the fronts ragged at cell scale.
//
// Ranked by area: the ground a front has eaten covers an area that grows with
// the square of how far it travelled, so squaring the arrival is what makes t
// mean area, and the change reads at a steady rate.
fn arrivalOf(f: Field, seed: f32, sp: vec2<f32>, uv: vec2<f32>, grain: f32) -> f32 {
  let gi = floor(uv * f.tiles);
  var best = 1.0e9;
  for (var oy: i32 = -1; oy <= 1; oy = oy + 1) {
    for (var ox: i32 = -1; ox <= 1; ox = ox + 1) {
      let cell = gi + vec2<f32>(f32(ox), f32(oy));
      let r = hash22(cell + vec2<f32>(seed, seed * 0.37));
      // The colony stays inside its own grid cell, and cells off-screen keep
      // their colonies off-screen: pinning them to the frame instead would
      // crowd the edges with colonies and make the whole screen arrive early.
      let seedUv = (cell + vec2<f32>(0.12, 0.12) + 0.76 * r) / f.tiles;
      let seedSp = (seedUv - vec2<f32>(0.5, 0.5)) * vec2<f32>(f.aspect, 1.0);
      let d = length(sp - seedSp);
      let path = costOf(f, mix(seedSp, sp, 0.5));
      best = min(best, r.y * f.t0max + d * (path + f.grain * grain) / f.cmed);
    }
  }
  // How much of the screen a front this far out has eaten: N discs of radius
  // `best` over a screen of `aspect` — the CDF of the arrival over area, which
  // is exactly what makes t mean area.
  let reached = 1.0 - exp(-f.lamPi * best * best);
  // The fronts of a jittered grid meet a little sooner than discs on paper say
  // they should, which bunches the last of the screen at the end. This second
  // curve is the measured rank of `reached` (tuned against the field itself):
  // with it the ground eaten tracks t to a few per cent over the whole change.
  return 1.0 - exp(-RANK_C * reached * reached);
}

// The new palette's most vivid colour, read off the incoming image: a few taps
// over the whole frame, keeping the one with the most chroma. The accent of a
// palette can be a cream that vanishes on a cream sky; this cannot.
fn vividOf() -> vec3<f32> {
  var best = vec3<f32>(1.0, 0.55, 0.10);
  var bestChroma = -1.0;
  for (var i: u32 = 0u; i < 16u; i = i + 1u) {
    let p = fract(vec2<f32>(f32(i) * 0.6180340, f32(i) * 0.7548777));
    let c = textureSampleLevel(next_tex, next_samp, p, 0.0).rgb;
    let ch = chromaOf(c);
    if (ch > bestChroma) {
      bestChroma = ch;
      best = c;
    }
  }
  return best;
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
  let sp = (uv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);
  let seed = floor(globals.u_time * (1.0 / 600.0)) * 13.17;
  let f = fieldFor(seed, aspect);
  let ember = lifted(vividOf());
  let hot = mix(ember, vec3<f32>(1.0), 0.18);

  // Cells decide by their centre, so the fronts stay crisp and stepped.
  let cell = floor(uv * res / CELL_PX);
  let cellUv = (cell + vec2<f32>(0.5, 0.5)) * CELL_PX / res;
  let cellSp = (cellUv - vec2<f32>(0.5, 0.5)) * vec2<f32>(aspect, 1.0);
  let duv = CELL_PX / res;
  let dsp = vec2<f32>(CELL_PX / res.y, CELL_PX / res.y);
  let grain = hash21(cell + vec2<f32>(seed, seed * 0.71));

  let cover = arrivalOf(f, seed, cellSp, cellUv, grain);
  // The front runs a little past 1.0 so the last cells finish. The burn is
  // faded out over the last three per cent: t = 1 hands over to the next shade
  // with nothing left glowing.
  let front = eased * (1.0 + BURN_LIFE + 0.02) - 0.02;
  let burnFade = 1.0 - smoothstep(0.97, 1.0, t);

  // The new image settles from a touch closer than it started.
  let z = 1.0 + 0.025 * (1.0 - eased);
  let uvNext = (uv - vec2<f32>(0.5, 0.5)) / z + vec2<f32>(0.5, 0.5);
  let prev = textureSample(prev_tex, prev_samp, uv).rgb;
  let next = textureSample(next_tex, next_samp, uvNext).rgb;

  var c = mix(prev, next, select(0.0, 1.0, cover <= front));

  if (cover <= front) {
    let life = clamp((front - cover) / BURN_LIFE, 0.0, 1.0);
    var edge = 0.0;
    if (life < 1.0) {
      // A cell on the front burns whole; one already behind it only smoulders.
      edge = step(front, arrivalOf(f, seed, cellSp + vec2<f32>(dsp.x, 0.0), cellUv + vec2<f32>(duv.x, 0.0),
                                   hash21(cell + vec2<f32>(1.0, 0.0) + seed)));
      edge = max(edge, step(front, arrivalOf(f, seed, cellSp - vec2<f32>(dsp.x, 0.0), cellUv - vec2<f32>(duv.x, 0.0),
                                        hash21(cell - vec2<f32>(1.0, 0.0) + seed))));
      edge = max(edge, step(front, arrivalOf(f, seed, cellSp + vec2<f32>(0.0, dsp.y), cellUv + vec2<f32>(0.0, duv.y),
                                        hash21(cell + vec2<f32>(0.0, 1.0) + seed))));
      edge = max(edge, step(front, arrivalOf(f, seed, cellSp - vec2<f32>(0.0, dsp.y), cellUv - vec2<f32>(0.0, duv.y),
                                        hash21(cell - vec2<f32>(0.0, 1.0) + seed))));
    }
    let burn = (1.0 - life) * (1.0 - life) * select(0.55, 1.0, edge > 0.5);
    c = mix(c, hot, burn * (0.55 + 0.45 * grain) * 0.85 * GLOW * burnFade);
  } else {
    // A spark: a lone cell just ahead of the front lights up for a moment.
    let ahead = cover - front;
    if (grain > 0.985 && ahead < 0.02 && front > 0.0) {
      c = mix(c, hot, (1.0 - ahead / 0.02) * 0.8 * GLOW * burnFade);
    }
  }

  // The thin glow riding the front, on both sides: the smooth twin of the
  // arrival field — no grain, no cell grid — at the front's own moment.
  let smoothCover = arrivalOf(f, seed, sp, uv, 0.0);
  c = c + ember * 0.22 * GLOW * sin(PI * t) * exp(-abs(smoothCover - front) / 0.006);

  return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
