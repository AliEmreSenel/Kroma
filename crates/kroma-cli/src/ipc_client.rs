//! IPC client — sends commands to the Kroma daemon over Unix socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use anyhow::{Context, Result};

use kroma_shared::ipc::{DaemonCommand, socket_path};

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
    let mut _ack = String::new();
    let _ = reader.read_line(&mut _ack);
    Ok(())
}

/// Tell the daemon to load a shade package.
pub fn send_load(path: &str) -> Result<()> {
    send_command(&DaemonCommand::LoadShade {
        path: path.to_string(),
    })
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
