//! Audio source enumeration utilities.
//!
//! Lists available PulseAudio/PipeWire audio sources by running
//! `pactl list sources short` and parsing the output.

use log::warn;

/// List available audio sources from PulseAudio/PipeWire.
///
/// Runs `pactl list sources short` and parses out source names.
/// Each output line has the format: `index\tname\tmodule\tsample_spec\tstate`
/// Returns a vec of source names, prepended with a "default" entry.
/// On failure (e.g., pactl not installed), returns just `["default"]`.
pub fn list_audio_sources() -> Vec<String> {
    let mut sources = vec!["default".to_string()];

    match std::process::Command::new("pactl")
        .args(["list", "sources", "short"])
        .output()
    {
        Ok(output) => {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 2 {
                        let name = parts[1].trim().to_string();
                        if !name.is_empty() && !sources.contains(&name) {
                            sources.push(name);
                        }
                    }
                }
            } else {
                warn!(
                    "pactl exited with status {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        Err(e) => {
            warn!("Failed to run pactl: {}. Audio source list unavailable.", e);
        }
    }

    sources
}
