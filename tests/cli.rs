//! The `lzo1x` tool, as a user runs it (#38).
//!
//! The library's tests call `compress` and `decompress` directly, and
//! `tests/oracle_lzop.rs` holds them to the reference compressor. Nothing
//! ran the binary, so it could break without a red check. These run it:
//! under the name it is installed as, with real files, through every way
//! it can be told something wrong.
//!
//! The binary is built only with the `cli` feature; without it these
//! tests FAIL naming the fix rather than skipping.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: Option<&str> = option_env!("CARGO_BIN_EXE_rust-lzo1x");

/// The binary under test: `LZO1X_BIN` when it is set, which is how CI and
/// a developer run this suite against an installed copy (#45), and
/// otherwise the one cargo just built. A path that is not a file fails;
/// it never falls back.
fn bin() -> String {
    if let Ok(path) = std::env::var("LZO1X_BIN") {
        assert!(
            std::path::Path::new(&path).is_file(),
            "LZO1X_BIN={path} is not a file"
        );
        return path;
    }
    BIN.expect(
        "the rust-lzo1x binary is built only with `--features cli`; run cargo test with it, \
         or set LZO1X_BIN to an installed lzo1x",
    )
    .to_string()
}
const CRATE: &str = env!("CARGO_PKG_NAME");
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The program as a user runs it: `argv[0]` is `lzo1x`.
fn lzo1x() -> Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(bin());
    cmd.arg0("lzo1x");
    cmd
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lzo1x-cli-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn text(out: &[u8]) -> String {
    String::from_utf8_lossy(out).into_owned()
}

#[track_caller]
fn ok(cmd: &mut Command) -> Output {
    let out = cmd.output().expect("spawn lzo1x");
    assert!(
        out.status.success(),
        "{cmd:?} failed ({:?}): {}",
        out.status.code(),
        text(&out.stderr)
    );
    out
}

/// Something compressible and something that is not, at a length.
fn payload(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..len)
        .map(|i| {
            if (i / 512) % 2 == 0 {
                b"the quick brown fox jumps over the lazy dog. "[i % 45]
            } else {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            }
        })
        .collect()
}

fn roundtrip(dir: &Path, bytes: &[u8], name: &str) {
    let raw = dir.join(format!("{name}.raw"));
    let packed = dir.join(format!("{name}.lzo1x"));
    let back = dir.join(format!("{name}.back"));
    std::fs::write(&raw, bytes).unwrap();
    ok(lzo1x().arg("--raw").arg("-o").arg(&packed).arg(&raw));
    ok(lzo1x()
        .args(["--raw", "-d", "--size"])
        .arg(bytes.len().to_string())
        .arg("-o")
        .arg(&back)
        .arg(&packed));
    assert!(
        std::fs::read(&back).unwrap() == bytes,
        "{name}: {} bytes compressed and decompressed by the tool came back different",
        bytes.len()
    );
}

#[test]
fn what_it_compresses_it_decompresses_to_the_same_bytes() {
    let dir = scratch("roundtrip");
    for (len, seed) in [(0, 1), (1, 2), (3, 3), (4096, 4), (65_537, 5), (1 << 20, 6)] {
        roundtrip(&dir, &payload(len, seed), &format!("p{len}"));
    }
}

#[test]
fn it_answers_version_as_itself_and_its_crate() {
    for flag in ["--version", "-V"] {
        let out = ok(lzo1x().arg(flag));
        assert_eq!(
            text(&out.stdout).trim_end(),
            format!("lzo1x ({CRATE}) {VERSION}"),
            "lzo1x {flag}"
        );
    }
}

#[test]
fn its_help_carries_an_example() {
    let out = ok(lzo1x().arg("--help"));
    assert!(
        text(&out.stdout).contains("Examples:"),
        "{}",
        text(&out.stdout)
    );
}

#[test]
fn the_repository_entry_point_lists_it_for_packaging() {
    use std::os::unix::process::CommandExt;
    // Under the repository's own name, as packaging calls it: an installed
    // `bin/lzo1x` is the same program, answering as the tool by default.
    let out = ok(Command::new(bin())
        .arg0("rust-lzo1x")
        .args(["generate", "names"]));
    assert_eq!(text(&out.stdout).trim(), "lzo1x");
}

/// Fails, with a message on stderr, an exit status of `code`, and no
/// panic.
#[track_caller]
fn refused(cmd: &mut Command, code: i32, says: &str) {
    let out = cmd.output().expect("spawn lzo1x");
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(code), "{cmd:?}: {err}");
    assert!(!err.contains("panicked"), "{cmd:?} panicked: {err}");
    assert!(
        err.to_lowercase().contains(&says.to_lowercase()),
        "{cmd:?} does not say {says:?}: {err}"
    );
}

#[test]
fn every_wrong_input_is_refused_with_a_reason() {
    let dir = scratch("refusals");
    let raw = dir.join("in.raw");
    let packed = dir.join("in.lzo1x");
    let out = dir.join("out");
    let bytes = payload(10_000, 9);
    std::fs::write(&raw, &bytes).unwrap();
    ok(lzo1x().arg("--raw").arg("-o").arg(&packed).arg(&raw));

    // The command line.
    refused(lzo1x().arg("--squash"), 2, "squash");
    refused(
        lzo1x()
            .args(["--raw", "-d", "--size", "lots", "-o"])
            .arg(&out)
            .arg(&packed),
        2,
        "lots",
    );
    refused(
        lzo1x().args(["--raw", "-d", "-o"]).arg(&out).arg(&packed),
        2,
        "size",
    );

    // The files.
    refused(
        lzo1x()
            .arg("--raw")
            .arg("-o")
            .arg(&out)
            .arg(dir.join("absent")),
        1,
        "absent",
    );
    refused(
        lzo1x()
            .arg("--raw")
            .arg("-o")
            .arg(dir.join("no/such/dir/out"))
            .arg(&raw),
        1,
        "no/such/dir",
    );

    // The data: a bound smaller than what the stream holds, and a stream
    // cut short, are each refused rather than half-written.
    refused(
        lzo1x()
            .args(["--raw", "-d", "--size"])
            .arg((bytes.len() - 1).to_string())
            .arg("-o")
            .arg(&out)
            .arg(&packed),
        1,
        "in.lzo1x",
    );
    let packed_bytes = std::fs::read(&packed).unwrap();
    let cut = dir.join("cut.lzo1x");
    std::fs::write(&cut, &packed_bytes[..packed_bytes.len() / 2]).unwrap();
    refused(
        lzo1x()
            .args(["--raw", "-d", "--size"])
            .arg(bytes.len().to_string())
            .arg("-o")
            .arg(&out)
            .arg(&cut),
        1,
        "cut.lzo1x",
    );
}
