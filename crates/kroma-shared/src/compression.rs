//! Shared compression policy parsing utilities.
//!
//! These helpers centralize CLI parsing semantics for package-level and
//! entry-level compression policy arguments.

use anyhow::{Context, Result};

use crate::shade::CompressionPolicy;

fn parse_policy(codec: &str, level: Option<&str>) -> Result<CompressionPolicy> {
    let codec = codec.trim().to_ascii_lowercase();
    let level = level.map(|raw| raw.trim().to_ascii_lowercase());

    match codec.as_str() {
        "auto" => {
            if level.as_deref().is_some_and(|l| l != "auto") {
                anyhow::bail!("auto codec cannot use an explicit level");
            }
            Ok(CompressionPolicy::Auto)
        }
        "none" => {
            if level.as_deref().is_some_and(|l| l != "auto") {
                anyhow::bail!("none codec must use level 'auto'");
            }
            Ok(CompressionPolicy::None)
        }
        "lz4" => {
            if level.as_deref().is_some_and(|l| l != "auto") {
                anyhow::bail!("lz4 codec must use level 'auto'");
            }
            Ok(CompressionPolicy::Lz4)
        }
        "zstd" => {
            let level = match level.as_deref() {
                None | Some("auto") => 3,
                Some(v) => v
                    .parse::<i32>()
                    .with_context(|| format!("Invalid zstd level '{}'", v))?,
            };
            Ok(CompressionPolicy::Zstd { level })
        }
        _ => anyhow::bail!("Unsupported codec '{}'. Use auto|none|zstd|lz4", codec),
    }
}

/// Parse default package compression policy from CLI arguments.
///
/// Returns `Ok(None)` when policy is `auto`, meaning package defaults should
/// remain path/type-derived.
pub fn parse_default_compression_policy(codec: &str, level: &str) -> Result<Option<CompressionPolicy>> {
    let policy = parse_policy(codec, Some(level))?;
    if matches!(policy, CompressionPolicy::Auto) {
        Ok(None)
    } else {
        Ok(Some(policy))
    }
}

/// Parse entry override format `PATH=CODEC[:LEVEL]`.
pub fn parse_entry_compression_override(spec: &str) -> Result<(String, CompressionPolicy)> {
    let (raw_path, raw_policy) = spec
        .split_once('=')
        .with_context(|| "Entry compression must be PATH=CODEC[:LEVEL]")?;

    let path = raw_path.trim().replace('\\', "/");
    if path.is_empty() {
        anyhow::bail!("Entry compression override path cannot be empty");
    }

    let (codec, level) = if let Some((codec, lvl)) = raw_policy.split_once(':') {
        (codec, Some(lvl))
    } else {
        (raw_policy, None)
    };

    let policy = parse_policy(codec, level)
        .with_context(|| format!("Invalid entry compression override '{}'", spec))?;

    Ok((path, policy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_auto_returns_none() {
        assert!(parse_default_compression_policy("auto", "auto")
            .expect("auto policy")
            .is_none());
    }

    #[test]
    fn default_zstd_level_parses() {
        let policy = parse_default_compression_policy("zstd", "9")
            .expect("zstd policy")
            .expect("some policy");
        assert!(matches!(policy, CompressionPolicy::Zstd { level: 9 }));
    }

    #[test]
    fn entry_override_parses_auto() {
        let (_, policy) = parse_entry_compression_override("shader.frag=auto").expect("auto");
        assert!(matches!(policy, CompressionPolicy::Auto));
    }

    #[test]
    fn entry_override_rejects_bad_lz4_level() {
        assert!(parse_entry_compression_override("assets/a.bin=lz4:9").is_err());
    }
}
