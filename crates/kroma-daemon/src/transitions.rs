use indexmap::IndexMap;
use kroma_shared::types::TransitionDef;

pub struct BuiltinTransitionEntry {
    pub id: &'static str,
    #[allow(dead_code)]
    pub group: &'static str,
    pub wgsl: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/builtin_transitions_registry.rs"));

pub fn builtin_transition_defs() -> IndexMap<String, TransitionDef> {
    BUILTIN_TRANSITIONS
        .iter()
        .map(|entry| {
            (
                entry.id.to_string(),
                TransitionDef {
                    shader: format!("__kroma_builtin__/{}.frag", entry.id),
                    duration: 0.8,
                    uniforms: Default::default(),
                    textures: Default::default(),
                    buffers: Default::default(),
                },
            )
        })
        .collect()
}

pub fn builtin_transition_wgsl(id: &str) -> Option<&'static str> {
    BUILTIN_TRANSITIONS
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.wgsl)
}

#[allow(dead_code)]
pub fn builtin_transition_group(id: &str) -> Option<&'static str> {
    BUILTIN_TRANSITIONS
        .iter()
        .find(|entry| entry.id == id)
        .map(|entry| entry.group)
}

#[cfg(test)]
mod tests {
    use super::{BUILTIN_TRANSITIONS, builtin_transition_group, builtin_transition_wgsl};

    #[test]
    fn registry_contains_expected_core_transitions() {
        assert!(BUILTIN_TRANSITIONS.len() >= 100);
        assert!(builtin_transition_wgsl("fade").is_some());
        assert!(builtin_transition_wgsl("cross-blur").is_some());
        assert!(builtin_transition_wgsl("wipe-left-right").is_some());
        assert!(builtin_transition_wgsl("wipe-top-bottom").is_some());
        assert!(builtin_transition_wgsl("pixelate").is_some());
        assert!(builtin_transition_wgsl("dissolve-noise").is_some());
    }

    #[test]
    fn registry_keeps_group_metadata() {
        assert_eq!(builtin_transition_group("fade"), Some("motion"));
        assert_eq!(
            builtin_transition_group("glitch-datamosh-style"),
            Some("digital")
        );
        assert_eq!(
            builtin_transition_group("iris-circle-open"),
            Some("cinematic")
        );
        assert!(builtin_transition_group("missing-transition").is_none());
    }
}
