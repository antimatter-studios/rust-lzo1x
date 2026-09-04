//! `lzo1x` — compress and decompress raw LZO1X blocks.
//!
//! Raw blocks, deliberately: no container, no header, no framing. That
//! is what a btrfs extent or a SquashFS block actually holds, and it is
//! what this crate's library API takes and returns. The reference CLI
//! only speaks its own container format, so the two are not
//! interchangeable — `tests/oracle/lzop_container.py` translates
//! between them, which is how the cross-validation gate works.
//!
//! Existing to make that gate a shell one-liner is most of the point:
//! both directions of the check are then a pipe rather than a test
//! harness that has to link this crate.
//!
//! Usage:
//!   lzo1x compress   <in> <out>
//!   lzo1x decompress <in> <out> <uncompressed-size>
//!
//! `decompress` needs the uncompressed size because the format does not
//! carry it. Callers always know it from somewhere else — a btrfs extent
//! header, a SquashFS block table — so the decoder takes it as a bound
//! rather than guessing and growing.

use std::process::exit;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage =
        "usage:\n  lzo1x compress   <in> <out>\n  lzo1x decompress <in> <out> <uncompressed-size>";

    match args.get(1).map(String::as_str) {
        Some("compress") if args.len() == 4 => {
            let input = read(&args[2]);
            write(&args[3], &lzo1x::compress(&input));
        }
        Some("decompress") if args.len() == 5 => {
            let input = read(&args[2]);
            let max_out: usize = args[4].parse().unwrap_or_else(|_| {
                eprintln!(
                    "lzo1x: uncompressed-size must be a number, got {:?}",
                    args[4]
                );
                exit(2);
            });
            match lzo1x::decompress(&input, max_out) {
                Ok(bytes) => write(&args[3], &bytes),
                Err(e) => {
                    eprintln!("lzo1x: {}: {e}", args[2]);
                    exit(1);
                }
            }
        }
        _ => {
            eprintln!("{usage}");
            exit(2);
        }
    }
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("lzo1x: {path}: {e}");
        exit(1);
    })
}

fn write(path: &str, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|e| {
        eprintln!("lzo1x: {path}: {e}");
        exit(1);
    });
}
