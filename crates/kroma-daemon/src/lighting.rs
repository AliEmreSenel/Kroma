//! External lighting output backends.

use std::{
    collections::BTreeMap,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use kroma_shared::traits::{LightingFrame, LightingSink};
use openrgb_client::{Client, Color, Controller, DeviceType};

use crate::config::{OpenRgbConfig, OpenRgbKeyboardConfig};

const CLIENT_NAME: &str = "Kroma";
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);
const INITIAL_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

/// Non-blocking OpenRGB lighting sink backed by a dedicated worker thread.
pub struct OpenRgbLightingSink {
    shared: Arc<(Mutex<WorkerState>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl OpenRgbLightingSink {
    /// Starts an idle OpenRGB worker that connects only after receiving a frame.
    pub fn start(config: OpenRgbConfig, monitor_name: String) -> Result<Self> {
        let shared = Arc::new((Mutex::new(WorkerState::default()), Condvar::new()));
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("kroma-openrgb".to_string())
            .spawn(move || run_worker(worker_shared, config, monitor_name))
            .context("Failed to start OpenRGB worker")?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    fn mutate_state(&self, mutate: impl FnOnce(&mut WorkerState)) -> Result<()> {
        let (lock, wake) = &*self.shared;
        let mut state = lock
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenRGB worker state is poisoned"))?;
        if state.shutdown {
            anyhow::bail!("OpenRGB worker has shut down");
        }
        mutate(&mut state);
        state.generation = state.generation.wrapping_add(1);
        wake.notify_one();
        Ok(())
    }
}

impl LightingSink for OpenRgbLightingSink {
    fn submit_frame(&self, frame: LightingFrame) -> Result<()> {
        self.mutate_state(|state| {
            state.connected_desired = true;
            state.pending_frame = Some(frame);
        })
    }

    fn disconnect(&self) -> Result<()> {
        self.mutate_state(|state| {
            state.connected_desired = false;
            state.pending_frame = None;
        })
    }
}

impl Drop for OpenRgbLightingSink {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        if let Ok(mut state) = lock.lock() {
            state.shutdown = true;
            state.generation = state.generation.wrapping_add(1);
            wake.notify_one();
        }
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            log::warn!("OpenRGB worker panicked during shutdown");
        }
    }
}

#[derive(Default)]
struct WorkerState {
    generation: u64,
    connected_desired: bool,
    pending_frame: Option<LightingFrame>,
    shutdown: bool,
}

struct ConnectedClient {
    client: Client,
    keyboards: Vec<KeyboardTarget>,
}

struct KeyboardTarget {
    controller_id: u32,
    name: String,
    colors: Vec<Color>,
    samples: Vec<LedSample>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct LedSample {
    led_index: usize,
    u: f32,
    v: f32,
}

fn run_worker(
    shared: Arc<(Mutex<WorkerState>, Condvar)>,
    config: OpenRgbConfig,
    monitor_name: String,
) {
    let mut observed_generation = 0;
    let mut connection: Option<ConnectedClient> = None;
    let mut reconnect_delay = INITIAL_RECONNECT_DELAY;
    let mut next_reconnect = Instant::now();

    loop {
        let (connected_desired, frame, shutdown) = {
            let (lock, wake) = &*shared;
            let mut state = match lock.lock() {
                Ok(state) => state,
                Err(_) => {
                    log::error!("OpenRGB worker state was poisoned");
                    return;
                }
            };
            while state.generation == observed_generation && !state.shutdown {
                state = match wake.wait(state) {
                    Ok(state) => state,
                    Err(_) => {
                        log::error!("OpenRGB worker state was poisoned while waiting");
                        return;
                    }
                };
            }
            observed_generation = state.generation;
            (
                state.connected_desired,
                state.pending_frame.take(),
                state.shutdown,
            )
        };

        if shutdown {
            return;
        }
        if !connected_desired {
            if connection.take().is_some() {
                log::info!("Disconnected from OpenRGB and relinquished keyboard control");
            }
            reconnect_delay = INITIAL_RECONNECT_DELAY;
            next_reconnect = Instant::now();
            continue;
        }
        let Some(mut frame) = frame else {
            continue;
        };

        if connection.is_none() {
            if Instant::now() < next_reconnect {
                continue;
            }
            match connect(&config, &monitor_name) {
                Ok(client) => {
                    reconnect_delay = INITIAL_RECONNECT_DELAY;
                    connection = Some(client);
                }
                Err(error) => {
                    log::warn!(
                        "OpenRGB connection failed: {:#}; retrying in {}s",
                        error,
                        reconnect_delay.as_secs()
                    );
                    next_reconnect = Instant::now() + reconnect_delay;
                    reconnect_delay = (reconnect_delay * 2).min(MAX_RECONNECT_DELAY);
                    continue;
                }
            }
        }

        // Connecting may block for up to the configured timeout. Reconcile any
        // commands that arrived meanwhile so an unload cannot emit a stale
        // frame after it requested disconnection.
        {
            let (lock, _) = &*shared;
            let mut state = match lock.lock() {
                Ok(state) => state,
                Err(_) => {
                    log::error!("OpenRGB worker state was poisoned");
                    return;
                }
            };
            if state.generation != observed_generation {
                observed_generation = state.generation;
                if state.shutdown {
                    return;
                }
                if !state.connected_desired {
                    connection = None;
                    continue;
                }
                if let Some(latest_frame) = state.pending_frame.take() {
                    frame = latest_frame;
                }
            }
        }

        let result = connection
            .as_mut()
            .context("OpenRGB connection disappeared")
            .and_then(|connection| update_keyboards(connection, &frame, &config));
        if let Err(error) = result {
            log::warn!(
                "OpenRGB update failed: {:#}; reconnecting in {}s",
                error,
                reconnect_delay.as_secs()
            );
            connection = None;
            next_reconnect = Instant::now() + reconnect_delay;
            reconnect_delay = (reconnect_delay * 2).min(MAX_RECONNECT_DELAY);
        }
    }
}

fn connect(config: &OpenRgbConfig, monitor_name: &str) -> Result<ConnectedClient> {
    let mut client = Client::connect(
        (config.host.as_str(), config.port),
        CLIENT_NAME,
        CONNECTION_TIMEOUT,
    )
    .with_context(|| format!("Could not connect to {}:{}", config.host, config.port))?;
    let protocol = client.protocol_version();
    let controllers = client
        .controllers()
        .context("Controller discovery failed")?;
    let mut keyboards = Vec::new();

    for controller in controllers {
        if controller.device_type != DeviceType::Keyboard
            || !matches_controller(&controller, &config.keyboards, monitor_name)
        {
            continue;
        }
        let Some(target) = keyboard_target(&controller) else {
            log::warn!(
                "Ignoring OpenRGB keyboard '{}' because it has no usable matrix metadata",
                controller.name
            );
            continue;
        };
        client
            .set_custom_mode(controller.id)
            .with_context(|| format!("Failed setting '{}' to Direct mode", controller.name))?;
        log::info!(
            "Controlling OpenRGB keyboard '{}' ({} mapped LEDs)",
            target.name,
            target.samples.len()
        );
        keyboards.push(target);
    }

    if keyboards.is_empty() {
        anyhow::bail!("No configured OpenRGB keyboards with matrix metadata were found");
    }
    log::info!(
        "Connected to OpenRGB {}:{} using protocol {}",
        config.host,
        config.port,
        protocol
    );
    Ok(ConnectedClient { client, keyboards })
}

fn matches_controller(
    controller: &Controller,
    selectors: &[OpenRgbKeyboardConfig],
    monitor_name: &str,
) -> bool {
    selectors.iter().any(|selector| {
        let name_matches = glob_matches_case_insensitive(&selector.name, &controller.name);
        let serial_matches = selector
            .serial
            .as_deref()
            .filter(|serial| !serial.is_empty())
            .is_none_or(|serial| serial == controller.serial);
        let monitor_matches = selector
            .monitor
            .as_deref()
            .filter(|monitor| !monitor.is_empty())
            .is_none_or(|monitor| monitor.eq_ignore_ascii_case(monitor_name));
        name_matches && serial_matches && monitor_matches
    })
}

fn keyboard_target(controller: &Controller) -> Option<KeyboardTarget> {
    let mut samples = BTreeMap::new();
    for zone in &controller.zones {
        let Some(matrix) = zone.matrix.as_ref() else {
            continue;
        };
        if matrix.width == 0 || matrix.height == 0 {
            continue;
        }
        for (offset, led_index) in matrix.leds.iter().enumerate() {
            let Some(led_index) = *led_index else {
                continue;
            };
            if led_index >= controller.colors.len() {
                continue;
            }
            let x = u32::try_from(offset).ok()? % matrix.width;
            let y = u32::try_from(offset).ok()? / matrix.width;
            let u = if matrix.width == 1 {
                0.5
            } else {
                x as f32 / (matrix.width - 1) as f32
            };
            let v = if matrix.height == 1 {
                0.5
            } else {
                y as f32 / (matrix.height - 1) as f32
            };
            samples.insert(led_index, LedSample { led_index, u, v });
        }
    }
    if samples.is_empty() {
        return None;
    }
    Some(KeyboardTarget {
        controller_id: controller.id,
        name: controller.name.clone(),
        colors: controller.colors.clone(),
        samples: samples.into_values().collect(),
    })
}

fn update_keyboards(
    connection: &mut ConnectedClient,
    frame: &LightingFrame,
    config: &OpenRgbConfig,
) -> Result<()> {
    let brightness = config.effective_brightness();
    let gamma = config.effective_gamma();
    for keyboard in &mut connection.keyboards {
        for sample in &keyboard.samples {
            keyboard.colors[sample.led_index] =
                sample_color(frame, sample.u, sample.v, brightness, gamma);
        }
        connection
            .client
            .update_leds(keyboard.controller_id, &keyboard.colors)
            .with_context(|| format!("Failed updating keyboard '{}'", keyboard.name))?;
    }
    Ok(())
}

fn sample_color(frame: &LightingFrame, u: f32, v: f32, brightness: f32, gamma: f32) -> Color {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    if width == 0 || height == 0 {
        return Color::default();
    }

    let x = u.clamp(0.0, 1.0) * width.saturating_sub(1) as f32;
    let y = v.clamp(0.0, 1.0) * height.saturating_sub(1) as f32;
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;

    let channel = |channel: usize| {
        let read =
            |px: usize, py: usize| frame.rgba()[(py * width + px) * 4 + channel] as f32 / 255.0;
        let top = read(x0, y0) * (1.0 - tx) + read(x1, y0) * tx;
        let bottom = read(x0, y1) * (1.0 - tx) + read(x1, y1) * tx;
        let value = top * (1.0 - ty) + bottom * ty;
        (value.powf(gamma) * brightness * 255.0)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::new(channel(0), channel(1), channel(2))
}

fn glob_matches_case_insensitive(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let value: Vec<char> = value.to_lowercase().chars().collect();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;

    for token in pattern {
        let mut current = vec![false; value.len() + 1];
        match token {
            '*' => {
                current[0] = previous[0];
                for index in 1..=value.len() {
                    current[index] = previous[index] || current[index - 1];
                }
            }
            '?' => {
                current[1..].copy_from_slice(&previous[..value.len()]);
            }
            literal => {
                for index in 1..=value.len() {
                    current[index] = previous[index - 1] && literal == value[index - 1];
                }
            }
        }
        previous = current;
    }
    previous[value.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use openrgb_client::{MatrixMap, Zone};

    #[test]
    fn glob_matching_is_case_insensitive() {
        assert!(glob_matches_case_insensitive("*keyboard*", "ASUS Keyboard"));
        assert!(glob_matches_case_insensitive("kbd-?", "KBD-A"));
        assert!(!glob_matches_case_insensitive("kbd-?", "KBD-AB"));
    }

    #[test]
    fn serial_disambiguation_is_exact() {
        let controller = controller_fixture();
        let selector = OpenRgbKeyboardConfig {
            name: "*board".to_string(),
            serial: Some("ABC123".to_string()),
            monitor: None,
        };
        assert!(matches_controller(
            &controller,
            std::slice::from_ref(&selector),
            "DP-1"
        ));

        let mut wrong_case = selector;
        wrong_case.serial = Some("abc123".to_string());
        assert!(!matches_controller(&controller, &[wrong_case], "DP-1"));
    }

    #[test]
    fn matrix_maps_to_normalized_key_positions() {
        let target = keyboard_target(&controller_fixture()).unwrap();
        assert_eq!(target.samples.len(), 4);
        assert_eq!(
            target.samples[0],
            LedSample {
                led_index: 0,
                u: 0.0,
                v: 0.0
            }
        );
        assert_eq!(
            target.samples[3],
            LedSample {
                led_index: 3,
                u: 1.0,
                v: 1.0
            }
        );
    }

    #[test]
    fn frame_sampling_interpolates_and_applies_brightness() {
        let frame = LightingFrame::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        assert_eq!(
            sample_color(&frame, 0.0, 0.0, 1.0, 1.0),
            Color::new(255, 0, 0)
        );
        assert_eq!(
            sample_color(&frame, 1.0, 0.0, 1.0, 1.0),
            Color::new(0, 0, 255)
        );
        assert_eq!(
            sample_color(&frame, 0.5, 0.0, 0.5, 1.0),
            Color::new(64, 0, 64)
        );
    }

    fn controller_fixture() -> Controller {
        Controller {
            id: 2,
            device_type: DeviceType::Keyboard,
            name: "Test Keyboard".to_string(),
            vendor: "Test".to_string(),
            serial: "ABC123".to_string(),
            zones: vec![Zone {
                name: "Keys".to_string(),
                led_count: 4,
                matrix: Some(MatrixMap {
                    height: 2,
                    width: 2,
                    leds: vec![Some(0), Some(1), Some(2), Some(3)],
                }),
            }],
            colors: vec![Color::default(); 4],
        }
    }
}
