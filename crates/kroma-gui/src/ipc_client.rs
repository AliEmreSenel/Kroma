//! IPC client — sends commands to the Kroma daemon.

use std::io::Write;
use std::os::unix::net::UnixStream;

use anyhow::{Context, Result};

use kroma_shared::ipc::{socket_path, DaemonCommand};

/// Send a single command to the daemon over the IPC socket.
fn send_command(cmd: &DaemonCommand) -> Result<()> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!(
            "Could not connect to daemon at {}. Is kroma-daemon running?",
            path.display()
        ))?;

    let json = serde_json::to_string(cmd)?;
    stream.write_all(json.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
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

/// Query the daemon's current status and return the raw JSON response.
pub fn query_status() -> Result<String> {
    use std::io::Read;

    let path = socket_path();
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!(
            "Could not connect to daemon at {}. Is kroma-daemon running?",
            path.display()
        ))?;

    // Send a JSON status request
    let request = r#""StatusQuery""#;
    stream.write_all(request.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    // Read the response
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}
