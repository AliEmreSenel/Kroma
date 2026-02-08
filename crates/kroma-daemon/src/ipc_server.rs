//! IPC server for the daemon.
//!
//! Listens on a Unix domain socket for JSON commands from the GUI.
//! Supports bidirectional communication — sends status events back.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use log::{error, info};

use kroma_shared::ipc::{socket_path, DaemonCommand, DaemonEvent};

/// Shared daemon status for IPC queries.
#[derive(Clone)]
pub struct DaemonStatus {
    pub fps: f32,
    pub paused: bool,
    pub loaded_shade: Option<String>,
}

impl Default for DaemonStatus {
    fn default() -> Self {
        Self {
            fps: 0.0,
            paused: false,
            loaded_shade: None,
        }
    }
}

/// Start the IPC listener on a background thread.
///
/// Returns a join handle and a shared status object that the main loop
/// should update periodically.
pub fn start(cmd_tx: Sender<DaemonCommand>) -> Result<(JoinHandle<()>, Arc<Mutex<DaemonStatus>>)> {
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
                                        // Check if this is a status query (special handling)
                                        if line.trim() == "\"StatusQuery\"" || line.trim() == "{\"type\":\"StatusQuery\"}" {
                                            let s = status.lock().unwrap();
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
                                            Ok(cmd) => {
                                                // Send acknowledgment
                                                let ack = DaemonEvent::Ready;
                                                if let Ok(json) = serde_json::to_string(&ack) {
                                                    let _ = writeln!(writer, "{}", json);
                                                    let _ = writer.flush();
                                                }

                                                if tx.send(cmd).is_err() {
                                                    return;
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
