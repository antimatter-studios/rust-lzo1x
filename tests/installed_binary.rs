//! The CLI suites run against the installed `lzo1x`, not only the one
//! cargo just built (#45).
//!
//! `tests/cli.rs`, `tests/cli_files.rs` and `tests/oracle_lzop_files.rs`
//! found the tool through `CARGO_BIN_EXE_rust-lzo1x` and nothing else, so
//! the binary a release tarball or a Homebrew install puts on PATH was
//! never run by them: #38's last condition was checked by hand. These
//! checks read the files as text.
//!
//! - Each suite takes the binary from `LZO1X_BIN` when it is set.
//! - CI packages the release tarball the way `release.yml` does, unpacks
//!   it into a prefix, and runs all three suites, oracle tests included,
//!   against `bin/lzo1x` there; `ci-ok` waits for that job.
//! - The README says how to run them against an installed copy.

use std::fs;
use std::path::PathBuf;

fn read(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

const SUITES: [&str; 3] = ["cli", "cli_files", "oracle_lzop_files"];

/// The text of the job `name` in a workflow: from its key to the next
/// job key at the same indent.
fn job<'a>(workflow: &'a str, name: &str) -> Option<&'a str> {
    let start = workflow.find(&format!("\n  {name}:\n"))? + 1;
    let rest = &workflow[start..];
    let mut end = rest.len();
    let mut at = rest.find('\n').unwrap_or(rest.len());
    while at < rest.len() {
        let line = &rest[at + 1..];
        let key = line.split('\n').next().unwrap_or("");
        if key.len() > 2
            && key.starts_with("  ")
            && key[2..].starts_with(|c: char| c.is_ascii_alphanumeric())
            && key.trim_end().ends_with(':')
        {
            end = at;
            break;
        }
        at += 1 + line.find('\n').unwrap_or(line.len());
    }
    Some(&rest[..end])
}

#[test]
fn every_cli_suite_takes_the_binary_from_lzo1x_bin() {
    for suite in SUITES {
        let text = read(&format!("tests/{suite}.rs"));
        assert!(
            text.contains("\"LZO1X_BIN\""),
            "tests/{suite}.rs does not read LZO1X_BIN, so it cannot run against an installed lzo1x"
        );
    }
}

#[test]
fn ci_runs_every_cli_suite_against_the_packaged_binary() {
    let ci = read(".github/workflows/ci.yml");
    let installed = job(&ci, "installed").expect("ci.yml has no `installed` job");
    assert!(
        installed.contains("package-cli"),
        "the installed job does not build the release tarball with package-cli"
    );
    assert!(
        installed.contains("LZO1X_BIN"),
        "the installed job does not point the suites at the unpacked binary"
    );
    for suite in SUITES {
        assert!(
            installed.contains(&format!("--test {suite}")),
            "the installed job does not run tests/{suite}.rs"
        );
    }
    assert!(
        installed.contains("--include-ignored"),
        "the installed job leaves out the oracle tests, which are #[ignore]-gated"
    );
    let gate = job(&ci, "ci-ok").expect("ci.yml has a ci-ok job");
    let needs = gate
        .lines()
        .find(|l| l.trim_start().starts_with("needs:"))
        .expect("ci-ok has needs");
    assert!(
        needs.contains("installed"),
        "ci-ok does not wait for the installed job: {needs}"
    );
}

#[test]
fn the_readme_says_how_to_test_an_installed_copy() {
    assert!(
        read("README.md").contains("LZO1X_BIN"),
        "README.md does not say how to run the suites against an installed lzo1x"
    );
}
