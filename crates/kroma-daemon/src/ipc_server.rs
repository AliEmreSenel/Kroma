//! IPC server for the daemon.
//!
//! Listens on a Unix domain socket for JSON commands from the GUI.
//! Supports bidirectional communication — sends status events back.
//! Uses a response channel for commands that produce results (e.g., LiveReload).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};
use log::{error, info};

use kroma_shared::ipc::{socket_path, DaemonCommand, DaemonEvent};

/// Return type for [`start`]: join handle, shared status, and preview stream state.
type IpcStartResult = (
    JoinHandle<()>,
    Arc<Mutex<DaemonStatus>>,
    Arc<Mutex<PreviewStreamState>>,
);

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
    pub fps: f32,
    pub paused: bool,
    pub loaded_shade: Option<String>,
    pub cpu_usage: f32,
    pub ram_usage: f32,
    pub battery: Option<f32>,
    pub audio_level: f32,
    pub cursor_x: f32,
    pub cursor_y: f32,
}

/// Shared state for preview frame streaming.
pub struct PreviewStreamState {
    /// Whether streaming is active.
    pub active: bool,
    /// Target frames per second for the preview stream.
    pub target_fps: u32,
    /// Requested preview width.
    pub width: u32,
    /// Requested preview height.
    pub height: u32,
    /// Writer to send frames back to the connected client.
    /// Only set when a streaming client is connected.
    pub writer: Option<Box<dyn std::io::Write + Send>>,
    /// Last frame send time for throttling.
    pub last_frame_time: std::time::Instant,
}

impl Default for DaemonStatus {
    fn default() -> Self {
        Self {
            fps: 0.0,
            paused: false,
            loaded_shade: None,
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
/// Returns a join handle, shared status, and shared preview stream state.
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

    let preview_state = Arc::new(Mutex::new(PreviewStreamState {
        active: false,
        target_fps: 15,
        width: 480,
        height: 270,
        writer: None,
        last_frame_time: std::time::Instant::now(),
    }));
    let preview_state_clone = Arc::clone(&preview_state);

    let handle = std::thread::Builder::new()
        .name("kroma-ipc".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let tx = cmd_tx.clone();
                        let status = Arc::clone(&status_clone);
                        let preview = Arc::clone(&preview_state_clone);
                        std::thread::spawn(move || {
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
                                        // Check for legacy bare-string status query
                                        if line.trim() == "\"StatusQuery\"" {
                                            let s = status.lock().unwrap_or_else(|e| e.into_inner());
                                            let event = DaemonEvent::Status {
                                                fps: s.fps,
                                                paused: s.paused,
                                                loaded_shade: s.loaded_shade.clone(),
                                            };
                                            if let Ok(json) = serde_json::to_string(&event) {
                                                let _ = writeln!(writer, "{}", json);
                                                let _ = writer.flush();
                                            }
                                            continue;
                                        }

                                        match serde_json::from_str::<DaemonCommand>(&line) {
                                            Ok(DaemonCommand::StatusQuery) => {
                                                let s = status.lock().unwrap_or_else(|e| e.into_inner());
                                                let event = DaemonEvent::Status {
                                                    fps: s.fps,
                                                    paused: s.paused,
                                                    loaded_shade: s.loaded_shade.clone(),
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
                                                log::info!("Preview stream started: {}x{} @ {} fps", width, height, target_fps);
                                                let stream_writer = match writer.try_clone() {
                                                    Ok(w) => w,
                                                    Err(e) => {
                                                        log::warn!("Failed to clone writer for preview: {}", e);
                                                        continue;
                                                    }
                                                };
                                                {
                                                    let mut ps = preview.lock().unwrap_or_else(|e| e.into_inner());
                                                    if ps.writer.is_some() {
                                                        log::warn!("Preview stream: replacing existing client connection");
                                                    }
                                                    ps.active = true;
                                                    ps.width = width;
                                                    ps.height = height;
                                                    ps.target_fps = target_fps;
                                                    ps.writer = Some(Box::new(stream_writer));
                                                    ps.last_frame_time = std::time::Instant::now();
                                                }
                                                let ack = DaemonEvent::Ready;
                                                if let Ok(json) = serde_json::to_string(&ack) {
                                                    let _ = writeln!(writer, "{}", json);
                                                    let _ = writer.flush();
                                                }
                                            }
                                            Ok(DaemonCommand::StopPreviewStream) => {
                                                log::info!("Preview stream stopped");
                                                {
                                                    let mut ps = preview.lock().unwrap_or_else(|e| e.into_inner());
                                                    ps.active = false;
                                                    ps.writer = None;
                                                }
                                                let ack = DaemonEvent::Ready;
                                                if let Ok(json) = serde_json::to_string(&ack) {
                                                    let _ = writeln!(writer, "{}", json);
                                                    let _ = writer.flush();
                                                }
                                            }
                                            Ok(DaemonCommand::RequestPreviewFrame { width: _w, height: _h }) => {
                                                // One-shot frame request — the main loop will
                                                // capture and send a single frame via response_tx.
                                                let (resp_tx, resp_rx) = std::sync::mpsc::channel();
                                                let icmd = InternalCommand {
                                                    command: DaemonCommand::RequestPreviewFrame { width: _w, height: _h },
                                                    response_tx: Some(resp_tx),
                                                };
                                                if tx.send(icmd).is_err() {
                                                    return;
                                                }
                                                let event = match resp_rx.recv_timeout(Duration::from_secs(5)) {
                                                    Ok(ev) => ev,
                                                    Err(_) => DaemonEvent::Error {
                                                        message: "Preview frame timed out".into(),
                                                    },
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
                                        // Clean up preview state on disconnect
                                        {
                                            let mut ps = preview.lock().unwrap_or_else(|e| e.into_inner());
                                            ps.active = false;
                                            ps.writer = None;
                                        }
                                        return;
                                    }
                                }
                            }

                            // Client disconnected — clean up preview state
                            {
                                let mut ps = preview.lock().unwrap_or_else(|e| e.into_inner());
                                ps.active = false;
                                ps.writer = None;
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

    Ok((handle, status, preview_state))
}
