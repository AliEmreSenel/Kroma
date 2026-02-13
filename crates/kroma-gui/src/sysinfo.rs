//! Lightweight Linux system info readers (no sysinfo crate needed).

/// Read approximate CPU usage from /proc/loadavg (1-minute load average).
/// Returns a percentage estimate: (load1 / num_cpus) * 100, capped at 100.
pub(crate) fn read_cpu_usage_fast() -> f32 {
    let loadavg = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let load1: f32 = loadavg
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);

    // Get number of online CPUs
    let cpus = std::fs::read_to_string("/proc/cpuinfo")
        .map(|s| s.lines().filter(|l| l.starts_with("processor")).count() as f32)
        .unwrap_or(1.0)
        .max(1.0);

    ((load1 / cpus) * 100.0).min(100.0)
}

/// Read used and total memory from /proc/meminfo (in MB).
/// Returns (used_mb, total_mb).
pub(crate) fn read_mem_info() -> (u64, u64) {
    let content = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total_kb = 0u64;
    let mut available_kb = 0u64;

    for line in content.lines() {
        if line.starts_with("MemTotal:") {
            total_kb = parse_meminfo_value(line);
        } else if line.starts_with("MemAvailable:") {
            available_kb = parse_meminfo_value(line);
        }
    }

    let used_kb = total_kb.saturating_sub(available_kb);
    (used_kb / 1024, total_kb / 1024)
}

fn parse_meminfo_value(line: &str) -> u64 {
    line.split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Read battery percentage from sysfs.
pub(crate) fn read_battery_pct() -> Option<f32> {
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
