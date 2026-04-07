//! Procedural noise texture source.
//!
//! Generates deterministic RGBA noise data once on load and uploads it on the
//! first frame. If `seed` is omitted, a process-wide default seed is
//! initialized once and reused for all omitted-seed noise textures.

use std::sync::OnceLock;

use anyhow::Result;
use log::info;

use super::{TextureSource, TextureUpdate};

const DEFAULT_NOISE_SEED: u64 = 0x4B52_4F4D_415F_4E30;

static OMITTED_NOISE_SEED: OnceLock<u64> = OnceLock::new();

fn process_default_seed() -> u64 {
    *OMITTED_NOISE_SEED.get_or_init(|| DEFAULT_NOISE_SEED)
}

fn splitmix64_next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn generate_noise_rgba(width: u32, height: u32, seed: u64) -> Vec<u8> {
    let mut state = seed;
    let mut rgba = vec![0u8; (width * height * 4) as usize];

    for px in rgba.chunks_exact_mut(4) {
        let n = splitmix64_next(&mut state);
        px[0] = (n & 0xFF) as u8;
        px[1] = ((n >> 8) & 0xFF) as u8;
        px[2] = ((n >> 16) & 0xFF) as u8;
        px[3] = 255;
    }

    rgba
}

/// A static procedural noise texture.
pub struct NoiseTexture {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    needs_upload: bool,
}

impl NoiseTexture {
    /// Build a procedural noise texture.
    ///
    /// If `seed` is `None`, a process-wide default seed initialized once is
    /// used to keep output stable across subsequent loads.
    pub fn load(width: u32, height: u32, seed: Option<u64>) -> Result<Self> {
        let width = width.max(1);
        let height = height.max(1);
        let seed = seed.unwrap_or_else(process_default_seed);
        let rgba = generate_noise_rgba(width, height, seed);

        info!(
            "NoiseTexture generated: {}x{}, seed={}{}",
            width,
            height,
            seed,
            if seed == process_default_seed() {
                " (default)"
            } else {
                ""
            }
        );

        Ok(Self {
            rgba,
            width,
            height,
            needs_upload: true,
        })
    }
}

impl TextureSource for NoiseTexture {
    fn update(&mut self, _dt: f64) -> Result<TextureUpdate> {
        if self.needs_upload {
            self.needs_upload = false;
            return Ok(TextureUpdate::NewFrame {
                data: self.rgba.clone(),
                width: self.width,
                height: self.height,
            });
        }
        Ok(TextureUpdate::Unchanged)
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn texture_type(&self) -> &'static str {
        "noise"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_seed_is_stable_across_loads() {
        let mut a = NoiseTexture::load(16, 16, None).expect("noise a");
        let mut b = NoiseTexture::load(16, 16, None).expect("noise b");

        let a_data = match a.update(0.0).expect("a update") {
            TextureUpdate::NewFrame { data, .. } => data,
            TextureUpdate::Unchanged => panic!("expected new frame from noise a"),
        };
        let b_data = match b.update(0.0).expect("b update") {
            TextureUpdate::NewFrame { data, .. } => data,
            TextureUpdate::Unchanged => panic!("expected new frame from noise b"),
        };

        assert_eq!(a_data, b_data);
    }

    #[test]
    fn explicit_seed_controls_output() {
        let mut a = NoiseTexture::load(16, 16, Some(7)).expect("seed 7");
        let mut b = NoiseTexture::load(16, 16, Some(7)).expect("seed 7 second");
        let mut c = NoiseTexture::load(16, 16, Some(8)).expect("seed 8");

        let a_data = match a.update(0.0).expect("a update") {
            TextureUpdate::NewFrame { data, .. } => data,
            TextureUpdate::Unchanged => panic!("expected new frame from noise a"),
        };
        let b_data = match b.update(0.0).expect("b update") {
            TextureUpdate::NewFrame { data, .. } => data,
            TextureUpdate::Unchanged => panic!("expected new frame from noise b"),
        };
        let c_data = match c.update(0.0).expect("c update") {
            TextureUpdate::NewFrame { data, .. } => data,
            TextureUpdate::Unchanged => panic!("expected new frame from noise c"),
        };

        assert_eq!(a_data, b_data);
        assert_ne!(a_data, c_data);
    }

    #[test]
    fn reported_dimensions_match_requested_size() {
        let tex = NoiseTexture::load(320, 200, None).expect("noise");
        assert_eq!(tex.dimensions(), (320, 200));
    }
}
