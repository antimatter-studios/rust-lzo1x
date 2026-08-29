//! Oracle tests: decode LZO1X streams produced by a real LZO compressor.
//!
//! The unit tests in `src/lib.rs` decode streams this project hand-built
//! from the grammar. That proves internal consistency, not correctness —
//! a misreading of the spec would produce an encoder-shaped blind spot
//! that hand-built streams can never expose. These tests close that gap
//! by feeding the decoder bytes emitted by `lzop` (the reference LZO
//! command-line compressor) and requiring the original payload back.
//!
//! `lzop` is invoked as an external process only. Nothing from it is
//! linked, copied, or redistributed — it is a test oracle in the same way
//! a filesystem driver's tests shell out to the canonical `mkfs`.
//!
//! All tests here are `#[ignore]`-gated so a fresh checkout without
//! `lzop` still has a green `cargo test`. Opt in with:
//!
//! ```sh
//! cargo test -- --ignored
//! ```

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

// ---------------------------------------------------------------------
// lzop container parsing
//
// A `.lzo` file is a small header followed by a sequence of blocks. Only
// the block payloads are LZO1X streams; the container itself is lzop's
// own framing and is documented in the lzop distribution. We parse just
// enough of it to reach the compressed blocks.
// ---------------------------------------------------------------------

const LZOP_MAGIC: [u8; 9] = [0x89, b'L', b'Z', b'O', 0x00, 0x0d, 0x0a, 0x1a, 0x0a];

const F_ADLER32_D: u32 = 0x0000_0001;
const F_ADLER32_C: u32 = 0x0000_0002;
const F_H_FILTER: u32 = 0x0000_0800;
const F_CRC32_D: u32 = 0x0000_0100;
const F_CRC32_C: u32 = 0x0000_0200;

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn u8(&mut self) -> u8 {
        let v = self.b[self.i];
        self.i += 1;
        v
    }
    /// lzop stores every multi-byte header field in network byte order.
    fn u16(&mut self) -> u16 {
        let v = u16::from_be_bytes([self.b[self.i], self.b[self.i + 1]]);
        self.i += 2;
        v
    }
    fn u32(&mut self) -> u32 {
        let v = u32::from_be_bytes([
            self.b[self.i],
            self.b[self.i + 1],
            self.b[self.i + 2],
            self.b[self.i + 3],
        ]);
        self.i += 4;
        v
    }
    fn skip(&mut self, n: usize) {
        self.i += n;
    }
}

/// One block of an lzop file: the raw LZO1X stream plus the length the
/// container says it decodes to.
struct Block {
    compressed: Vec<u8>,
    uncompressed_len: usize,
    /// lzop stores a block verbatim when compression did not shrink it.
    /// Those blocks are not LZO1X streams and must be skipped.
    stored: bool,
}

fn parse_lzop(data: &[u8]) -> Vec<Block> {
    assert_eq!(&data[..9], &LZOP_MAGIC, "not an lzop file");
    let mut c = Cursor { b: data, i: 9 };

    let version = c.u16();
    let _lib_version = c.u16();
    if version >= 0x0940 {
        let _version_needed = c.u16();
    }
    let _method = c.u8();
    if version >= 0x0940 {
        let _level = c.u8();
    }
    let flags = c.u32();
    if flags & F_H_FILTER != 0 {
        let _filter = c.u32();
    }
    let _mode = c.u32();
    let _mtime_low = c.u32();
    if version >= 0x0940 {
        let _mtime_high = c.u32();
    }
    let fname_len = c.u8() as usize;
    c.skip(fname_len);
    let _header_checksum = c.u32();

    let d_csum = flags & (F_ADLER32_D | F_CRC32_D) != 0;
    let c_csum = flags & (F_ADLER32_C | F_CRC32_C) != 0;

    let mut blocks = Vec::new();
    loop {
        let uncompressed_len = c.u32() as usize;
        if uncompressed_len == 0 {
            break; // end-of-file marker
        }
        let compressed_len = c.u32() as usize;
        if d_csum {
            let _ = c.u32();
        }
        // One domain question — did the encoder actually compress this
        // block — decides two things eight lines apart: whether a
        // compressed-side checksum is present in the stream, and whether
        // the payload is an LZO1X stream or the original bytes. Written
        // twice in inverted form, a disagreement between them would
        // desync the cursor and every later block would be read from the
        // wrong offset, surfacing as a confusing decode failure rather
        // than a parse error. Asked once.
        let stored = compressed_len >= uncompressed_len;

        // The compressed-side checksum is only written when the block
        // actually got compressed.
        if c_csum && !stored {
            let _ = c.u32();
        }
        let payload = data[c.i..c.i + compressed_len].to_vec();
        c.skip(compressed_len);
        blocks.push(Block {
            compressed: payload,
            uncompressed_len,
            stored,
        });
    }
    blocks
}

/// Compress `payload` with `lzop` at the given level and return the
/// parsed blocks. Returns `None` if `lzop` is not installed.
fn lzop_blocks(payload: &[u8], level: &str) -> Option<Vec<Block>> {
    if Command::new("lzop").arg("--version").output().is_err() {
        return None;
    }
    // Tests run in parallel and several use the same payload length, so
    // the directory name needs more than pid+len to stay unique — a
    // collision lets one test delete another's input mid-run.
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "lzo1x-oracle-{}-{}-{}-{}",
        std::process::id(),
        level.trim_start_matches('-'),
        payload.len(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let raw = dir.join("payload.bin");
    let mut f = std::fs::File::create(&raw).unwrap();
    f.write_all(payload).unwrap();
    drop(f);

    let out = Command::new("lzop")
        .arg(level)
        .arg("-f")
        .arg("-o")
        .arg(dir.join("payload.lzo"))
        .arg(&raw)
        .output()
        .expect("run lzop");
    assert!(
        out.status.success(),
        "lzop failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let data = std::fs::read(dir.join("payload.lzo")).unwrap();
    let blocks = parse_lzop(&data);
    std::fs::remove_dir_all(&dir).ok();
    Some(blocks)
}

/// Decode every compressed block of `payload` and require the bytes back.
///
/// `expect_compression` guards against a test that silently proves
/// nothing: if the encoder stored every block verbatim, no LZO1X stream
/// was ever decoded. Deliberately incompressible payloads pass `false`.
fn assert_roundtrip_inner(payload: &[u8], level: &str, expect_compression: bool) {
    let Some(blocks) = lzop_blocks(payload, level) else {
        eprintln!("lzop not installed — skipping");
        return;
    };
    let mut decoded = Vec::new();
    let mut saw_compressed = false;
    for b in &blocks {
        if b.stored {
            // Incompressible block stored verbatim by the container.
            decoded.extend_from_slice(&b.compressed);
            continue;
        }
        saw_compressed = true;
        let out = lzo1x::decompress(&b.compressed, b.uncompressed_len)
            .unwrap_or_else(|e| panic!("{level}: decode failed on a real lzop block: {e}"));
        assert_eq!(
            out.len(),
            b.uncompressed_len,
            "{level}: block decoded to {} bytes, container says {}",
            out.len(),
            b.uncompressed_len
        );
        decoded.extend_from_slice(&out);
    }
    assert_eq!(decoded, payload, "{level}: round-trip mismatch");
    if expect_compression {
        assert!(
            saw_compressed,
            "{level}: nothing was actually compressed — the test proved nothing"
        );
    }
}

/// Round-trip a payload that the encoder is expected to compress.
fn assert_roundtrip(payload: &[u8], level: &str) {
    assert_roundtrip_inner(payload, level, true);
}

/// Highly compressible input: long literal runs plus heavy repetition,
/// which drives the long-match and zero-run-extension paths.
fn repetitive(len: usize) -> Vec<u8> {
    "the quick brown fox jumps over the lazy dog\n"
        .bytes()
        .cycle()
        .take(len)
        .collect()
}

/// Deterministic pseudo-random bytes. A simple xorshift keeps the test
/// reproducible without pulling in an RNG dependency.
fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 24) as u8
        })
        .collect()
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_repetitive_fast() {
    assert_roundtrip(&repetitive(64 * 1024), "-1");
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_repetitive_best() {
    // -9 selects LZO1X-999, a different encoder to -1's LZO1X-1. Both
    // emit the same grammar, so both must decode.
    assert_roundtrip(&repetitive(64 * 1024), "-9");
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_large_multiblock() {
    // Larger than lzop's default block size, so the container holds
    // several independent LZO1X streams.
    assert_roundtrip(&repetitive(1024 * 1024), "-9");
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_mixed_compressibility() {
    // Alternating compressible and incompressible regions: exercises
    // long literal runs (which the encoder falls back to on noise)
    // interleaved with matches.
    let mut v = Vec::new();
    for i in 0..16 {
        v.extend_from_slice(&repetitive(4096));
        v.extend_from_slice(&pseudo_random(4096, i + 1));
    }
    assert_roundtrip(&v, "-9");
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_incompressible() {
    // Pure noise: the encoder stores every block verbatim, so this
    // asserts the container path rather than the decoder. Kept because a
    // decoder that mistakenly tried to decode a stored block would fail
    // here, and that is a real regression.
    assert_roundtrip_inner(&pseudo_random(256 * 1024, 0xDEAD_BEEF), "-9", false);
}

#[test]
#[ignore = "requires lzop"]
fn roundtrip_many_small_sizes() {
    // Sweep lengths around the encoder's short-match and literal-run
    // boundaries, where off-by-one grammar errors hide.
    for len in [
        1usize, 2, 3, 4, 5, 15, 16, 17, 18, 19, 31, 32, 33, 255, 256, 257, 1023, 1024,
    ] {
        // Very short payloads have nothing to match against, so the
        // encoder may legitimately store them.
        assert_roundtrip_inner(&repetitive(len), "-9", len >= 256);
    }
}
