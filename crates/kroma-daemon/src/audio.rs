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
                (v * bass_falloff).clamp(0.0, 1.0)
            })
            .collect()
    }

    fn get_level(&self) -> f32 {
        let t = self.start_time.elapsed().as_secs_f32();
        ((t * 1.5).sin() * 0.5 + 0.5).clamp(0.0, 1.0)
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
        self.spectrum.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
    ///
    /// Prefers desktop audio (monitor/loopback) over microphone.
    /// Set `source` to "desktop" (default), "microphone", or a specific device name.
    pub fn new_with_source(source: &str) -> anyhow::Result<Self> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use rustfft::{FftPlanner, num_complex::Complex};

        let host = cpal::default_host();

        // Log all available input devices for debugging
        if let Ok(devices) = host.input_devices() {
            log::info!("Available audio input devices:");
            for d in devices {
                if let Ok(name) = d.name() {
                    log::info!("  - {}", name);
                }
            }
        }

        let device = match source {
            "microphone" | "mic" => {
                log::info!("Audio: using microphone (default input device)");
                host.default_input_device()
                    .ok_or_else(|| anyhow::anyhow!("No microphone available"))?
            }
            "desktop" | "" => {
                // First, try to get the exact monitor source name for the current
                // default audio output using pactl (PulseAudio/PipeWire).
                // This gives us e.g. "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor"
                let pa_monitor = get_default_sink_monitor();

                let monitor_device = if let Some(ref monitor_name) = pa_monitor {
                    log::info!("Audio: PulseAudio default sink monitor: {}", monitor_name);
                    host.input_devices().ok().and_then(|devices| {
                        devices
                            .filter_map(|d| {
                                let name = d.name().ok()?;
                                // Try exact match first, then partial
                                if name == *monitor_name || name.contains(monitor_name.as_str()) {
                                    Some(d)
                                } else {
                                    None
                                }
                            })
                            .next()
                    })
                } else {
                    log::warn!("Audio: pactl not available, cannot detect default sink monitor");
                    None
                };

                // If pactl approach didn't work, fall back to substring matching
                let monitor_device = monitor_device.or_else(|| {
                    log::info!("Audio: Trying substring match for monitor device...");
                    host.input_devices().ok().and_then(|devices| {
                        // Collect devices and prefer ones with "monitor" in the name
                        let all: Vec<_> = devices.collect();
                        // First pass: look for ".monitor" (PulseAudio/PipeWire convention)
                        for d in &all {
                            if let Ok(name) = d.name() {
                                if name.to_lowercase().contains(".monitor") {
                                    log::info!("Audio: found monitor device via substring: {}", name);
                                    // We need to return owned device, re-enumerate
                                    drop(all);
                                    return host.input_devices().ok().and_then(|mut devs| {
                                        devs.find(|d2| d2.name().ok().as_deref() == Some(&name))
                                    });
                                }
                            }
                        }
                        // Second pass: look for loopback/desktop/output
                        for d in &all {
                            if let Ok(name) = d.name() {
                                let lower = name.to_lowercase();
                                if lower.contains("loopback")
                                    || lower.contains("desktop")
                                    || lower.contains("output")
                                {
                                    log::info!("Audio: found fallback device via substring: {}", name);
                                    drop(all);
                                    return host.input_devices().ok().and_then(|mut devs| {
                                        devs.find(|d2| d2.name().ok().as_deref() == Some(&name))
                                    });
                                }
                            }
                        }
                        None
                    })
                });

                if let Some(dev) = monitor_device {
                    let name = dev.name().unwrap_or_default();
                    log::info!("Audio: using desktop monitor device: {}", name);
                    dev
                } else {
                    // No monitor device found — do NOT fall back to microphone.
                    // Return an error so the caller can use SilentAudioProvider.
                    log::error!("Audio: no monitor/loopback device found for desktop audio capture. \
                        Ensure PipeWire/PulseAudio is running and has a monitor source for your output device.");
                    return Err(anyhow::anyhow!("No desktop audio monitor source found. \
                        Run 'pactl list sources short' to see available sources."));
                }
            }
            device_name => {
                // User specified a device name directly
                let device = host.input_devices().ok()
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
                    .ok_or_else(|| anyhow::anyhow!("Audio device '{}' not found", device_name))?;
                let name = device.name().unwrap_or_default();
                log::info!("Audio: using specified device: {}", name);
                device
            }
        };

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
                        let mut buf = sample_buf_clone.lock().unwrap_or_else(|e| e.into_inner());
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
                    *level_writer_fft.lock().unwrap_or_else(|e| e.into_inner()) = level;

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

                    for (band, spectrum_val) in spectrum.iter_mut().enumerate().take(SPECTRUM_BANDS) {
                        let start_bin = (band as f32 * bins_per_band) as usize;
                        let end_bin = ((band + 1) as f32 * bins_per_band) as usize;
                        let end_bin = end_bin.min(half);

                        if start_bin < end_bin {
                            let mut sum = 0.0f32;
                            for item in fft_input.iter().take(end_bin).skip(start_bin) {
                                let mag = item.norm();
                                sum += mag;
                            }
                            let avg = sum / (end_bin - start_bin) as f32;
                            // Convert to dB-like scale and normalize
                            let db = 20.0 * (avg + 1e-10).log10();
                            let normalized = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
                            *spectrum_val = normalized;
                        }
                    }

                    *spectrum_writer_fft.lock().unwrap_or_else(|e| e.into_inner()) = spectrum;
                }
            })
            .map_err(|e| anyhow::anyhow!("Failed to spawn FFT thread: {}", e))?;

        // Build audio input stream
        let channels = config.channels() as usize;
        let sample_format = config.sample_format();

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let buf = Arc::clone(&sample_buffer);
                device.build_input_stream(
                    &config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        let mut buffer = buf.lock().unwrap_or_else(|e| e.into_inner());
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
                        let mut buffer = buf.lock().unwrap_or_else(|e| e.into_inner());
                        for chunk in data.chunks(channels) {
                            let mono: f32 = chunk.iter()
                                .map(|&s| s as f32 / 32768.0)
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
                        let mut buffer = buf.lock().unwrap_or_else(|e| e.into_inner());
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

    /// Convenience constructor that defaults to desktop audio.
    pub fn new() -> anyhow::Result<Self> {
        Self::new_with_source("desktop")
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
    log::debug!("Default sink: {} → monitor source: {}", sink_name, monitor_name);
    Some(monitor_name)
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
