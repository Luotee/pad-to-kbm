//! A release bumps three files together (CLAUDE.md, "Release flow"): `Cargo.toml`,
//! `tauri.conf.json` and the top `## [X.Y.Z]` heading of `CHANGELOG.md`. Pin that here so a
//! release PR that misses one fails `cargo test` (and CI) instead of a reviewer's eye.

fn changelog_top_version(changelog: &str) -> Option<&str> {
    changelog
        .lines()
        .find_map(|l| l.strip_prefix("## ["))
        .and_then(|rest| rest.split_once(']'))
        .map(|(v, _)| v)
}

#[test]
fn cargo_tauri_and_changelog_versions_agree() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let cargo = env!("CARGO_PKG_VERSION");

    let conf: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("tauri.conf.json")).unwrap()).unwrap();
    assert_eq!(conf["version"].as_str(), Some(cargo), "tauri.conf.json version != Cargo.toml {cargo}");

    let changelog = std::fs::read_to_string(root.join("../CHANGELOG.md")).unwrap();
    assert_eq!(changelog_top_version(&changelog), Some(cargo), "CHANGELOG.md top heading != Cargo.toml {cargo}");
}

#[test]
fn changelog_top_version_reads_the_first_release_heading() {
    let text = "# Changelog\n\nintro [link](x)\n\n## [3.6.0] - 2026-10-31\n\n## [3.5.1] - 2026-10-30\n";
    assert_eq!(changelog_top_version(text), Some("3.6.0"));
    assert_eq!(changelog_top_version("# Changelog\n"), None);
}
