//! Audio spectrum texture source.
//!
//! Captures audio from a system device via cpal, runs FFT in a background
//! thread, and produces a 1D R32Float texture with the frequency spectrum.
//! Each `AudioTexture` owns its own capture stream, so multiple audio
//! textures with different sources can coexist.

use anyhow::Result;
use log::info;

use crate::audio::{AudioProvider, CpalAudioProvider};
use kroma_shared::types::AudioConfig;

use super::{TextureFormat, TextureSource, TextureUpdate};

/// A self-contained audio spectrum texture.
///
/// Wraps a [`CpalAudioProvider`] that captures audio and processes FFT
/// in a background thread. On each [`update`](TextureSource::update),
/// the current spectrum is read and returned as R32Float pixel data.
pub struct AudioTexture {
    provider: CpalAudioProvider,
    bands: usize,
}

impl AudioTexture {
    /// Start audio capture for the given source device.
    ///
    /// `source` is an audio device identifier: `"desktop"`, `"microphone"`,
    /// or a specific device name substring.
    /// `bands` is the number of FFT frequency bands (e.g. 512, 1024).
    pub fn load(source: &str, bands: usize) -> Result<Self> {
        let mut provider = CpalAudioProvider::new();
        let audio_conf = AudioConfig {
            enabled: true,
            source: source.to_string(),
            fft_bands: bands,
        };
        provider.switch(&audio_conf)?;
        info!(
            "AudioTexture loaded: source='{}', bands={}",
            source, bands
        );
        Ok(Self { provider, bands })
    }
}

impl TextureSource for AudioTexture {
    fn update(&mut self, _dt: f64) -> Result<TextureUpdate> {
        let spectrum = self.provider.get_spectrum();
        // Pad or truncate to exactly `self.bands` values
        let mut padded = vec![0.0f32; self.bands];
        let len = spectrum.len().min(self.bands);
        padded[..len].copy_from_slice(&spectrum[..len]);
        // Cast f32 slice to raw bytes
        let data = bytemuck::cast_slice(&padded).to_vec();
        Ok(TextureUpdate::NewFrame {
            data,
            width: self.bands as u32,
            height: 1,
        })
    }

    fn dimensions(&self) -> (u32, u32) {
        (self.bands as u32, 1)
    }

    fn format(&self) -> TextureFormat {
        TextureFormat::R32Float
    }

    fn audio_level(&self) -> f32 {
        self.provider.get_level()
    }

    fn texture_type(&self) -> &'static str {
        "audio_spectrum"
    }
}
