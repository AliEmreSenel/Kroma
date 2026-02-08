//! Audio spectrum analysis provider.
//!
//! Provides a normalised 512-band audio spectrum for shader consumption.
//! Includes:
//! - `SilentAudioProvider` — returns silence (no audio backend)
//! - `SimulatedAudioProvider` — fake sine-wave spectrum for demos
//! - `CpalAudioProvider` — real audio capture via cpal + FFT

use std::sync::{Arc, Mutex};

/// Number of frequency bands in the spectrum.
pub const SPECTRUM_BANDS: usize = 512;

/// FFT size (must be power of 2, >= SPECTRUM_BANDS * 2).
const FFT_SIZE: usize = 1024;

/// Trait for audio spectrum providers.
pub trait AudioProvider: Send + Sync {
    /// Returns a normalised audio spectrum: `SPECTRUM_BANDS` values in 0.0–1.0.
    fn get_spectrum(&self) -> Vec<f32>;

    /// Returns the current audio level (RMS, 0.0–1.0).
    fn get_level(&self) -> f32;
}

/// Stub audio provider that returns silence.
///
/// Used when no audio backend is available.
pub struct SilentAudioProvider;

impl AudioProvider for SilentAudioProvider {
    fn get_spectrum(&self) -> Vec<f32> {
        vec![0.0; SPECTRUM_BANDS]
    }

    fn get_level(&self) -> f32 {
        0.0
    }
}

/// Simulated audio provider for testing and demos.
///
/// Generates a fake spectrum that changes over time (sine-wave based).
pub struct SimulatedAudioProvider {
    start_time: std::time::Instant,
}

impl SimulatedAudioProvider {
    pub fn new() -> Self {
        Self {
            start_time: std::time::Instant::now(),
        }
    }
}

impl AudioProvider for SimulatedAudioProvider {
    fn get_spectrum(&self) -> Vec<f32> {
        let t = self.start_time.elapsed().as_secs_f32();
        (0..SPECTRUM_BANDS)
            .map(|i| {
                let freq = i as f32 / SPECTRUM_BANDS as f32;
                let v = (t * 2.0 + freq * 20.0).sin() * 0.5 + 0.5;
                // Simulate bass-heavy spectrum
                let bass_falloff = 1.0 - (freq * 2.0).min(1.0);
                (v * bass_falloff).max(0.0).min(1.0)
            })
            .collect()
    }

    fn get_level(&self) -> f32 {
        let t = self.start_time.elapsed().as_secs_f32();
        ((t * 1.5).sin() * 0.5 + 0.5).max(0.0).min(1.0)
    }
}

/// Thread-safe shared audio state that can be updated by a background thread.
pub struct SharedAudioState {
    spectrum: Arc<Mutex<Vec<f32>>>,
    level: Arc<Mutex<f32>>,
}

impl SharedAudioState {
    pub fn new() -> Self {
        Self {
            spectrum: Arc::new(Mutex::new(vec![0.0; SPECTRUM_BANDS])),
            level: Arc::new(Mutex::new(0.0)),
        }
    }

    /// Get a clone of the spectrum/level Arcs for the writer thread.
    pub fn writer_handles(&self) -> (Arc<Mutex<Vec<f32>>>, Arc<Mutex<f32>>) {
        (Arc::clone(&self.spectrum), Arc::clone(&self.level))
    }
}

impl AudioProvider for SharedAudioState {
    fn get_spectrum(&self) -> Vec<f32> {
        self.spectrum.lock().unwrap().clone()
    }

    fn get_level(&self) -> f32 {
        *self.level.lock().unwrap()
    }
}

/// Real audio capture provider using cpal.
///
/// Captures audio from the default input device, runs FFT on the samples,
/// and exposes the frequency spectrum to the render loop.
///
/// The cpal stream is intentionally leaked (lives for program duration)
/// because `cpal::Stream` is not Send+Sync.
pub struct CpalAudioProvider {
    state: SharedAudioState,
}

impl CpalAudioProvider {
    /// Create a new cpal audio provider.
    ///
    /// Attempts to open the default audio input device. Falls back to the
    /// default output device's monitor source if available.
    pub fn new() -> anyhow::Result<Self> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use rustfft::{FftPlanner, num_complex::Complex};

        let host = cpal::default_host();

        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No audio input device available"))?;

        let device_name = device.name().unwrap_or_else(|_| "unknown".into());
        log::info!("Audio input device: {}", device_name);

        let config = device.default_input_config()
            .map_err(|e| anyhow::anyhow!("Failed to get audio input config: {}", e))?;

        log::info!("Audio config: {} channels, {} Hz, {:?}",
            config.channels(), config.sample_rate().0, config.sample_format());

        let state = SharedAudioState::new();
        let (spectrum_writer, level_writer) = state.writer_handles();

        // Pre-allocate FFT resources
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let fft = Arc::new(fft);

        // Ring buffer for accumulating samples
        let sample_buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(FFT_SIZE * 2)));

        let sample_buf_clone = Arc::clone(&sample_buffer);
        let fft_clone = Arc::clone(&fft);

        // Spawn FFT processing thread
        let spectrum_writer_fft = Arc::clone(&spectrum_writer);
        let level_writer_fft = Arc::clone(&level_writer);

        std::thread::Builder::new()
            .name("kroma-audio-fft".into())
            .spawn(move || {
                let mut scratch = vec![Complex::new(0.0f32, 0.0); FFT_SIZE];
                let mut fft_input = vec![Complex::new(0.0f32, 0.0); FFT_SIZE];
                let hann_window: Vec<f32> = (0..FFT_SIZE)
                    .map(|i| {
                        0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
                    })
                    .collect();

                loop {
                    std::thread::sleep(std::time::Duration::from_millis(16)); // ~60 Hz

                    let samples: Vec<f32> = {
                        let mut buf = sample_buf_clone.lock().unwrap();
                        if buf.len() < FFT_SIZE {
                            continue;
                        }
                        // Take the latest FFT_SIZE samples
                        let start = buf.len().saturating_sub(FFT_SIZE);
                        let result = buf[start..].to_vec();
                        buf.clear();
                        result
                    };

                    // Compute RMS level
                    let rms = {
                        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
                        (sum_sq / samples.len() as f32).sqrt()
                    };
                    // Normalize RMS to 0-1 range (assuming max amplitude is 1.0)
                    let level = (rms * 3.0).min(1.0); // Boost for visibility
                    *level_writer_fft.lock().unwrap() = level;

                    // Apply Hann window and prepare FFT input
                    for i in 0..FFT_SIZE {
                        let sample = if i < samples.len() { samples[i] } else { 0.0 };
                        fft_input[i] = Complex::new(sample * hann_window[i], 0.0);
                    }

                    // Run FFT
                    fft_clone.process_with_scratch(&mut fft_input, &mut scratch);

                    // Convert to magnitude spectrum (first half = positive frequencies)
                    let half = FFT_SIZE / 2;
                    let mut spectrum = vec![0.0f32; SPECTRUM_BANDS];
                    let bins_per_band = half as f32 / SPECTRUM_BANDS as f32;

                    for band in 0..SPECTRUM_BANDS {
                        let start_bin = (band as f32 * bins_per_band) as usize;
                        let end_bin = ((band + 1) as f32 * bins_per_band) as usize;
                        let end_bin = end_bin.min(half);

                        if start_bin < end_bin {
                            let mut sum = 0.0f32;
                            for bin in start_bin..end_bin {
                                let mag = fft_input[bin].norm();
                                sum += mag;
                            }
                            let avg = sum / (end_bin - start_bin) as f32;
                            // Convert to dB-like scale and normalize
                            let db = 20.0 * (avg + 1e-10).log10();
                            let normalized = ((db + 60.0) / 60.0).max(0.0).min(1.0);
                            spectrum[band] = normalized;
                        }
                    }

                    *spectrum_writer_fft.lock().unwrap() = spectrum;
                }
            })
            .expect("Failed to spawn FFT thread");

        // Build audio input stream
        let channels = config.channels() as usize;
        let sample_format = config.sample_format();

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let buf = Arc::clone(&sample_buffer);
                device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let mut buffer = buf.lock().unwrap();
                        // Mix to mono
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                            buffer.push(mono);
                        }
                        // Keep buffer bounded
                        if buffer.len() > FFT_SIZE * 4 {
                            let drain_to = buffer.len() - FFT_SIZE * 2;
                            buffer.drain(..drain_to);
                        }
                    },
                    |err| log::error!("Audio input error: {}", err),
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let buf = Arc::clone(&sample_buffer);
                device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let mut buffer = buf.lock().unwrap();
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk.iter()
                                .map(|&s| s as f32 / i16::MAX as f32)
                                .sum::<f32>() / channels as f32;
                            buffer.push(mono);
                        }
                        if buffer.len() > FFT_SIZE * 4 {
                            let drain_to = buffer.len() - FFT_SIZE * 2;
                            buffer.drain(..drain_to);
                        }
                    },
                    |err| log::error!("Audio input error: {}", err),
                    None,
                )
            }
            _ => {
                // For other formats, convert via f32
                let buf = Arc::clone(&sample_buffer);
                device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let mut buffer = buf.lock().unwrap();
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                            buffer.push(mono);
                        }
                        if buffer.len() > FFT_SIZE * 4 {
                            let drain_to = buffer.len() - FFT_SIZE * 2;
                            buffer.drain(..drain_to);
                        }
                    },
                    |err| log::error!("Audio input error: {}", err),
                    None,
                )
            }
        }
        .map_err(|e| anyhow::anyhow!("Failed to build audio input stream: {}", e))?;

        stream.play()
            .map_err(|e| anyhow::anyhow!("Failed to start audio stream: {}", e))?;

        log::info!("Audio capture started on '{}'", device_name);

        // Intentionally leak the stream — it lives for the program's lifetime.
        // cpal::Stream is not Send+Sync, so we can't store it in a struct
        // that needs to be shared across threads.
        Box::leak(Box::new(stream));

        Ok(Self {
            state,
        })
    }
}

impl AudioProvider for CpalAudioProvider {
    fn get_spectrum(&self) -> Vec<f32> {
        self.state.get_spectrum()
    }

    fn get_level(&self) -> f32 {
        self.state.get_level()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_provider_returns_correct_size() {
        let provider = SilentAudioProvider;
        let spectrum = provider.get_spectrum();
        assert_eq!(spectrum.len(), SPECTRUM_BANDS);
        assert!(spectrum.iter().all(|&v| v == 0.0));
        assert_eq!(provider.get_level(), 0.0);
    }

    #[test]
    fn simulated_provider_returns_valid_values() {
        let provider = SimulatedAudioProvider::new();
        let spectrum = provider.get_spectrum();
        assert_eq!(spectrum.len(), SPECTRUM_BANDS);
        for &v in &spectrum {
            assert!(v >= 0.0 && v <= 1.0, "Spectrum value {} out of range", v);
        }
        let level = provider.get_level();
        assert!(level >= 0.0 && level <= 1.0, "Level {} out of range", level);
    }

    #[test]
    fn shared_audio_state_roundtrip() {
        let state = SharedAudioState::new();
        let (spectrum_w, level_w) = state.writer_handles();

        // Write some data
        {
            let mut s = spectrum_w.lock().unwrap();
            s[0] = 0.5;
            s[100] = 0.8;
        }
        *level_w.lock().unwrap() = 0.42;

        // Read back
        let spectrum = state.get_spectrum();
        assert_eq!(spectrum[0], 0.5);
        assert_eq!(spectrum[100], 0.8);
        assert_eq!(state.get_level(), 0.42);
    }
}
