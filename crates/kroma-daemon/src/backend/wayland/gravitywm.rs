//! GravityWM integration over its generic control socket.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use glam::Vec2;
use serde::Deserialize;

use super::WaylandEvent;

#[derive(Debug, Deserialize, PartialEq)]
struct GravityState {
    cursor: [f32; 2],
    fullscreen: bool,
}

impl GravityState {
    fn cursor_pos(&self) -> Vec2 {
        Vec2::new(self.cursor[0], self.cursor[1])
    }
}

pub fn start_state_tracker(tx: Sender<WaylandEvent>) -> Result<(Arc<Mutex<Vec2>>, JoinHandle<()>)> {
    let socket = control_socket_path()?;
    let mut last_fullscreen = None;

    super::start_cursor_tracker("gravitywm", move || {
        let state = query_state(&socket)?;
        if last_fullscreen.replace(state.fullscreen) != Some(state.fullscreen) {
            let _ = tx.send(WaylandEvent::Fullscreen {
                fullscreen: state.fullscreen,
            });
        }
        Ok(state.cursor_pos())
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
        assert!(state.fullscreen);
    }

    #[test]
    fn reports_control_socket_errors() {
        let err = parse_control_json::<GravityState>("error\nunknown command\n").unwrap_err();
        assert!(err.to_string().contains("unknown command"));
    }
}
