//! Machine check for the in-repo plugin manifest archive
//! (`plugins/manifest.toml`, todo.md line 312 option 1). The archive must
//! stay truthful: every plugin crate that declares a `[[bin]]` is listed
//! with the exact binary name its Cargo.toml declares, entries are unique,
//! and the recorded version tracks `workspace.package.version`. Signing
//! itself remains pending the user ruling, so `signed = false` and
//! `distribution = "none"` are asserted too — flipping either without a
//! real distribution/signing change fails here.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn parse(path: &std::path::Path) -> toml::Value {
    let raw = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    toml::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn manifest_archive_stays_truthful() {
    let root = repo_root();
    let manifest = parse(&root.join("plugins/manifest.toml"));

    // The current state of the world: nothing is signed, nothing ships.
    assert_eq!(
        manifest["signed"].as_bool(),
        Some(false),
        "signed=true requires a real trust anchor (todo.md decision point 2)"
    );
    assert_eq!(
        manifest["distribution"].as_str(),
        Some("none"),
        "a distribution source requires a real pipeline (todo.md decision point 1)"
    );

    // Archive version must track the workspace version.
    let workspace = parse(&root.join("Cargo.toml"));
    assert_eq!(
        workspace["workspace"]["package"]["version"].as_str(),
        manifest["version"].as_str(),
        "manifest version must equal workspace.package.version"
    );

    // Every archived entry names an existing plugin crate by its exact
    // [[bin]] name, and entries are unique.
    let entries = manifest["plugin"].as_array().expect("[[plugin]] entries");
    let mut archived = BTreeSet::new();
    for entry in entries {
        let dir = entry["source"].as_str().expect("source path");
        let cargo = parse(&root.join(dir).join("Cargo.toml"));
        let declared = cargo["bin"][0]["name"]
            .as_str()
            .unwrap_or_else(|| panic!("{dir}: manifest lists a source with no [[bin]]"));
        assert_eq!(
            entry["binary"].as_str(),
            Some(declared),
            "{dir}: manifest binary must equal the Cargo.toml [[bin]] name"
        );
        assert!(
            archived.insert(dir.to_owned()),
            "{dir}: duplicate plugin entry"
        );
    }

    // And the reverse direction: no plugin crate that declares a binary
    // may go unarchived.
    for entry in fs::read_dir(root.join("plugins")).expect("plugins/ exists") {
        let dir = entry.expect("plugin dir").path();
        let cargo_path = dir.join("Cargo.toml");
        if !cargo_path.is_file() {
            continue;
        }
        let cargo = parse(&cargo_path);
        if cargo.get("bin").is_none() {
            continue; // library or shim-only directory (clash-api, python)
        }
        let rel = format!(
            "plugins/{}",
            dir.file_name().expect("dir name").to_str().expect("utf8")
        );
        assert!(
            archived.contains(&rel),
            "{rel} declares a [[bin]] but is missing from plugins/manifest.toml"
        );
    }
}
