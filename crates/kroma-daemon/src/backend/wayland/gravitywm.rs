//! GravityWM integration over its generic control socket.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use glam::Vec2;
use serde::Deserialize;

/// GravityWM state as reported by the control socket's `state` command.
/// Fullscreen is optional; unknown or failed snapshots never pause rendering.
#[derive(Debug, Deserialize, PartialEq)]
struct GravityState {
    cursor: [f32; 2],
    #[serde(default)]
    fullscreen: Option<bool>,
}

impl GravityState {
    fn cursor_pos(&self) -> Vec2 {
        Vec2::new(self.cursor[0], self.cursor[1])
    }
}

pub fn start_state_tracker(
    fullscreen: Arc<Mutex<(std::time::Instant, bool)>>,
) -> Result<(Arc<Mutex<Vec2>>, JoinHandle<()>)> {
    let socket = control_socket_path()?;

    super::start_cursor_tracker("gravitywm", move || {
        let result = query_state(&socket);
        if let Ok(mut snapshot) = fullscreen.lock() {
            *snapshot = (
                std::time::Instant::now(),
                result
                    .as_ref()
                    .ok()
                    .and_then(|s| s.fullscreen)
                    .unwrap_or(false),
            );
        }
        result.map(|state| state.cursor_pos())
    })
}

fn control_socket_path() -> Result<PathBuf> {
    std::env::var_os("GRAVITYWM_CONTROL_SOCKET")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("GRAVITYWM_CONTROL_SOCKET not set"))
}

fn query_state(socket: &PathBuf) -> Result<GravityState> {
    let response = query_control_socket(socket, "state")?;
    parse_control_json(&response)
}

fn query_control_socket(path: &PathBuf, command: &str) -> Result<String> {
    let mut stream = UnixStream::connect(path)
        .with_context(|| format!("failed to connect to GravityWM socket {}", path.display()))?;
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    stream.write_all(command.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn parse_control_json<T: for<'de> Deserialize<'de>>(response: &str) -> Result<T> {
    let response = response.trim();
    if let Some(error) = response.strip_prefix("error\n") {
        return Err(anyhow!(error.trim().to_string()));
    }
    let payload = response.strip_prefix("ok\n").unwrap_or(response).trim();
    serde_json::from_str(payload).context("failed to parse GravityWM state JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_state_with_json_parser() {
        let state: GravityState =
            parse_control_json("ok\n{\"cursor\":[960,540],\"fullscreen\":true}\n").unwrap();
        assert_eq!(state.cursor_pos(), Vec2::new(960.0, 540.0));
    }

    #[test]
    fn reports_control_socket_errors() {
        let err = parse_control_json::<GravityState>("error\nunknown command\n").unwrap_err();
        assert!(err.to_string().contains("unknown command"));
    }
}
