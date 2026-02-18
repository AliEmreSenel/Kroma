//! Audio spectrum analysis provider.
//!
//! Provides a normalised 512-band audio spectrum for shader consumption.
//! Includes:
//! - `SilentAudioProvider` — returns silence (no audio backend)
//! - `SimulatedAudioProvider` — fake sine-wave spectrum for demos
//! - `CpalAudioProvider` — real audio capture via cpal + FFT

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rustfft::{num_complex::Complex, FftPlanner};

/// Number of frequency bands in the spectrum.
pub const SPECTRUM_BANDS: usize = 1024;

/// FFT size (must be power of 2, >= SPECTRUM_BANDS * 2).
const FFT_SIZE: usize = 2048;

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
        self.spectrum
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn get_level(&self) -> f32 {
        *self.level.lock().unwrap_or_else(|e| e.into_inner())
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
    pub fn new_with_source(source: &str) -> anyhow::Result<Self> {
        let host = cpal::default_host();

        // --- Device Selection Logic ---
        let device = match source {
            "microphone" | "mic" => {
                log::info!("Audio: using microphone (default input device)");
                host.default_input_device()
                    .ok_or_else(|| anyhow::anyhow!("No microphone available"))?
            }
            "desktop" | "" => {
                // Try to find monitor source via pactl (Linux PulseAudio/PipeWire)
                let pa_monitor = get_default_sink_monitor();

                let monitor_device = if let Some(ref monitor_name) = pa_monitor {
                    log::info!("Audio: PulseAudio default sink monitor: {}", monitor_name);

                    // Try direct match first
                    let direct = host.input_devices().ok().and_then(|devices| {
                        devices
                            .filter_map(|d| {
                                let name = d.name().ok()?;
                                if name == *monitor_name {
                                    Some(d)
                                } else {
                                    None
                                }
                            })
                            .next()
                    });

                    if direct.is_some() {
                        direct
                    } else {
                        // Fallback: Set PULSE_SOURCE and look for "pulse" or "default" ALSA device
                        std::env::set_var("PULSE_SOURCE", monitor_name);
                        host.input_devices().ok().and_then(|devices| {
                            devices
                                .filter_map(|d| {
                                    let name = d.name().ok()?;
                                    if name == "pulse" || name == "default" {
                                        Some(d)
                                    } else {
                                        None
                                    }
                                })
                                .next()
                        })
                    }
                } else {
                    None
                };

                // Fallback: substring match "monitor"
                let monitor_device = monitor_device.or_else(|| {
                    host.input_devices().ok().and_then(|devices| {
                        devices
                            .filter_map(|d| {
                                let name = d.name().ok()?.to_lowercase();
                                if name.contains("monitor") {
                                    Some(d)
                                } else {
                                    None
                                }
                            })
                            .next()
                    })
                });

                if let Some(dev) = monitor_device {
                    log::info!(
                        "Audio: using desktop monitor: {}",
                        dev.name().unwrap_or_default()
                    );
                    dev
                } else {
                    return Err(anyhow::anyhow!("No desktop audio monitor source found."));
                }
            }
            device_name => host
                .input_devices()
                .ok()
                .and_then(|devices| {
                    devices
                        .filter_map(|d| {
                            let name = d.name().ok()?;
                            if name.contains(device_name) {
                                Some(d)
                            } else {
                                None
                            }
                        })
                        .next()
                })
                .ok_or_else(|| anyhow::anyhow!("Audio device '{}' not found", device_name))?,
        };

        let config = device.default_input_config()?;
        log::info!(
            "Audio config: {}ch, {}Hz, {:?}",
            config.channels(),
            config.sample_rate(),
            config.sample_format()
        );

        // --- Shared State Setup ---
        let state = SharedAudioState::new();
        let (spectrum_writer, level_writer) = state.writer_handles();

        // Shared Ring Buffer: Audio Thread writes to back, FFT Thread reads snapshot
        let sample_buffer: Arc<Mutex<VecDeque<f32>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(FFT_SIZE)));

        // --- FFT Processing Thread ---
        let buf_read = Arc::clone(&sample_buffer);
        let spec_write = Arc::clone(&spectrum_writer);
        let lvl_write = Arc::clone(&level_writer);

        let mut planner = FftPlanner::new();
        let fft = Arc::new(planner.plan_fft_forward(FFT_SIZE));

        std::thread::Builder::new()
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
                    *lvl_write.lock().unwrap() = (rms * 4.0).clamp(0.0, 1.0);

                    // 3. Prepare FFT Input (Windowing)
                    for (i, &sample) in samples.iter().enumerate() {
                        input[i] = Complex::new(sample * window[i], 0.0);
                    }

                    // 4. Execute FFT
                    fft.process_with_scratch(&mut input, &mut scratch);

                    // 5. Compute Magnitude Spectrum (Linear -> Logarithmic Mapping)
                    let mut spectrum = vec![0.0f32; SPECTRUM_BANDS];
                    let half_size = FFT_SIZE / 2;

                    // Simple linear mapping for now (can be swapped for log mapping if shader expects it)
                    // The shader we wrote handles log mapping on the texture coordinate side.
                    let bins_per_band = half_size as f32 / SPECTRUM_BANDS as f32;

                    for i in 0..SPECTRUM_BANDS {
                        let start_bin = (i as f32 * bins_per_band) as usize;
                        let end_bin = ((i + 1) as f32 * bins_per_band) as usize;
                        let end_bin = end_bin.max(start_bin + 1).min(half_size);

                        let mut mag_sum = 0.0;
                        for bin in start_bin..end_bin {
                            mag_sum += input[bin].norm();
                        }
                        let avg_mag = mag_sum / (end_bin - start_bin) as f32;

                        // Convert magnitude to normalized dB (roughly)
                        // Log10 of 0.001 (-60dB) to 1.0 (0dB)
                        let db = 20.0 * (avg_mag + 1e-6).log10();
                        let val = ((db + 60.0) / 60.0).max(0.0);

                        spectrum[i] = val;
                    }

                    *spec_write.lock().unwrap() = spectrum;
                }
            })?;

        // --- Input Stream ---
        let buf_write = (&sample_buffer).clone();
        let channels = config.channels() as usize;

        let err_fn = |err| log::error!("Audio input stream error: {}", err);
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &_| write_input_data(data, channels, &buf_write),
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &_| write_input_data(data, channels, &buf_write),
                err_fn,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &config.into(),
                move |data: &[u16], _: &_| write_input_data(data, channels, &buf_write),
                err_fn,
                None,
            ),
            f => return Err(anyhow::anyhow!("Unsupported sample format: {:?}", f)),
        }?;

        stream.play()?;

        // Leak the stream to keep it alive
        Box::leak(Box::new(stream));

        Ok(Self { state })
    }

    pub fn new() -> anyhow::Result<Self> {
        Self::new_with_source("desktop")
    }
}

// Helper to write generic sample data into the f32 Ring Buffer
fn write_input_data<T>(input: &[T], channels: usize, buffer: &Arc<Mutex<VecDeque<f32>>>)
where
    T: cpal::Sample + cpal::FromSample<f32>, // Wait, cpal traits are tricky. Let's use f32 conversion directly.
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

/// Query PulseAudio/PipeWire for the monitor source of the current default sink.
///
/// Runs `pactl get-default-sink` to get the default output device name, then
/// appends ".monitor" which is the standard PulseAudio naming convention for
/// the loopback/monitor source of a sink.
///
/// Returns `None` if pactl is not available or the command fails.
fn get_default_sink_monitor() -> Option<String> {
    use std::process::Command;

    // Get the default sink name (e.g. "alsa_output.pci-0000_00_1f.3.analog-stereo")
    let output = Command::new("pactl")
        .arg("get-default-sink")
        .output()
        .ok()?;

    if !output.status.success() {
        log::debug!("pactl get-default-sink failed (status {})", output.status);
        return None;
    }

    let sink_name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sink_name.is_empty() {
        return None;
    }

    // The monitor source is conventionally "<sink_name>.monitor"
    let monitor_name = format!("{}.monitor", sink_name);
    log::debug!(
        "Default sink: {} → monitor source: {}",
        sink_name,
        monitor_name
    );
    Some(monitor_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_audio_state_roundtrip() {
        let state = SharedAudioState::new();
        let (spectrum_w, level_w) = state.writer_handles();

        // Write some data
        {
            let mut s = spectrum_w.lock().unwrap_or_else(|e| e.into_inner());
            s[0] = 0.5;
            s[100] = 0.8;
        }
        *level_w.lock().unwrap_or_else(|e| e.into_inner()) = 0.42;

        // Read back
        let spectrum = state.get_spectrum();
        assert_eq!(spectrum[0], 0.5);
        assert_eq!(spectrum[100], 0.8);
        assert_eq!(state.get_level(), 0.42);
    }
}
