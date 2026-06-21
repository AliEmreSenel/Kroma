use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn slug_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .expect("transition shader file stem")
        .replace('_', "-")
        .to_ascii_lowercase()
}

fn walk_wgsl_files(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(root).expect("read transition shader dir");
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.is_dir() {
            walk_wgsl_files(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("wgsl") {
            out.push(path);
        }
    }
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let transitions_root = manifest_dir.join("src/shaders/transitions");
    println!("cargo:rerun-if-changed={}", transitions_root.display());

    let mut files = Vec::new();
    walk_wgsl_files(&transitions_root, &mut files);
    files.sort();

    let mut by_id: BTreeMap<String, (String, String)> = BTreeMap::new();
    for path in files {
        let rel = path
            .strip_prefix(&manifest_dir)
            .expect("path under crate root")
            .to_string_lossy()
            .replace('\\', "/");
        let group = path
            .parent()
            .and_then(|p| p.strip_prefix(&transitions_root).ok())
            .and_then(|p| p.iter().next())
            .and_then(|s| s.to_str())
            .unwrap_or("misc")
            .to_ascii_lowercase();
        let id = slug_from_path(&path);

        if by_id.insert(id.clone(), (group, rel)).is_some() {
            panic!("Duplicate builtin transition id detected: {}", id);
        }
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let registry_path = out_dir.join("builtin_transitions_registry.rs");
    let mut file = fs::File::create(&registry_path).expect("create transition registry");

    writeln!(
        file,
        "pub static BUILTIN_TRANSITIONS: &[BuiltinTransitionEntry] = &["
    )
    .expect("write header");
    for (id, (group, rel)) in by_id {
        writeln!(
            file,
            "    BuiltinTransitionEntry {{ id: \"{}\", group: \"{}\", wgsl: include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/{}\")) }},",
            id, group, rel
        )
        .expect("write registry row");
    }
    writeln!(file, "];\n").expect("write footer");
}
