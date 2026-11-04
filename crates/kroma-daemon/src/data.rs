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

use crate::worker::PeriodicWorker;

/// Data provider that gathers system stats and cursor position.
pub struct SystemDataProvider {
    snapshot: Arc<Mutex<SystemStats>>,
    _worker: PeriodicWorker,
}

impl SystemDataProvider {
    /// Starts a one-second system-stat sampler with a cached render-thread snapshot.
    pub fn new() -> anyhow::Result<Self> {
        let _timing = crate::timing::scope("startup.system_stats");
        // Use System::new() instead of System::new_all() — new_all() enumerates
        // every process, disk, network interface, etc. which is extremely slow.
        // We only need CPU usage and memory stats.
        let mut sys = System::new();

        // Publish memory/battery immediately. The first CPU sample establishes
        // a baseline; the background refresh computes its first usage delta.
        sys.refresh_cpu_usage();
        sys.refresh_memory();

        let snapshot = Arc::new(Mutex::new(collect_stats(&sys)));
        let published = Arc::clone(&snapshot);
        let worker = PeriodicWorker::spawn("kroma-sysinfo", Duration::from_secs(1), move || {
            // Keep sysinfo and sysfs I/O off the render thread and outside the
            // snapshot lock. Readers only contend for copying a small struct.
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            let stats = collect_stats(&sys);
            *published.lock().unwrap_or_else(|e| e.into_inner()) = stats;
        })
        .context("Failed to spawn sysinfo thread")?;

        Ok(Self {
            snapshot,
            _worker: worker,
        })
    }
}

impl DataProvider for SystemDataProvider {
    fn get_system_stats(&self) -> SystemStats {
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn get_cursor_pos(&self) -> Vec2 {
        Vec2::new(0.5, 0.5)
    }
}

fn collect_stats(sys: &System) -> SystemStats {
    SystemStats {
        cpu_usage: sys.global_cpu_usage(),
        ram_total: sys.total_memory(),
        ram_used: sys.used_memory(),
        battery: read_battery_capacity(),
    }
}

/// Read battery capacity from sysfs.
///
/// Tries `/sys/class/power_supply/BAT0/capacity` first, then `BAT1`.
/// Returns `None` if neither exists or is readable.
fn read_battery_capacity() -> Option<f32> {
    for path in [
        "/sys/class/power_supply/BAT0/capacity",
        "/sys/class/power_supply/BAT1/capacity",
    ] {
        if let Ok(contents) = std::fs::read_to_string(path)
            && let Ok(pct) = contents.trim().parse::<f32>()
        {
            return Some(pct.clamp(0.0, 100.0));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_reads_the_published_snapshot() {
        let provider = SystemDataProvider::new().unwrap();
        *provider.snapshot.lock().unwrap() = SystemStats {
            cpu_usage: 12.5,
            ram_total: 1024,
            ram_used: 256,
            battery: Some(80.0),
        };
        let stats = provider.get_system_stats();
        assert_eq!(stats.cpu_usage, 12.5);
        assert_eq!(stats.ram_total, 1024);
        assert_eq!(stats.ram_used, 256);
        assert_eq!(stats.battery, Some(80.0));
        assert_eq!(provider.get_cursor_pos(), Vec2::new(0.5, 0.5));
    }
}
