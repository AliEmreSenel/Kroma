//! IPC client — sends commands to the Kroma daemon.

use std::io::Write;
use std::os::unix::net::UnixStream;

use anyhow::{Context, Result};

use kroma_shared::ipc::{socket_path, DaemonCommand, DaemonEvent};

/// Send a fire-and-forget command to the daemon.
/// Reads one ack line and ignores it.
fn send_command(cmd: &DaemonCommand) -> Result<()> {
    use std::io::{BufRead, BufReader};

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

    // Read the daemon's acknowledgment (or error) so it doesn't get broken pipe
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
    let mut reader = BufReader::new(stream);
    let mut _ack = String::new();
    let _ = reader.read_line(&mut _ack); // best-effort
    Ok(())
}

/// Send a command to the daemon and wait for a typed response.
///
/// Commands like LiveReload, LoadShade, and Reload return a `CompileResult`
/// event instead of a simple `Ready` ack.
fn send_command_with_response(cmd: &DaemonCommand) -> Result<DaemonEvent> {
    use std::io::{BufRead, BufReader};

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

    // Wait for the compile result (daemon blocks until compilation completes)
    stream.set_read_timeout(Some(std::time::Duration::from_secs(15)))?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .context("Failed to read compile result from daemon")?;

    let event: DaemonEvent = serde_json::from_str(response.trim())
        .with_context(|| format!("Invalid daemon response: {}", response.trim()))?;
    Ok(event)
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

/// Query the daemon's current status and return the raw JSON response.
pub fn query_status() -> Result<String> {
    use std::io::{BufRead, BufReader};

    let path = socket_path();
    let stream = UnixStream::connect(&path).with_context(|| {
        format!(
            "Could not connect to daemon at {}. Is kroma-daemon running?",
            path.display()
        )
    })?;

    // Send a proper StatusQuery command
    let cmd = DaemonCommand::StatusQuery;
    let request = serde_json::to_string(&cmd)?;
    let mut writer = stream.try_clone()?;
    writer.write_all(request.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;

    // Read ONE newline-delimited JSON response line (daemon doesn't close the socket)
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    Ok(response.trim().to_string())
}

/// Tell the daemon to load a shade package and return compile results.
#[allow(dead_code)]
pub fn send_load_with_result(path: &str) -> Result<DaemonEvent> {
    send_command_with_response(&DaemonCommand::LoadShade {
        path: path.to_string(),
    })
}
