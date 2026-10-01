//! Every pull request gets CI, whatever branch it is based on (#26).
//!
//! `branches:` under `pull_request:` filters on the pull request's BASE.
//! `ci.yml` carried `branches: [main]`, so a pull request stacked on
//! another one's branch got no CI run at all -- and a pull request with no
//! checks has no failing checks, so `gh pr view` could call it `CLEAN`.
//! Work arrives here as stacks, so every layer above the bottom would have
//! merged unchecked.
//!
//! This reads each workflow as YAML rather than scanning its text, for the
//! reason `tests/ci_profile.rs` gives: a guard that misreads its input
//! reports protection it is not providing.

use saphyr::{LoadableYamlNode, Yaml};
use std::path::PathBuf;

fn field<'a, 'b>(node: &'a Yaml<'b>, name: &str) -> Option<&'a Yaml<'b>> {
    node.as_mapping()?
        .iter()
        .find(|(key, _)| key.as_str() == Some(name))
        .map(|(_, value)| value)
}

/// What a workflow's `pull_request` trigger says: `None` when the workflow
/// does not run on pull requests, otherwise the base-branch filters it sets.
fn pull_request_filters(text: &str) -> Option<Vec<String>> {
    let documents = Yaml::load_from_str(text).unwrap_or_else(|e| {
        panic!("workflow is not valid YAML: {e}; a guard that cannot read it must fail")
    });
    let on = field(documents.first()?, "on")?;
    let names_it = on.as_str() == Some("pull_request")
        || on
            .as_sequence()
            .is_some_and(|s| s.iter().any(|i| i.as_str() == Some("pull_request")));
    if names_it {
        return Some(Vec::new());
    }
    let trigger = field(on, "pull_request")?;
    Some(
        ["branches", "branches-ignore"]
            .into_iter()
            .filter(|key| field(trigger, key).is_some())
            .map(str::to_string)
            .collect(),
    )
}

#[test]
fn the_reading_tells_a_filtered_trigger_from_an_open_one() {
    let filtered =
        "on:\n  push:\n    branches: [main]\n  pull_request:\n    branches: [main]\njobs: {}\n";
    assert_eq!(
        pull_request_filters(filtered),
        Some(vec!["branches".to_string()])
    );
    let ignored = "on:\n  pull_request:\n    branches-ignore: ['wip/**']\njobs: {}\n";
    assert_eq!(
        pull_request_filters(ignored),
        Some(vec!["branches-ignore".to_string()])
    );
    let open = "on:\n  push:\n    branches: [main]\n  pull_request:\njobs: {}\n";
    assert_eq!(pull_request_filters(open), Some(Vec::new()));
    let listed = "on: [push, pull_request]\njobs: {}\n";
    assert_eq!(pull_request_filters(listed), Some(Vec::new()));
    let none = "on:\n  push:\n    tags: ['v*']\njobs: {}\n";
    assert_eq!(pull_request_filters(none), None);
}

#[test]
fn no_workflow_limits_pull_request_ci_to_one_base_branch() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".github")
        .join("workflows");
    let mut on_pull_requests = 0;
    let mut filtered = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("workflow entry").path();
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yml" | "yaml")
        ) {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        if let Some(filters) = pull_request_filters(&text) {
            on_pull_requests += 1;
            if !filters.is_empty() {
                filtered.push(format!(
                    "{}: pull_request {}",
                    path.display(),
                    filters.join(", ")
                ));
            }
        }
    }
    assert!(
        on_pull_requests >= 1,
        "no workflow runs on pull_request at all, so no pull request is checked"
    );
    assert!(
        filtered.is_empty(),
        "these workflows skip a pull request whose base is not the one they name, \
         so a stacked pull request gets no CI and reads as CLEAN:\n{}",
        filtered.join("\n")
    );
}
