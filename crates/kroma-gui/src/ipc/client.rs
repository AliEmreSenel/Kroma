//! Async IPC client with persistent connection and auto-reconnect.

use iced::futures::SinkExt;
use iced::Subscription;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use kroma_shared::ipc::{socket_path, DaemonCommand, DaemonEvent};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Events emitted by the IPC subscription to the GUI.
#[derive(Debug, Clone)]
pub enum IpcEvent {
    /// Connection to the daemon was established.
    Connected,
    /// Connection lost (will auto-reconnect).
    Disconnected(String),
    /// Received a `DaemonEvent` from the daemon.
    Event(DaemonEvent),
    /// Reconnection attempt in progress.
    Reconnecting { attempt: u32 },
}

/// Handle to send commands from the GUI to the IPC background task.
#[derive(Debug, Clone)]
pub struct IpcHandle {
    tx: mpsc::UnboundedSender<DaemonCommand>,
}

impl IpcHandle {
    /// Send a command to the daemon (non-blocking, fire-and-forget).
    pub fn send(&self, cmd: DaemonCommand) -> bool {
        self.tx.send(cmd).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Subscription
// ---------------------------------------------------------------------------

/// Creates an iced `Subscription` that runs a background IPC connection.
///
/// Returns `IpcEvent` messages and an `IpcHandle` for sending commands.
/// The `IpcHandle` is returned via the `IpcEvent::Connected` callback — the
/// caller should store the handle on first connection.
pub fn ipc_subscription() -> Subscription<(IpcEvent, Option<IpcHandle>)> {
    Subscription::run(ipc_worker)
}

/// The background worker that maintains a persistent daemon connection.
fn ipc_worker() -> impl iced::futures::Stream<Item = (IpcEvent, Option<IpcHandle>)> {
    iced::stream::channel(64, |mut output| async move {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<DaemonCommand>();

        // Build the handle upfront so we can share it with the GUI,
        // but only send it once the first connection succeeds.
        let handle = IpcHandle { tx: cmd_tx };

        let mut reconnect_attempt: u32 = 0;
        let max_backoff_secs: u64 = 8;

        loop {
            // Try to connect
            let path = socket_path();
            match UnixStream::connect(&path).await {
                Ok(stream) => {
                    reconnect_attempt = 0;
                    let _ = output.send((IpcEvent::Connected, Some(handle.clone()))).await;

                    // Run the read/write loop on this connection
                    match run_connection(stream, &mut cmd_rx, &mut output).await {
                        Ok(()) => {
                            // Connection closed gracefully
                            let _ = output
                                .send((
                                    IpcEvent::Disconnected("Connection closed".into()),
                                    None,
                                ))
                                .await;
                        }
                        Err(e) => {
                            let _ = output
                                .send((
                                    IpcEvent::Disconnected(format!("{}", e)),
                                    None,
                                ))
                                .await;
                        }
                    }
                }
                Err(e) => {
                    let _ = output
                        .send((
                            IpcEvent::Disconnected(format!(
                                "Cannot connect to {}: {}",
                                path.display(),
                                e
                            )),
                            None,
                        ))
                        .await;
                }
            }

            // Exponential backoff before reconnect
            reconnect_attempt += 1;
            let backoff = (1u64 << reconnect_attempt.min(3)).min(max_backoff_secs);
            let _ = output
                .send((
                    IpcEvent::Reconnecting {
                        attempt: reconnect_attempt,
                    },
                    None,
                ))
                .await;
            tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        }
    })
}

/// Run the read/write loop on an established connection.
///
/// Returns `Ok(())` when the connection is cleanly closed, or `Err` on I/O error.
async fn run_connection(
    stream: UnixStream,
    cmd_rx: &mut mpsc::UnboundedReceiver<DaemonCommand>,
    output: &mut iced::futures::channel::mpsc::Sender<(IpcEvent, Option<IpcHandle>)>,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    loop {
        tokio::select! {
            // Read events from daemon
            line = lines.next_line() => {
                match line {
                    Ok(Some(line)) if !line.trim().is_empty() => {
                        match serde_json::from_str::<DaemonEvent>(line.trim()) {
                            Ok(event) => {
                                let _ = output.send((IpcEvent::Event(event), None)).await;
                            }
                            Err(e) => {
                                log::warn!("Failed to parse daemon event: {}: {:?}", line.trim(), e);
                            }
                        }
                    }
                    Ok(Some(_)) => {
                        // Empty line, ignore
                    }
                    Ok(None) => {
                        // EOF — connection closed
                        return Ok(());
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!("Read error: {}", e));
                    }
                }
            }

            // Send commands to daemon
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(cmd) => {
                        let json = serde_json::to_string(&cmd)?;
                        writer.write_all(json.as_bytes()).await?;
                        writer.write_all(b"\n").await?;
                        writer.flush().await?;
                    }
                    None => {
                        // Channel closed — GUI is shutting down
                        return Ok(());
                    }
                }
            }
        }
    }
}
