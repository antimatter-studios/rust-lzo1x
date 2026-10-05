//! `lzo1x` handles `.lzo` files the way `gzip` handles `.gz` files (#39).
//!
//! Compressing `FILE` makes `FILE.lzo` and removes `FILE`; `-d` reverses
//! it, giving the file back its name, mode and modification time; `-k`
//! keeps the input; `-c` writes to stdout and keeps it; with no file the
//! tool filters stdin to stdout; `-t` tests and `-l` lists; an existing
//! output is not overwritten without `-f`. These run the tool against
//! itself; `tests/oracle_lzop_files.rs` holds the files to the reference
//! compressor.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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

fn lzo1x() -> Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(bin());
    cmd.arg0("lzo1x");
    cmd
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lzo1x-files-{}-{tag}", std::process::id()));
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

#[track_caller]
fn refused(cmd: &mut Command, says: &str) -> Output {
    let out = cmd.output().expect("spawn lzo1x");
    let err = text(&out.stderr);
    assert!(!out.status.success(), "{cmd:?} succeeded: {err}");
    assert_ne!(out.status.code(), Some(101), "{cmd:?} panicked: {err}");
    assert!(
        err.to_lowercase().contains(&says.to_lowercase()),
        "{cmd:?} does not say {says:?}: {err}"
    );
    out
}

/// Half text, half noise, so some blocks compress and some are stored.
fn payload(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..len)
        .map(|i| {
            if (i / 4096) % 2 == 0 {
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

fn write_file(path: &Path, bytes: &[u8], mode: u32, mtime: i64) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime as u64);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

fn mtime_of(path: &Path) -> u64 {
    std::fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn a_file_becomes_file_lzo_and_comes_back_with_its_name_mode_and_time() {
    let dir = scratch("roundtrip");
    // Empty, one block, several blocks with a short last one.
    for (len, seed) in [(0usize, 1u32), (1000, 2), (700_000, 3)] {
        let file = dir.join(format!("f{len}.bin"));
        let lzo = dir.join(format!("f{len}.bin.lzo"));
        let bytes = payload(len, seed);
        write_file(&file, &bytes, 0o640, 1_600_000_000);

        ok(lzo1x().arg(&file));
        assert!(lzo.is_file(), "{len}: no {}", lzo.display());
        assert!(!file.exists(), "{len}: the input was not removed");

        ok(lzo1x().arg("-d").arg(&lzo));
        assert!(!lzo.exists(), "{len}: the .lzo was not removed after -d");
        assert!(
            std::fs::read(&file).unwrap() == bytes,
            "{len}: different bytes came back"
        );
        let meta = std::fs::metadata(&file).unwrap();
        assert_eq!(meta.permissions().mode() & 0o7777, 0o640, "{len}: mode");
        assert_eq!(mtime_of(&file), 1_600_000_000, "{len}: modification time");
    }
}

#[test]
fn keep_stdout_and_stdin_behave_as_gzip_does() {
    let dir = scratch("streams");
    let file = dir.join("data.txt");
    let bytes = payload(300_000, 7);
    write_file(&file, &bytes, 0o644, 1_700_000_000);

    // -k keeps the input.
    ok(lzo1x().arg("-k").arg(&file));
    assert!(file.is_file(), "-k removed the input");
    assert!(dir.join("data.txt.lzo").is_file());

    // -c writes the archive to stdout and keeps the input.
    let to_stdout = ok(lzo1x().arg("-c").arg(&file));
    assert!(file.is_file(), "-c removed the input");
    assert_eq!(
        &to_stdout.stdout[..9],
        &[0x89, b'L', b'Z', b'O', 0x00, 0x0d, 0x0a, 0x1a, 0x0a],
        "-c did not write an .lzo file to stdout"
    );

    // No file: stdin to stdout, both ways.
    let mut child = lzo1x()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&bytes).unwrap();
    let packed = child.wait_with_output().unwrap();
    assert!(packed.status.success(), "{}", text(&packed.stderr));
    let mut child = lzo1x()
        .arg("-d")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&packed.stdout)
        .unwrap();
    let back = child.wait_with_output().unwrap();
    assert!(back.status.success(), "{}", text(&back.stderr));
    assert!(
        back.stdout == bytes,
        "stdin to stdout and back changed the bytes"
    );

    // -d -c from a file to stdout.
    let out = ok(lzo1x().args(["-d", "-c"]).arg(dir.join("data.txt.lzo")));
    assert!(out.stdout == bytes, "-d -c changed the bytes");
}

#[test]
fn test_and_list_report_on_an_archive_without_writing() {
    let dir = scratch("test-list");
    let file = dir.join("listed.bin");
    let bytes = payload(600_000, 11);
    write_file(&file, &bytes, 0o600, 1_650_000_000);
    ok(lzo1x().arg(&file));
    let lzo = dir.join("listed.bin.lzo");

    ok(lzo1x().arg("-t").arg(&lzo));
    let listed = text(&ok(lzo1x().arg("-l").arg(&lzo)).stdout);
    assert!(
        listed.contains("listed.bin"),
        "-l does not name the file:\n{listed}"
    );
    assert!(
        listed.contains("600000"),
        "-l does not give the size:\n{listed}"
    );
    assert!(!file.exists(), "-t or -l wrote the file out");

    // A flipped byte inside a block's data is found by its checksum.
    let mut damaged = std::fs::read(&lzo).unwrap();
    let at = damaged.len() - 100;
    damaged[at] ^= 0x40;
    let bad = dir.join("bad.bin.lzo");
    std::fs::write(&bad, &damaged).unwrap();
    refused(lzo1x().arg("-t").arg(&bad), "bad.bin.lzo");
    refused(lzo1x().arg("-d").arg(&bad), "bad.bin.lzo");
    assert!(
        !dir.join("bad.bin").exists(),
        "a damaged archive was decompressed to a file"
    );
}

#[test]
fn nothing_is_overwritten_or_guessed_at() {
    let dir = scratch("refusals");
    let file = dir.join("x.txt");
    write_file(
        &file,
        b"some text, some text, some text",
        0o644,
        1_700_000_000,
    );
    ok(lzo1x().arg("-k").arg(&file));

    // The .lzo exists: compressing again is refused, and forced with -f.
    refused(lzo1x().arg("-k").arg(&file), "exists");
    ok(lzo1x().args(["-k", "-f"]).arg(&file));

    // The output of -d exists: refused without -f.
    refused(lzo1x().arg("-d").arg(dir.join("x.txt.lzo")), "exists");

    // -d on a name without .lzo has nothing to strip.
    refused(lzo1x().arg("-d").arg(&file), ".lzo");

    // A file that is not an .lzo file.
    let fake = dir.join("fake.lzo");
    std::fs::write(&fake, b"not an lzop file at all").unwrap();
    refused(lzo1x().arg("-d").arg(&fake), "fake.lzo");

    // A missing input.
    refused(lzo1x().arg(dir.join("absent")), "absent");
}
