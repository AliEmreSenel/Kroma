//! System data provider implementation.
//!
//! Implements [`kroma_shared::traits::DataProvider`] using `sysinfo` for
//! system stats and Hyprland IPC for cursor position.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use glam::Vec2;
use log::debug;
use sysinfo::System;

use kroma_shared::traits::DataProvider;
use kroma_shared::types::SystemStats;

/// Data provider that gathers system stats and cursor position.
pub struct SystemDataProvider {
    sys: Arc<Mutex<System>>,
    cursor_pos: Arc<Mutex<Vec2>>,
}

impl SystemDataProvider {
    pub fn new() -> Self {
        // Use System::new() instead of System::new_all() — new_all() enumerates
        // every process, disk, network interface, etc. which is extremely slow.
        // We only need CPU usage and memory stats.
        let sys = Arc::new(Mutex::new(System::new()));

        // Do an initial refresh so values are available immediately.
        // Note: CPU usage requires two calls to get meaningful values —
        // the first call establishes a baseline, the second computes deltas.
        {
            let mut s = sys.lock().unwrap();
            s.refresh_cpu_usage();
            s.refresh_memory();
            // Sleep briefly then refresh again so cpu_usage() returns non-zero
            std::thread::sleep(Duration::from_millis(200));
            s.refresh_cpu_usage();
        }

        // Spawn a background thread that refreshes system stats every second
        let sys_clone = Arc::clone(&sys);
        std::thread::Builder::new()
            .name("kroma-sysinfo".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    {
                        let mut s = sys_clone.lock().unwrap();
                        s.refresh_cpu_usage();
                        s.refresh_memory();
                    }
                }
            })
            .expect("Failed to spawn sysinfo thread");

        // Spawn a background thread that queries cursor position.
        // Uses hyprctl on Hyprland, or /dev/input fallback.
        let cursor_pos = Arc::new(Mutex::new(Vec2::new(0.5, 0.5)));
        let cursor_clone = Arc::clone(&cursor_pos);
        let is_hyprland = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").is_ok();
        std::thread::Builder::new()
            .name("kroma-cursor".into())
            .spawn(move || {
                if !is_hyprland {
                    debug!("Not running under Hyprland — cursor tracking disabled");
                    return;
                }
                loop {
                    match query_hyprland_cursor() {
                        Ok(pos) => {
                            *cursor_clone.lock().unwrap() = pos;
                        }
                        Err(_) => {
                            debug!("Could not query Hyprland cursor position");
                        }
                    }
                    std::thread::sleep(Duration::from_millis(33)); // ~30 Hz
                }
            })
            .expect("Failed to spawn cursor thread");

        Self { sys, cursor_pos }
    }
}

impl DataProvider for SystemDataProvider {
    fn get_audio_spectrum(&self) -> Vec<f32> {
        // TODO: Implement Pipewire/Pulse audio FFT capture.
        // For now, return a silent 512-band spectrum.
        vec![0.0; 512]
    }

    fn get_system_stats(&self) -> SystemStats {
        let sys = self.sys.lock().unwrap();
        let cpu_usage = sys.global_cpu_usage();
        let ram_total = sys.total_memory();
        let ram_used = sys.used_memory();

        let battery = read_battery_capacity();

        SystemStats {
            cpu_usage,
            ram_total,
            ram_used,
            battery,
        }
    }

    fn get_cursor_pos(&self) -> Vec2 {
        *self.cursor_pos.lock().unwrap()
    }
}

/// Read battery capacity from sysfs.
///
/// Tries `/sys/class/power_supply/BAT0/capacity` first, then `BAT1`.
/// Returns `None` if neither exists or is readable.
fn read_battery_capacity() -> Option<f32> {
    for bat in &["BAT0", "BAT1"] {
        let path = format!("/sys/class/power_supply/{}/capacity", bat);
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if let Ok(pct) = contents.trim().parse::<f32>() {
                return Some(pct.clamp(0.0, 100.0));
            }
        }
    }
    None
}

/// Query cursor position via Hyprland IPC socket directly (no process spawning).
///
/// Connects to `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`,
/// sends `cursorpos`, reads the response.
fn query_hyprland_cursor() -> anyhow::Result<Vec2> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let instance_sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| anyhow::anyhow!("HYPRLAND_INSTANCE_SIGNATURE not set"))?;

    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .unwrap_or_else(|_| "/tmp".into());

    let socket_path = format!("{}/hypr/{}/.socket.sock", runtime_dir, instance_sig);
    let mut stream = UnixStream::connect(&socket_path)?;
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;

    stream.write_all(b"cursorpos")?;
    // Shutdown write side so server knows command is complete
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut buf = [0u8; 128];
    let n = stream.read(&mut buf)?;
    let response = std::str::from_utf8(&buf[..n]).unwrap_or("");

    // Response format: "960, 540" or "960, 540\n"
    let parts: Vec<&str> = response.trim().split(',').collect();
    if parts.len() == 2 {
        let x: f32 = parts[0].trim().parse().unwrap_or(0.0);
        let y: f32 = parts[1].trim().parse().unwrap_or(0.0);
        Ok(Vec2::new(x, y))
    } else {
        Err(anyhow::anyhow!("Unexpected cursor response: {}", response))
    }
}
