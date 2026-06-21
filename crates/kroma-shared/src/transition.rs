//! Transition spec parsing helpers shared by CLI, daemon, and config readers.

use anyhow::{Result, anyhow, bail};
use indexmap::IndexMap;

use crate::types::TransitionDef;

/// Transition source scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionScope {
    Kroma,
    Outgoing,
    Incoming,
}

impl TransitionScope {
    fn parse(scope: &str) -> Result<Self> {
        match scope {
            "kroma" => Ok(Self::Kroma),
            "outgoing" => Ok(Self::Outgoing),
            "incoming" => Ok(Self::Incoming),
            _ => bail!(
                "Unsupported transition scope '{}'. Use one of: kroma, outgoing, incoming",
                scope
            ),
        }
    }
}

/// Parsed transition usage in `scope.id[:seconds]` format.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionSpec {
    pub scope: TransitionScope,
    pub id: String,
    pub duration_override: Option<f64>,
}

/// Fully-resolved transition selection with selected definition and duration.
pub struct ResolvedTransition<'a> {
    pub scope: TransitionScope,
    pub id: &'a str,
    pub definition: &'a TransitionDef,
    pub duration: f64,
}

/// Parse a transition usage spec in `scope.id[:seconds]` format.
pub fn parse_transition_spec(spec: &str) -> Result<TransitionSpec> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        bail!("Transition spec cannot be empty");
    }

    let (scope_raw, rest) = trimmed
        .split_once('.')
        .ok_or_else(|| anyhow!("Transition spec '{}' must include scope and id", trimmed))?;
    if rest.is_empty() {
        bail!("Transition spec '{}' is missing transition id", trimmed);
    }

    let scope = TransitionScope::parse(scope_raw)?;

    let (id_raw, duration_override) = match rest.split_once(':') {
        Some((id, dur_raw)) => {
            if dur_raw.is_empty() {
                bail!(
                    "Transition spec '{}' has empty duration override after ':'",
                    trimmed
                );
            }
            let dur: f64 = dur_raw
                .parse()
                .map_err(|_| anyhow!("Transition duration '{}' is not a valid number", dur_raw))?;
            if !dur.is_finite() || dur <= 0.0 {
                bail!(
                    "Transition duration must be finite and greater than zero, got {}",
                    dur
                );
            }
            (id, Some(dur))
        }
        None => (rest, None),
    };

    if id_raw.is_empty() {
        bail!("Transition spec '{}' is missing transition id", trimmed);
    }
    if !id_raw
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        bail!(
            "Transition id '{}' contains invalid characters; allowed: [A-Za-z0-9_-]",
            id_raw
        );
    }

    Ok(TransitionSpec {
        scope,
        id: id_raw.to_string(),
        duration_override,
    })
}

/// Resolve a parsed transition spec against explicit transition sources.
pub fn resolve_transition_spec<'a>(
    parsed: &'a TransitionSpec,
    kroma: &'a IndexMap<String, TransitionDef>,
    outgoing: Option<&'a IndexMap<String, TransitionDef>>,
    incoming: Option<&'a IndexMap<String, TransitionDef>>,
) -> Result<ResolvedTransition<'a>> {
    let definitions = match parsed.scope {
        TransitionScope::Kroma => kroma,
        TransitionScope::Outgoing => outgoing
            .ok_or_else(|| anyhow!("Transition scope 'outgoing' is unavailable in this context"))?,
        TransitionScope::Incoming => incoming
            .ok_or_else(|| anyhow!("Transition scope 'incoming' is unavailable in this context"))?,
    };

    let definition = definitions.get(&parsed.id).ok_or_else(|| {
        anyhow!(
            "Transition '{}.{}' is not defined",
            match parsed.scope {
                TransitionScope::Kroma => "kroma",
                TransitionScope::Outgoing => "outgoing",
                TransitionScope::Incoming => "incoming",
            },
            parsed.id
        )
    })?;

    let duration = parsed.duration_override.unwrap_or(definition.duration);
    if !duration.is_finite() || duration <= 0.0 {
        bail!(
            "Effective transition duration for '{}.{}' must be finite and greater than zero",
            match parsed.scope {
                TransitionScope::Kroma => "kroma",
                TransitionScope::Outgoing => "outgoing",
                TransitionScope::Incoming => "incoming",
            },
            parsed.id
        );
    }

    Ok(ResolvedTransition {
        scope: parsed.scope,
        id: &parsed.id,
        definition,
        duration,
    })
}

#[cfg(test)]
mod tests {
    use super::{TransitionScope, parse_transition_spec, resolve_transition_spec};
    use crate::types::TransitionDef;
    use indexmap::IndexMap;

    fn defs(ids: &[(&str, f64)]) -> IndexMap<String, TransitionDef> {
        ids.iter()
            .map(|(id, duration)| {
                (
                    (*id).to_string(),
                    TransitionDef {
                        shader: format!("{}.frag", id),
                        duration: *duration,
                        uniforms: IndexMap::new(),
                        textures: IndexMap::new(),
                        buffers: IndexMap::new(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn parses_with_duration_override() {
        let spec = parse_transition_spec("kroma.fade:0.8").expect("valid spec");
        assert_eq!(spec.scope, TransitionScope::Kroma);
        assert_eq!(spec.id, "fade");
        assert_eq!(spec.duration_override, Some(0.8));
    }

    #[test]
    fn parses_without_duration_override() {
        let spec = parse_transition_spec("incoming.cinematic").expect("valid spec");
        assert_eq!(spec.scope, TransitionScope::Incoming);
        assert_eq!(spec.id, "cinematic");
        assert!(spec.duration_override.is_none());
    }

    #[test]
    fn rejects_missing_scope_separator() {
        assert!(parse_transition_spec("kromafade:0.5").is_err());
    }

    #[test]
    fn rejects_non_positive_duration() {
        assert!(parse_transition_spec("kroma.fade:0").is_err());
        assert!(parse_transition_spec("kroma.fade:-1").is_err());
    }

    #[test]
    fn rejects_invalid_id_characters() {
        assert!(parse_transition_spec("outgoing.fade*bad").is_err());
    }

    #[test]
    fn rejects_empty_transition_id() {
        assert!(parse_transition_spec("kroma.:0.8").is_err());
    }

    #[test]
    fn resolve_uses_scope_and_duration_override() {
        let kroma = defs(&[("fade", 0.6)]);
        let incoming = defs(&[("cinematic", 1.2)]);

        let parsed = parse_transition_spec("incoming.cinematic:0.8").expect("valid spec");
        let resolved =
            resolve_transition_spec(&parsed, &kroma, None, Some(&incoming)).expect("must resolve");

        assert_eq!(resolved.scope, TransitionScope::Incoming);
        assert_eq!(resolved.id, "cinematic");
        assert!((resolved.duration - 0.8).abs() < 1e-6);
        assert_eq!(resolved.definition.shader, "cinematic.frag");
    }

    #[test]
    fn resolve_fails_on_missing_context() {
        let kroma = defs(&[("fade", 0.6)]);
        let parsed = parse_transition_spec("outgoing.fade").expect("valid spec");
        assert!(resolve_transition_spec(&parsed, &kroma, None, None).is_err());
    }

    #[test]
    fn resolve_fails_on_missing_id() {
        let kroma = defs(&[("fade", 0.6)]);
        let parsed = parse_transition_spec("kroma.pixelate").expect("valid spec");
        assert!(resolve_transition_spec(&parsed, &kroma, None, None).is_err());
    }
}
