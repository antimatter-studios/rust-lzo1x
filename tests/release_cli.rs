//! A release ships the `lzo1x` tool, not only the crate (#38).
//!
//! The tool is packaged by rust-fs-core's release-cli workflow, the
//! family's one copy of those jobs, from what Cargo.toml declares under
//! `[package.metadata.package-cli]`. These read both files as data: a
//! release that dropped either would publish the crate and quietly ship no
//! binary, which is how the tool went unreleased in the first place.

use std::path::Path;

fn read(path: &str) -> String {
    let full = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{}: {e}", full.display()))
}

#[test]
fn the_manifest_declares_the_tool_for_packaging() {
    let manifest = read("Cargo.toml");
    let section = manifest
        .split("[package.metadata.package-cli]")
        .nth(1)
        .expect("Cargo.toml has no [package.metadata.package-cli]: the release packages no tool");
    let section = section.split("\n[").next().unwrap();
    assert!(
        section.contains("\"lzo1x\" = 1"),
        "[package.metadata.package-cli] does not name lzo1x in section 1:\n{section}"
    );
    assert!(
        manifest.contains("name = \"rust-lzo1x\""),
        "there is no rust-lzo1x binary for the package to carry"
    );
}

#[test]
fn the_release_runs_the_tool_packaging_job() {
    let release = read(".github/workflows/release.yml");
    assert!(
        release.contains("antimatter-studios/rust-fs-core/.github/workflows/release-cli.yml@"),
        "release.yml does not call rust-fs-core's release-cli workflow, so no tarball is attached"
    );
}

/// The packaging script refuses a tarball without the caveats it prints
/// after `brew install`, and it found that out at the v0.3.0 tag, after
/// the crate had published: the release job is the only place it runs.
/// So the file is required here, on every pull request.
#[test]
fn the_tarball_has_the_caveats_the_packaging_requires() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/CAVEATS");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        !text.trim().is_empty(),
        "{} is missing or empty, and package-cli refuses to package without it",
        path.display()
    );
}
