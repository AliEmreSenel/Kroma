//! IPC client — sends commands to the Kroma daemon over Unix socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use anyhow::{Context, Result};

use kroma_shared::ipc::{DaemonCommand, DaemonEvent, socket_path};

/// Send a fire-and-forget command to the daemon.
fn send_command(cmd: &DaemonCommand) -> Result<()> {
    let path = socket_path();
    let stream = UnixStream::connect(&path).with_context(|| {
        format!(
            "Could not connect to daemon at {}. Is kroma-daemon running?",
            path.display()
        )
    })?;

    let mut writer = stream.try_clone()?;
    let json = serde_json::to_string(cmd)?;
    writer.write_all(json.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;

    // Read the daemon's acknowledgment so it doesn't get broken pipe
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
    let mut reader = BufReader::new(stream);
    let mut ack = String::new();
    let _ = reader.read_line(&mut ack);

    if ack.trim().is_empty() {
        return Ok(());
    }

    parse_ack_event(ack.trim())
}

fn parse_ack_event(ack: &str) -> Result<()> {
    if ack.is_empty() {
        return Ok(());
    }

    if let Ok(event) = serde_json::from_str::<DaemonEvent>(ack) {
        return match event {
            DaemonEvent::Ready | DaemonEvent::ShadeLoaded { .. } => Ok(()),
            DaemonEvent::Error { message } => anyhow::bail!(message),
            DaemonEvent::LoadRejected { code, message } => {
                anyhow::bail!("load rejected ({:?}): {}", code, message)
            }
            DaemonEvent::CompileResult {
                success, errors, ..
            } => {
                if success {
                    Ok(())
                } else {
                    let detail = errors
                        .iter()
                        .map(|e| e.message.as_str())
                        .collect::<Vec<_>>()
                        .join(" | ");
                    anyhow::bail!("compile/load failed: {}", detail)
                }
            }
            _ => Ok(()),
        };
    }

    Ok(())
}

/// Tell the daemon to load a shade package.
pub fn send_load(path: &str, force: bool) -> Result<()> {
    send_command(&DaemonCommand::LoadShade {
        path: path.to_string(),
        force,
    })
}

/// Tell the daemon to unload the current shade.
pub fn send_unload() -> Result<()> {
    send_command(&DaemonCommand::UnloadShade)
}

/// Tell the daemon to pause rendering.
pub fn send_pause() -> Result<()> {
    send_command(&DaemonCommand::Pause)
}

/// Tell the daemon to resume rendering.
pub fn send_resume() -> Result<()> {
    send_command(&DaemonCommand::Resume)
}

/// Tell the daemon to shut down gracefully.
pub fn send_shutdown() -> Result<()> {
    send_command(&DaemonCommand::Shutdown)
}

/// Tell the daemon to reload the current shade package.
pub fn send_reload() -> Result<()> {
    send_command(&DaemonCommand::Reload)
}

/// Query the daemon's current status and return the raw JSON response.
pub fn query_status() -> Result<String> {
    let path = socket_path();
    let stream = UnixStream::connect(&path).with_context(|| {
        format!(
            "Could not connect to daemon at {}. Is kroma-daemon running?",
            path.display()
        )
    })?;

    let cmd = DaemonCommand::StatusQuery;
    let request = serde_json::to_string(&cmd)?;
    let mut writer = stream.try_clone()?;
    writer.write_all(request.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;

    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    Ok(response.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_ack_event;

    #[test]
    fn parse_ack_event_accepts_ready() {
        let ack = r#"{"type":"Ready"}"#;
        assert!(parse_ack_event(ack).is_ok());
    }

    #[test]
    fn parse_ack_event_rejects_load_rejected() {
        let ack = r#"{"type":"LoadRejected","code":"busy_running_unload","message":"unload phase is running"}"#;
        let err = parse_ack_event(ack).expect_err("must reject");
        assert!(err.to_string().contains("BusyRunningUnload"));
    }

    #[test]
    fn parse_ack_event_rejects_compile_failure() {
        let ack = r#"{"type":"CompileResult","success":false,"errors":[{"message":"compile error","line":null,"column":null}],"warnings":[]}"#;
        let err = parse_ack_event(ack).expect_err("must reject");
        assert!(err.to_string().contains("compile/load failed"));
    }
}
