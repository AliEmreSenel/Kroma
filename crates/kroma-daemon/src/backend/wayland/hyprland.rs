//! Hyprland IPC event listener.
//!
//! Monitors Hyprland's event socket (`socket2`) for compositor events
//! that affect rendering behaviour — fullscreen windows, workspace changes,
//! active monitor changes, etc.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{Context, Result};
use glam::Vec2;
use log::{debug, error, info, warn};

use super::WaylandEvent;

/// Returns the path to Hyprland's event socket (socket2).
fn hyprland_socket2_path() -> Result<String> {
    let instance_sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .context("HYPRLAND_INSTANCE_SIGNATURE not set — not running under Hyprland?")?;
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    Ok(format!(
        "{}/hypr/{}/.socket2.sock",
        runtime_dir, instance_sig
    ))
}

/// Start the Hyprland event listener on a background thread.
///
/// Events are sent through `tx`. The returned `JoinHandle` can be used
/// to monitor the thread, but it runs indefinitely until disconnected.
pub fn start_listener(tx: Sender<WaylandEvent>) -> Result<std::thread::JoinHandle<()>> {
    let socket_path = hyprland_socket2_path()?;
    info!("Connecting to Hyprland event socket: {}", socket_path);

    let handle = std::thread::Builder::new()
        .name("kroma-hypr-events".into())
        .spawn(move || {
            // Retry connection with backoff
            let mut retry_count = 0;
            loop {
                match UnixStream::connect(&socket_path) {
                    Ok(stream) => {
                        retry_count = 0;
                        info!("Connected to Hyprland event socket");
                        process_events(&stream, &tx);
                    }
                    Err(e) => {
                        retry_count += 1;
                        if retry_count <= 5 {
                            warn!(
                                "Failed to connect to Hyprland socket (attempt {}): {}",
                                retry_count, e
                            );
                        } else {
                            error!(
                                "Giving up on Hyprland event socket after {} attempts",
                                retry_count
                            );
                            let _ = tx.send(WaylandEvent::Disconnected);
                            return;
                        }
                    }
                }

                let backoff = Duration::from_secs(retry_count.clamp(1, 30));
                std::thread::sleep(backoff);
            }
        })
        .context("Failed to spawn Hyprland event listener thread")?;

    Ok(handle)
}

/// Read and parse events from the stream until disconnection.
fn process_events(stream: &UnixStream, tx: &Sender<WaylandEvent>) {
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        match line {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }

                debug!("Hyprland event: {}", line);

                if let Some(event) = parse_event(&line)
                    && tx.send(event).is_err()
                {
                    // Receiver dropped — daemon shutting down
                    return;
                }
            }
            Err(e) => {
                warn!("Error reading Hyprland socket: {}", e);
                let _ = tx.send(WaylandEvent::Disconnected);
                return;
            }
        }
    }

    // Stream ended
    let _ = tx.send(WaylandEvent::Disconnected);
}

/// Parse a single Hyprland event line.
///
/// Hyprland socket2 events have the format: `EVENT>>DATA`
fn parse_event(line: &str) -> Option<WaylandEvent> {
    let (event_name, data) = line.split_once(">>")?;

    match event_name {
        // "fullscreen" events are intentionally ignored: occlusion is tracked
        // per-monitor from `hyprctl` queries, not from this global signal.
        "workspace" => {
            if let Ok(id) = data.trim().parse::<i64>() {
                Some(WaylandEvent::WorkspaceChanged { id })
            } else {
                debug!("Could not parse workspace id: {}", data);
                None
            }
        }
        "focusedmon" => {
            // Format: "MONITORNAME,WORKSPACEID"
            let name = data.split(',').next().unwrap_or(data).trim().to_string();
            Some(WaylandEvent::MonitorChanged { name })
        }
        _ => {
            // Ignore other events (activewindow, openwindow, etc.)
            None
        }
    }
}

/// Start polling Hyprland cursor position on a background thread.
///
/// Returns shared cursor state and a handle for the polling thread.
pub fn start_cursor_tracker(
    occluded_names: super::OcclusionSnapshot,
) -> Result<(
    std::sync::Arc<std::sync::Mutex<Vec2>>,
    std::thread::JoinHandle<()>,
)> {
    let mut last_poll = None;
    super::start_cursor_tracker("hypr", move || {
        let now = std::time::Instant::now();
        if last_poll.is_none_or(|last| now.duration_since(last) >= Duration::from_millis(500)) {
            last_poll = Some(now);
            let names = query_hyprland_occluded_names().unwrap_or_else(|error| {
                debug!("Hyprland occlusion query failed: {error}");
                Vec::new()
            });
            // Refresh the timestamp even when the contents are unchanged.
            if let Ok(mut snapshot) = occluded_names.lock() {
                *snapshot = (std::time::Instant::now(), names);
            }
        }
        query_hyprland_cursor()
    })
}

/// Run one Hyprland IPC command over its unix socket (no process spawning).
///
/// Connects to `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`,
/// sends the command, and reads the full response.
fn hypr_ipc(command: &str) -> anyhow::Result<String> {
    let instance_sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| anyhow::anyhow!("HYPRLAND_INSTANCE_SIGNATURE not set"))?;
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let socket_path = format!("{}/hypr/{}/.socket.sock", runtime_dir, instance_sig);

    let mut stream = UnixStream::connect(&socket_path)?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_millis(250)))?;
    stream.write_all(command.as_bytes())?;
    // Shutdown write side so server knows command is complete
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut out = Vec::new();
    stream.take(4 * 1024 * 1024).read_to_end(&mut out)?;
    anyhow::ensure!(
        out.len() < 4 * 1024 * 1024,
        "Hyprland response exceeded size limit"
    );
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Query cursor position via the Hyprland IPC socket.
pub fn query_hyprland_cursor() -> anyhow::Result<Vec2> {
    // Response format: "960, 540" or "960, 540\n"
    let response = hypr_ipc("cursorpos")?;
    let parts: Vec<&str> = response.trim().split(',').collect();
    if parts.len() == 2 {
        let x: f32 = parts[0].trim().parse().unwrap_or(0.0);
        let y: f32 = parts[1].trim().parse().unwrap_or(0.0);
        Ok(Vec2::new(x, y))
    } else {
        Err(anyhow::anyhow!("Unexpected cursor response: {:?}", parts))
    }
}

/// Returns the names of monitors whose background is covered by a fullscreen
/// window (or a window that fully spans the monitor), per `hyprctl -j` data.
///
/// Best-effort and defensive across Hyprland versions: missing fields degrade
/// to "not occluded".
pub fn query_hyprland_occluded_names() -> anyhow::Result<Vec<String>> {
    let monitors: Vec<serde_json::Value> = serde_json::from_str(&hypr_ipc("j/monitors")?)
        .map_err(|e| anyhow::anyhow!("Failed to parse hyprctl monitors: {e}"))?;
    let windows: Vec<serde_json::Value> = serde_json::from_str(&hypr_ipc("j/clients")?)
        .map_err(|e| anyhow::anyhow!("Failed to parse hyprctl clients: {e}"))?;

    Ok(compute_occluded_names(&monitors, &windows))
}

/// Computes fullscreen occlusion only for clients on a displayed workspace.
/// Monitor IDs are identifiers, not positions in the monitors array. Unknown
/// fullscreen modes and missing visibility fields fail open.
fn compute_occluded_names(
    monitors: &[serde_json::Value],
    windows: &[serde_json::Value],
) -> Vec<String> {
    monitors
        .iter()
        .filter_map(|monitor| {
            let id = monitor.get("id")?.as_i64()?;
            let name = monitor.get("name")?.as_str()?;
            let workspace = monitor.pointer("/activeWorkspace/id")?.as_i64()?;
            let special = monitor
                .pointer("/specialWorkspace/id")
                .and_then(|v| v.as_i64())
                .filter(|id| *id != 0);
            let covered = windows.iter().any(|window| {
                let client_workspace = window.pointer("/workspace/id").and_then(|v| v.as_i64());
                let visible_workspace = if let Some(special) = special {
                    client_workspace == Some(special)
                } else {
                    client_workspace == Some(workspace)
                };
                // Modern Hyprland: 1 = maximized, 2 = fullscreen, 3 = both.
                // Older releases exposed a boolean instead.
                let fullscreen = match window.get("fullscreen") {
                    Some(serde_json::Value::Bool(value)) => *value,
                    Some(value) => matches!(value.as_u64(), Some(2 | 3)),
                    None => false,
                };
                window.get("monitor").and_then(|v| v.as_i64()) == Some(id)
                    && window.get("mapped").and_then(|v| v.as_bool()) == Some(true)
                    && window.get("hidden").and_then(|v| v.as_bool()) == Some(false)
                    && visible_workspace
                    && fullscreen
            });
            covered.then(|| name.to_string())
        })
        .collect()
}

/// Snapshots older than this are treated as absent (fail-open), so a dead
/// hyprctl socket cannot keep occluding monitors forever.
pub const OCCLUSION_SNAPSHOT_TTL: Duration = Duration::from_secs(5);

/// True when the occlusion snapshot taken at `last` is still usable at `now`.
pub fn is_snapshot_fresh(last: &std::time::Instant, now: &std::time::Instant) -> bool {
    now.duration_since(*last) <= OCCLUSION_SNAPSHOT_TTL
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fullscreen_requires_visible_workspace_and_real_monitor_id() {
        let monitors = vec![
            json!({"id":7,"name":"DP-1","activeWorkspace":{"id":3},"specialWorkspace":{"id":0}}),
        ];
        let mut client =
            json!({"monitor":7,"workspace":{"id":3},"mapped":true,"hidden":false,"fullscreen":2});
        assert_eq!(
            compute_occluded_names(&monitors, &[client.clone()]),
            vec!["DP-1"]
        );
        for mode in [0, 1, 4, 99] {
            client["fullscreen"] = json!(mode);
            assert!(compute_occluded_names(&monitors, &[client.clone()]).is_empty());
        }
        client["fullscreen"] = json!(true);
        assert_eq!(
            compute_occluded_names(&monitors, &[client.clone()]),
            vec!["DP-1"]
        );
        client["workspace"]["id"] = json!(4);
        assert!(compute_occluded_names(&monitors, &[client.clone()]).is_empty());
        client["workspace"]["id"] = json!(3);
        client["hidden"] = json!(true);
        assert!(compute_occluded_names(&monitors, &[client]).is_empty());
    }

    #[test]
    fn unknown_schema_and_special_overlay_fail_open() {
        assert!(compute_occluded_names(&[json!({})], &[json!({"fullscreen":true})]).is_empty());
        let monitors = vec![
            json!({"id":2,"name":"DP-2","activeWorkspace":{"id":3},"specialWorkspace":{"id":-99}}),
        ];
        let mut client =
            json!({"monitor":2,"workspace":{"id":3},"mapped":true,"hidden":false,"fullscreen":2});
        assert!(compute_occluded_names(&monitors, &[client.clone()]).is_empty());
        client["workspace"]["id"] = json!(-99);
        assert_eq!(compute_occluded_names(&monitors, &[client]), vec!["DP-2"]);
    }

    #[test]
    fn snapshots_expire() {
        let now = std::time::Instant::now();
        assert!(is_snapshot_fresh(&now, &now));
        assert!(!is_snapshot_fresh(
            &now,
            &(now + OCCLUSION_SNAPSHOT_TTL + Duration::from_secs(1))
        ));
    }
}
