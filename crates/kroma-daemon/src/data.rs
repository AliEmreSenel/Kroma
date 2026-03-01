//! System data provider implementation.
//!
//! Implements [`kroma_shared::traits::DataProvider`] using `sysinfo` for
//! system stats.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use glam::Vec2;
use sysinfo::System;

use kroma_shared::traits::DataProvider;
use kroma_shared::types::SystemStats;

/// Data provider that gathers system stats and cursor position.
pub struct SystemDataProvider {
    sys: Arc<Mutex<System>>,
}

impl SystemDataProvider {
    pub fn new() -> anyhow::Result<Self> {
        // Use System::new() instead of System::new_all() — new_all() enumerates
        // every process, disk, network interface, etc. which is extremely slow.
        // We only need CPU usage and memory stats.
        let sys = Arc::new(Mutex::new(System::new()));

        // Do an initial refresh so values are available immediately.
        // Note: CPU usage requires two calls to get meaningful values —
        // the first call establishes a baseline, the second computes deltas.
        {
            let mut s = sys.lock().unwrap_or_else(|e| e.into_inner());
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
                        let mut s = sys_clone.lock().unwrap_or_else(|e| e.into_inner());
                        s.refresh_cpu_usage();
                        s.refresh_memory();
                    }
                }
            })
            .context("Failed to spawn sysinfo thread")?;

        Ok(Self { sys })
    }
}

impl DataProvider for SystemDataProvider {
    fn get_system_stats(&self) -> SystemStats {
        let sys = self.sys.lock().unwrap_or_else(|e| e.into_inner());
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
        Vec2::new(0.5, 0.5)
    }
}

/// Read battery capacity from sysfs.
///
/// Tries `/sys/class/power_supply/BAT0/capacity` first, then `BAT1`.
/// Returns `None` if neither exists or is readable.
fn read_battery_capacity() -> Option<f32> {
    for bat in &["BAT0", "BAT1"] {
        let path = format!("/sys/class/power_supply/{}/capacity", bat);
        if let Ok(contents) = std::fs::read_to_string(&path)
            && let Ok(pct) = contents.trim().parse::<f32>()
        {
            return Some(pct.clamp(0.0, 100.0));
        }
    }
    None
}
