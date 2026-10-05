//! `.lzo` files, judged by the reference compressor (#39).
//!
//! Both directions, whole files: what `lzo1x` writes, `lzop` tests clean
//! and decompresses to the same bytes; what `lzop` writes, at its fast and
//! its best levels, with Adler-32 or CRC-32 checksums, and with blocks it
//! could not shrink, `lzo1x` tests clean, lists with the right name and
//! size, and decompresses to the same bytes.
//!
//! `lzop` is run as an external process only; nothing from it is linked,
//! copied or redistributed. Like `tests/oracle_lzop.rs` these are
//! `#[ignore]`-gated, so a checkout without `lzop` has a green `cargo
//! test`, and CI runs them with `-- --ignored`, where it is installed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: Option<&str> = option_env!("CARGO_BIN_EXE_rust-lzo1x");

fn lzo1x() -> Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new(BIN.expect(
        "the rust-lzo1x binary is built only with `--features cli`; run cargo test with it",
    ));
    cmd.arg0("lzo1x");
    cmd
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lzo1x-oracle-files-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[track_caller]
fn ok(cmd: &mut Command) -> Output {
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("{cmd:?} would not run ({e}); the oracle needs lzop installed"));
    assert!(
        out.status.success(),
        "{cmd:?} failed ({:?}): {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn payload(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..len)
        .map(|i| {
            if (i / 8192) % 3 != 2 {
                b"pack my box with five dozen liquor jugs; "[i % 41]
            } else {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            }
        })
        .collect()
}

fn sizes() -> Vec<(usize, u32)> {
    vec![
        (0, 1),
        (1, 2),
        (4096, 3),
        (262_144, 4),
        (262_145, 5),
        (900_000, 6),
    ]
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
#[ignore = "needs lzop; CI runs it with --ignored"]
fn what_lzo1x_writes_lzop_tests_and_decompresses() {
    let dir = scratch("ours-to-lzop");
    for (len, seed) in sizes() {
        let bytes = payload(len, seed);
        let file = write(&dir, &format!("o{len}.bin"), &bytes);
        ok(lzo1x().arg("-k").arg(&file));
        let lzo = dir.join(format!("o{len}.bin.lzo"));
        ok(Command::new("lzop").arg("-t").arg(&lzo));
        let back = ok(Command::new("lzop").args(["-d", "-c"]).arg(&lzo));
        assert!(
            back.stdout == bytes,
            "{len}: lzop decompresses different bytes from ours"
        );
    }
}

#[test]
#[ignore = "needs lzop; CI runs it with --ignored"]
fn what_lzop_writes_lzo1x_tests_lists_and_decompresses() {
    let dir = scratch("lzop-to-ours");
    for options in [&["-1"][..], &["-9"][..], &["--crc32"][..], &["-3"][..]] {
        for (len, seed) in sizes() {
            let bytes = payload(len, seed);
            let name = format!("r{len}{}.bin", options.join("").replace('-', "_"));
            let file = write(&dir, &name, &bytes);
            ok(Command::new("lzop").args(options).arg("-f").arg(&file));
            let lzo = dir.join(format!("{name}.lzo"));
            ok(lzo1x().arg("-t").arg(&lzo));
            let listed =
                String::from_utf8_lossy(&ok(lzo1x().args(["-l", "--text"]).arg(&lzo)).stdout)
                    .into_owned();
            assert!(
                listed.contains(&name),
                "{options:?} {len}: -l does not name {name}:\n{listed}"
            );
            assert!(
                listed.contains(&len.to_string()),
                "{options:?} {len}: -l does not give the size:\n{listed}"
            );
            let back = ok(lzo1x().args(["-d", "-c"]).arg(&lzo));
            assert!(
                back.stdout == bytes,
                "{options:?} {len}: we decompress different bytes from lzop's"
            );
        }
    }
}
