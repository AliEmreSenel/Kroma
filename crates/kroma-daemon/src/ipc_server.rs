//! IPC server for the daemon.
//!
//! Listens on a Unix domain socket for JSON commands from the GUI.
//! Supports bidirectional communication — sends status events back.
//! Uses a response channel for commands that produce results (e.g., LiveReload).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};
use log::{error, info};

use kroma_shared::ipc::{DaemonCommand, DaemonEvent, DaemonPhase, DaemonWaitReason, socket_path};

use crate::render_demand::RenderDemand;

static NEXT_CONSUMER: AtomicU64 = AtomicU64::new(1);

struct ConsumerLease {
    id: u64,
    status: Arc<Mutex<DaemonStatus>>,
}

impl Drop for ConsumerLease {
    fn drop(&mut self) {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .render_demand
            .remove(self.id);
    }
}

/// Return type for [`start`]: join handle and shared daemon status.
type IpcStartResult = (JoinHandle<()>, Arc<Mutex<DaemonStatus>>);

/// Internal command wrapper that includes an optional response channel.
///
/// For commands that produce results (LiveReload, LoadShade, Reload),
/// the IPC server creates a oneshot-style channel and waits for the
/// main loop to send a `DaemonEvent` back.
pub struct InternalCommand {
    pub command: DaemonCommand,
    /// If set, the main loop should send a response (e.g., `CompileResult`).
    pub response_tx: Option<std::sync::mpsc::Sender<DaemonEvent>>,
}

/// Shared daemon status for IPC queries.
#[derive(Clone)]
pub struct DaemonStatus {
    pub render_demand: RenderDemand,
    pub fps: f32,
    pub paused: bool,
    pub loaded_shade: Option<String>,
    pub current_phase: DaemonPhase,
    pub pending_request_path: Option<String>,
    pub wait_reason: DaemonWaitReason,
    pub cpu_usage: f32,
    pub ram_usage: f32,
    pub battery: Option<f32>,
    pub audio_level: f32,
    pub cursor_x: f32,
    pub cursor_y: f32,
}

impl Default for DaemonStatus {
    fn default() -> Self {
        Self {
            render_demand: RenderDemand::default(),
            fps: 0.0,
            paused: false,
            loaded_shade: None,
            current_phase: DaemonPhase::None,
            pending_request_path: None,
            wait_reason: DaemonWaitReason::Idle,
            cpu_usage: 0.0,
            ram_usage: 0.0,
            battery: None,
            audio_level: 0.0,
            cursor_x: 0.0,
            cursor_y: 0.0,
        }
    }
}

/// Start the IPC listener on a background thread.
///
/// Returns a join handle and shared status.
pub fn start(cmd_tx: Sender<InternalCommand>) -> Result<IpcStartResult> {
    let path = socket_path();

    // Remove stale socket if it exists
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("Failed to remove stale socket: {}", path.display()))?;
    }

    let listener = UnixListener::bind(&path)
        .with_context(|| format!("Failed to bind IPC socket: {}", path.display()))?;

    info!("IPC socket: {}", path.display());

    let status = Arc::new(Mutex::new(DaemonStatus::default()));
    let status_clone = Arc::clone(&status);

    let handle = std::thread::Builder::new()
        .name("kroma-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let tx = cmd_tx.clone();
                        let status = Arc::clone(&status_clone);
                        std::thread::spawn(move || {
                            let consumer = ConsumerLease {
                                id: NEXT_CONSUMER.fetch_add(1, Ordering::Relaxed),
                                status: Arc::clone(&status),
                            };
                            let writer = match stream.try_clone() {
                                Ok(cloned) => cloned,
                                Err(e) => {
                                    log::warn!("Failed to clone IPC stream: {}", e);
                                    return;
                                }
                            };
                            let reader = BufReader::new(stream);
                            let mut writer = writer;

                            for line in reader.lines() {
                                match line {
                                    Ok(line) if line.trim().is_empty() => continue,
                                    Ok(line) => {
                                        match serde_json::from_str::<DaemonCommand>(&line) {
                                            Ok(DaemonCommand::SetRenderDemand { monitors }) => {
                                                let event = if monitors.len() > 64
                                                    || monitors.iter().any(|name| name.is_empty() || name.len() > 256)
                                                {
                                                    DaemonEvent::Error { message: "Invalid render demand outputs".into() }
                                                } else {
                                                    status.lock().unwrap_or_else(|e| e.into_inner())
                                                        .render_demand.set(consumer.id, monitors);
                                                    DaemonEvent::Ready
                                                };
                                                if let Ok(json) = serde_json::to_string(&event) {
                                                    let _ = writeln!(writer, "{}", json);
                                                    let _ = writer.flush();
                                                }
                                            }
                                            Ok(DaemonCommand::StatusQuery) => {
                                                let s = status.lock().unwrap_or_else(|e| e.into_inner());
                                                let event = DaemonEvent::Status {
                                                    fps: s.fps,
                                                    paused: s.paused,
                                                    loaded_shade: s.loaded_shade.clone(),
                                                    current_phase: s.current_phase,
                                                    pending_request_path: s.pending_request_path.clone(),
                                                    wait_reason: s.wait_reason,
                                                };
                                                if let Ok(json) = serde_json::to_string(&event) {
                                                    let _ = writeln!(writer, "{}", json);
                                                    let _ = writer.flush();
                                                }
                                            }
                                            Ok(DaemonCommand::QuerySystemInfo) => {
                                                let s = status.lock().unwrap_or_else(|e| e.into_inner());
                                                let event = DaemonEvent::SystemInfo {
                                                    cpu_usage: s.cpu_usage,
                                                    ram_usage: s.ram_usage,
                                                    battery: s.battery,
                                                    audio_level: s.audio_level,
                                                    cursor_x: s.cursor_x,
                                                    cursor_y: s.cursor_y,
                                                };
                                                if let Ok(json) = serde_json::to_string(&event) {
                                                    let _ = writeln!(writer, "{}", json);
                                                    let _ = writer.flush();
                                                }
                                            }
                                            Ok(DaemonCommand::StartPreviewStream { width, height, target_fps }) => {
                                                let _ = (width, height, target_fps);
                                                let event = DaemonEvent::Error {
                                                    message: "Preview pipeline is disabled in unified renderer mode".into(),
                                                };
                                                if let Ok(json) = serde_json::to_string(&event) {
                                                    let _ = writeln!(writer, "{}", json);
                                                    let _ = writer.flush();
                                                }
                                            }
                                            Ok(DaemonCommand::StopPreviewStream) => {
                                                let event = DaemonEvent::Error {
                                                    message: "Preview pipeline is disabled in unified renderer mode".into(),
                                                };
                                                if let Ok(json) = serde_json::to_string(&event) {
                                                    let _ = writeln!(writer, "{}", json);
                                                    let _ = writer.flush();
                                                }
                                            }
                                            Ok(DaemonCommand::RequestPreviewFrame { width: _w, height: _h }) => {
                                                let _ = (_w, _h);
                                                let event = DaemonEvent::Error {
                                                    message: "Preview pipeline is disabled in unified renderer mode".into(),
                                                };
                                                if let Ok(json) = serde_json::to_string(&event) {
                                                    let _ = writeln!(writer, "{}", json);
                                                    let _ = writer.flush();
                                                }
                                            }
                                            Ok(cmd) => {
                                                // Determine if this command expects a compile result
                                                let needs_response = matches!(
                                                    cmd,
                                                    DaemonCommand::LiveReload { .. }
                                                        | DaemonCommand::LoadShade { .. }
                                                        | DaemonCommand::Reload
                                                );

                                                if needs_response {
                                                    // Create response channel and wait for result
                                                    let (resp_tx, resp_rx) =
                                                        std::sync::mpsc::channel();
                                                    let icmd = InternalCommand {
                                                        command: cmd,
                                                        response_tx: Some(resp_tx),
                                                    };
                                                    if tx.send(icmd).is_err() {
                                                        return;
                                                    }
                                                    // Wait for compile result with timeout
                                                    let event = match resp_rx
                                                        .recv_timeout(Duration::from_secs(10))
                                                    {
                                                        Ok(ev) => ev,
                                                        Err(_) => DaemonEvent::Error {
                                                            message:
                                                                "Compile response timed out"
                                                                    .into(),
                                                        },
                                                    };
                                                    if let Ok(json) =
                                                        serde_json::to_string(&event)
                                                    {
                                                        let _ = writeln!(writer, "{}", json);
                                                        let _ = writer.flush();
                                                    }
                                                } else {
                                                    // Fire-and-forget with Ready ack
                                                    let ack = DaemonEvent::Ready;
                                                    if let Ok(json) =
                                                        serde_json::to_string(&ack)
                                                    {
                                                        let _ = writeln!(writer, "{}", json);
                                                        let _ = writer.flush();
                                                    }
                                                    let icmd = InternalCommand {
                                                        command: cmd,
                                                        response_tx: None,
                                                    };
                                                    if tx.send(icmd).is_err() {
                                                        return;
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                let err_event = DaemonEvent::Error {
                                                    message: format!("Invalid command: {}", e),
                                                };
                                                if let Ok(json) = serde_json::to_string(&err_event) {
                                                    let _ = writeln!(writer, "{}", json);
                                                    let _ = writer.flush();
                                                }
                                                error!("Invalid IPC message: {} — {}", line, e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!("IPC read error: {}", e);
                                        return;
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => {
                        error!("IPC accept error: {}", e);
                    }
                }
            }
        })
        .context("Failed to spawn IPC thread")?;

    Ok((handle, status))
}

#[cfg(test)]
mod demand_lifetime_tests {
    use super::*;

    #[test]
    fn connection_guard_releases_only_its_own_outputs() {
        let status = Arc::new(Mutex::new(DaemonStatus::default()));
        {
            let _consumer = ConsumerLease {
                id: 10,
                status: Arc::clone(&status),
            };
            let mut state = status.lock().unwrap();
            state.render_demand.set(10, vec!["eDP-1".into()]);
            state.render_demand.set(11, vec!["DP-1".into()]);
        }
        let remaining = status.lock().unwrap().render_demand.monitors();
        assert_eq!(remaining, std::collections::HashSet::from(["DP-1".into()]));
    }
}
