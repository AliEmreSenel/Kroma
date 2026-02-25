//! Hyprland IPC event listener.
//!
//! Monitors Hyprland's event socket (`socket2`) for compositor events
//! that affect rendering behaviour — fullscreen windows, workspace changes,
//! active monitor changes, etc.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::{Context, Result};
use glam::Vec2;
use log::{debug, error, info, warn};

/// Events produced by the Hyprland IPC listener.
#[derive(Debug, Clone)]
pub enum HyprlandEvent {
    /// A window entered or exited fullscreen on the given workspace.
    Fullscreen { fullscreen: bool },
    /// The active workspace changed.
    WorkspaceChanged { id: i64 },
    /// The active monitor changed.
    MonitorChanged { name: String },
    /// Hyprland is shutting down / socket disconnected.
    Disconnected,
}

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
pub fn start_listener(tx: Sender<HyprlandEvent>) -> Result<std::thread::JoinHandle<()>> {
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
                            let _ = tx.send(HyprlandEvent::Disconnected);
                            return;
                        }
                    }
                }

                let backoff = Duration::from_secs(retry_count.min(30));
                std::thread::sleep(backoff);
            }
        })
        .context("Failed to spawn Hyprland event listener thread")?;

    Ok(handle)
}

/// Read and parse events from the stream until disconnection.
fn process_events(stream: &UnixStream, tx: &Sender<HyprlandEvent>) {
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
                let _ = tx.send(HyprlandEvent::Disconnected);
                return;
            }
        }
    }

    // Stream ended
    let _ = tx.send(HyprlandEvent::Disconnected);
}

/// Parse a single Hyprland event line.
///
/// Hyprland socket2 events have the format: `EVENT>>DATA`
fn parse_event(line: &str) -> Option<HyprlandEvent> {
    let (event_name, data) = line.split_once(">>")?;

    match event_name {
        "fullscreen" => {
            let fullscreen = data.trim() == "1";
            Some(HyprlandEvent::Fullscreen { fullscreen })
        }
        "workspace" => {
            if let Ok(id) = data.trim().parse::<i64>() {
                Some(HyprlandEvent::WorkspaceChanged { id })
            } else {
                debug!("Could not parse workspace id: {}", data);
                None
            }
        }
        "focusedmon" => {
            // Format: "MONITORNAME,WORKSPACEID"
            let name = data.split(',').next().unwrap_or(data).trim().to_string();
            Some(HyprlandEvent::MonitorChanged { name })
        }
        _ => {
            // Ignore other events (activewindow, openwindow, etc.)
            None
        }
    }
}

/// Query cursor position via Hyprland IPC socket directly (no process spawning).
///
/// Connects to `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`,
/// sends `cursorpos`, reads the response.
pub fn query_hyprland_cursor() -> anyhow::Result<Vec2> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let instance_sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| anyhow::anyhow!("HYPRLAND_INSTANCE_SIGNATURE not set"))?;

    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fullscreen_event() {
        let event = parse_event("fullscreen>>1").unwrap();
        match event {
            HyprlandEvent::Fullscreen { fullscreen } => assert!(fullscreen),
            _ => panic!("Expected Fullscreen event"),
        }

        let event = parse_event("fullscreen>>0").unwrap();
        match event {
            HyprlandEvent::Fullscreen { fullscreen } => assert!(!fullscreen),
            _ => panic!("Expected Fullscreen event"),
        }
    }

    #[test]
    fn parse_workspace_event() {
        let event = parse_event("workspace>>3").unwrap();
        match event {
            HyprlandEvent::WorkspaceChanged { id } => assert_eq!(id, 3),
            _ => panic!("Expected WorkspaceChanged event"),
        }
    }

    #[test]
    fn parse_focusedmon_event() {
        let event = parse_event("focusedmon>>DP-1,2").unwrap();
        match event {
            HyprlandEvent::MonitorChanged { name } => assert_eq!(name, "DP-1"),
            _ => panic!("Expected MonitorChanged event"),
        }
    }

    #[test]
    fn parse_unknown_event() {
        assert!(parse_event("activewindow>>kitty,~").is_none());
    }
}
