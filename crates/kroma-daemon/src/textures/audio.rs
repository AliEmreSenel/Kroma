//! Audio spectrum texture source.
//!
//! Captures audio from a system device via cpal, runs FFT in a background
//! thread, and produces a 1D R32Float texture with the frequency spectrum.
//! Each `AudioTexture` owns its own capture stream, so multiple audio
//! textures with different sources can coexist.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    thread,
};

use anyhow::{Context, Result, anyhow};
use cpal::{
    Device, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use kroma_shared::{traits::AudioProvider, types::AudioConfig};
use log::info;
use rustfft::{FftPlanner, num_complex::Complex};

use super::{TextureFormat, TextureSource, TextureUpdate};

/// Number of frequency bands in the spectrum.
pub const SPECTRUM_BANDS: usize = 1024;

/// FFT size (must be power of 2, >= SPECTRUM_BANDS * 2).
const FFT_SIZE: usize = 2048;

pub struct SharedAudioState {
    spectrum: Arc<Mutex<Vec<f32>>>,
    level: Arc<Mutex<f32>>,
}

impl SharedAudioState {
    pub fn new(bands: usize) -> Self {
        Self {
            spectrum: Arc::new(Mutex::new(vec![0.0; bands])),
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
        self.spectrum
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn get_level(&self) -> f32 {
        *self.level.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn get_source_by_name(source: &str) -> Result<Device> {
    let host = cpal::default_host();
    match source {
        "microphone" | "mic" => {
            log::info!("Audio: using microphone (default input device)");
            host.default_input_device()
                .ok_or_else(|| anyhow!("No microphone available"))
        }
        "desktop" | "" => {
            #[cfg(target_os = "windows")]
            {
                // Windows: Loopback is captured via the render (output) device.
                // CPAL detects this usage and enables WASAPI loopback automatically.
                info!("Audio (Windows): Selecting default output for Loopback");
                host.default_output_device()
                    .context("No default output device available")
            }

            #[cfg(target_os = "linux")]
            {
                // Linux: We need to find the "Monitor" source.
                // 1. Try to find a device explicitly named "monitor"
                // 2. Fallback to "pulse" or "pipewire" which often default to the monitor in desktop environments.

                info!("Audio (Linux): Searching for monitor device...");

                let mut devices = host
                    .input_devices()
                    .context("Failed to list input devices")?;

                // Priority 1: Explicit Monitor Device
                // Some ALSA setups expose 'hw:0,0' and 'hw:0,1' where one is a monitor.
                // We look for 'monitor' in the name if CPAL exposes it.
                if let Some(dev) = devices.find(|d| {
                    d.description()
                        .map(|desc| desc.name().to_lowercase().contains("monitor"))
                        .unwrap_or(false)
                }) {
                    log::info!(
                        "Audio: Found explicit monitor device: {}",
                        dev.description()
                            .map(|desc| desc.name().to_string())
                            .unwrap_or_default()
                    );
                    Ok(dev)
                } else {
                    // Priority 2: PulseAudio / PipeWire Server
                    // If we can't find a hardware monitor, we connect to the sound server.
                    // The *default* input on a desktop Linux setup is usually the mic,
                    // BUT the "PulseAudio Sound Server" device is often the bridge we need.
                    // We try to find the specific bridge device rather than the default input.
                    let bridge = host.input_devices()?.find(|d| {
                        let name = d
                            .description()
                            .map(|desc| desc.name().to_lowercase())
                            .unwrap_or_default();
                        name == "pulseaudio sound server" || name == "pipewire sound server"
                    });

                    if let Some(dev) = bridge {
                        info!(
                            "Audio: Using Sound Server Bridge: {}",
                            dev.description()
                                .map(|desc| desc.name().to_string())
                                .unwrap_or_default()
                        );
                        Ok(dev)
                    } else {
                        Err(anyhow!("Audio: No monitor or bridge found."))
                    }
                }
            }

            #[cfg(target_os = "macos")]
            {
                return Err(anyhow!(
                    "MacOS doesnt support monitoring desktop output by default. Select the exact output to monitor."
                ));
            }
        }
        device_name => host
            .input_devices()
            .ok()
            .and_then(|devices| {
                devices
                    .filter_map(|d| {
                        match d
                            .description()
                            .map(|desc| desc.name().to_string().contains(device_name))
                            .unwrap_or(false)
                        {
                            true => Some(d),
                            false => None,
                        }
                    })
                    .next()
            })
            .ok_or_else(|| anyhow::anyhow!("Audio device '{}' not found", device_name)),
    }
}

pub struct CpalAudioProvider {
    source: String,
    bands: usize,
    state: SharedAudioState,
    stream: Option<Stream>,
}

impl CpalAudioProvider {
    pub fn close(&mut self) {
        self.source = "none".to_string();
        self.stream = None;
        self.state = SharedAudioState::new(self.bands);
    }

    pub fn switch(&mut self, config: &AudioConfig) -> Result<()> {
        let source = &config.source;
        self.bands = config.fft_bands;
        let bands = self.bands;
        self.source = source.to_string();
        let device = get_source_by_name(source)?;

        let input_config = device.default_input_config()?;
        let sample_rate = input_config.sample_rate() as f32; // Capture sample rate for math
        log::info!(
            "Audio config: {}ch, {}Hz, {:?}",
            input_config.channels(),
            sample_rate,
            input_config.sample_format()
        );

        // --- Shared State Setup ---
        self.state = SharedAudioState::new(self.bands);
        let (spectrum_writer, level_writer) = self.state.writer_handles();

        // Shared Ring Buffer: Audio Thread writes to back, FFT Thread reads snapshot
        let sample_buffer: Arc<Mutex<VecDeque<f32>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(FFT_SIZE)));

        // --- FFT Processing Thread ---
        let buf_read = Arc::clone(&sample_buffer);
        let spec_write = Arc::clone(&spectrum_writer);
        let lvl_write = Arc::clone(&level_writer);

        let mut planner = FftPlanner::new();
        let fft = Arc::new(planner.plan_fft_forward(FFT_SIZE));

        thread::Builder::new()
            .name("kroma-audio-fft".into())
            .spawn(move || {
                let mut input = vec![Complex::new(0.0, 0.0); FFT_SIZE];
                let mut scratch = vec![Complex::new(0.0, 0.0); FFT_SIZE];

                // Pre-compute Hann Window
                let window: Vec<f32> = (0..FFT_SIZE)
                    .map(|i| {
                        0.5 * (1.0
                            - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
                    })
                    .collect();

                loop {
                    // Update at ~60Hz
                    std::thread::sleep(std::time::Duration::from_millis(16));

                    // 1. Snapshot the Ring Buffer (Sliding Window)
                    let samples: Vec<f32> = {
                        let buf = buf_read.lock().unwrap();
                        if buf.len() < FFT_SIZE {
                            continue;
                        } // Wait for buffer to fill
                        buf.iter().copied().collect()
                    };

                    // 2. Compute RMS Level
                    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
                    let rms = (sum_sq / samples.len() as f32).sqrt();
                    // Just max(0.0) here too if you want the level to exceed 1.0
                    *lvl_write.lock().unwrap() = (rms * 4.0).max(0.0);

                    // 3. Prepare FFT Input (Windowing)
                    for (i, &sample) in samples.iter().enumerate() {
                        input[i] = Complex::new(sample * window[i], 0.0);
                    }

                    // 4. Execute FFT
                    fft.process_with_scratch(&mut input, &mut scratch);

                    // 5. Compute Magnitude Spectrum (Logarithmic Mapping)
                    let mut spectrum = vec![0.0f32; bands];
                    let half_size = FFT_SIZE / 2;

                    // --- NEW MATH ---
                    // Restrict mapping from 20 Hz (sub-bass) up to 16 kHz (practical high-end)
                    let min_freq = 20.0f32;
                    let max_freq = 16000.0f32;

                    let min_target_bin = (min_freq * FFT_SIZE as f32 / sample_rate).max(1.0);
                    let max_target_bin =
                        (max_freq * FFT_SIZE as f32 / sample_rate).min(half_size as f32 - 1.0);

                    let min_log = min_target_bin.log2();
                    let max_log = max_target_bin.log2();
                    let log_range = max_log - min_log;

                    for (i, spectrum_val) in spectrum.iter_mut().enumerate().take(bands) {
                        let start_log = min_log + (i as f32 / bands as f32) * log_range;
                        let end_log = min_log + ((i + 1) as f32 / bands as f32) * log_range;

                        let start_bin = (2.0f32.powf(start_log) as usize).min(half_size - 1);
                        let mut end_bin = (2.0f32.powf(end_log) as usize).min(half_size);

                        // Ensure end_bin is strictly greater than start_bin
                        end_bin = end_bin.max(start_bin + 1);

                        // Use fold to find the maximum magnitude in this band instead of averaging
                        let max_mag = input[start_bin..end_bin]
                            .iter()
                            .map(|x| x.norm())
                            .fold(0.0f32, f32::max);

                        // Convert magnitude to normalized dB
                        let db = 20.0 * (max_mag + 1e-6).log10();

                        // No clamping on the top end, just prevent it from going negative
                        *spectrum_val = ((db + 60.0) / 60.0).max(0.0);
                    }

                    *spec_write.lock().unwrap() = spectrum;
                }
            })?;
        // --- Input Stream ---
        let buf_write = sample_buffer.clone();
        let channels = input_config.channels() as usize;

        let err_fn = |err| log::error!("Audio input stream error: {}", err);
        let stream = match input_config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &input_config.into(),
                move |data: &[f32], _: &_| write_input_data(data, channels, &buf_write),
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &input_config.into(),
                move |data: &[i16], _: &_| write_input_data(data, channels, &buf_write),
                err_fn,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &input_config.into(),
                move |data: &[u16], _: &_| write_input_data(data, channels, &buf_write),
                err_fn,
                None,
            ),
            f => return Err(anyhow::anyhow!("Unsupported sample format: {:?}", f)),
        }?;

        stream.play()?;
        self.stream = Some(stream);
        Ok(())
    }

    pub fn new() -> Self {
        Self {
            source: "none".to_string(),
            bands: SPECTRUM_BANDS,
            state: SharedAudioState::new(SPECTRUM_BANDS),
            stream: None,
        }
    }
}

// Helper to write generic sample data into the f32 Ring Buffer
fn write_input_data<T>(input: &[T], channels: usize, buffer: &Arc<Mutex<VecDeque<f32>>>)
where
    T: cpal::Sample + cpal::FromSample<f32>,
    f32: cpal::FromSample<T>,
{
    let mut buf = buffer.lock().unwrap();

    // Mix to Mono and push
    for frame in input.chunks(channels) {
        let mut sum = 0.0;
        for &sample in frame {
            let s: f32 = cpal::FromSample::from_sample_(sample);
            sum += s;
        }
        buf.push_back(sum / channels as f32);
    }

    // Maintain Fixed Size (Drop Oldest)
    while buf.len() > FFT_SIZE {
        buf.pop_front();
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
        info!("AudioTexture loaded: source='{}', bands={}", source, bands);
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
