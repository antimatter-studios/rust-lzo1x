//! Pure-Rust LZO1X decompressor.
//!
//! LZO1X is the byte-stream format produced by the LZO family of
//! compressors (LZO1X-1, LZO1X-999, …). All LZO1X-* encoders share one
//! decode grammar, so a single decoder handles every variant. The format
//! shows up in SquashFS (compression id 3), Btrfs compressed extents,
//! JFFS2, and kernel crash dumps.
//!
//! This crate decodes only. There is no compressor — the decode grammar
//! is small and fully published, while a competitive encoder is a much
//! larger undertaking with no consumer in sight.
//!
//! # Provenance
//!
//! Written from the publicly published prose description of the LZO1X
//! byte-stream grammar, principally the format-description document
//! distributed as `Documentation/staging/lzo.rst` in the Linux source
//! tree — a prose specification of the on-the-wire token grammar, not
//! decompressor code. The grammar as used is reproduced below so the
//! mapping from spec to implementation is auditable in-tree.
//!
//! A byte-stream format is a set of facts, not an expressive work, and
//! this crate implements one from its published description. It carries
//! no code from any other LZO implementation.
//!
//! # The grammar
//!
//! The stream is a sequence of *instructions*. Each instruction is one
//! command byte, sometimes followed by extension/distance bytes, that
//! either emits a run of literal bytes copied straight from the input,
//! or a *match*: a back-reference that copies `len` bytes from `dist`
//! bytes earlier in the already-produced output. After most matches a
//! small number (0..=3) of trailing literals are copied immediately;
//! this count is the *state* carried into the next instruction and is
//! taken from the low 2 bits ("SS") of the match's distance operand.
//!
//! Lengths that would overflow the few bits available in the command
//! byte are extended by a run of zero bytes: each `0x00` adds 255, and
//! the terminating non-zero byte adds its own value (the "zero-run"
//! length extension).
//!
//! Instruction buckets, keyed by the command byte `t`:
//!
//! ```text
//! 0 0 0 0 L L L L  (t < 16)  -- three-way, keyed by the carried `state`:
//!     - state == 0: a literal run, len = 3 + (t==0 ? zero_run+15 : t).
//!                   Sets state = 4 afterwards (the "long-literals" sentinel).
//!     - state 1..=3 (the SS bits of the previous match): a 2-byte match,
//!                   dist = (next_byte << 2) + ((t >> 2) & 3) + 1.
//!     - state == 4: a 3-byte match (immediately after a long literal run),
//!                   dist = (next_byte << 2) + ((t >> 2) & 3) + 2049.
//!     In both match cases the new state is t & 3.
//! 0 0 0 1 H L L L  (16..=31)
//!     match, len = 2 + (LLL==0 ? zero_run+7 : LLL),
//!     dist = 16384 + (H << 14) + (LE16_operand >> 2);
//!     a token contributing nothing above that base -- H clear and the
//!     operand's upper 14 bits zero -- is the END-OF-STREAM marker,
//!     whatever LLL holds. 11 00 00 is the spelling encoders emit;
//!     13 00 00 and 17 00 00 terminate just as surely.
//! 0 0 1 L L L L L  (32..=63)
//!     match, len = 2 + (LLLLL==0 ? zero_run+31 : LLLLL),
//!     dist = (LE16_operand >> 2) + 1
//! 0 1 L D D D S S  (64..=127)
//!     match, len = 3 + ((t >> 5) & 1),
//!     dist = (next_byte << 3) + ((t >> 2) & 7) + 1
//! 1 L L D D D S S  (128..=255)
//!     match, len = 5 + ((t >> 5) & 3),
//!     dist = (next_byte << 3) + ((t >> 2) & 7) + 1
//! ```
//!
//! Stream bootstrap (first command byte only): a value >= 18 means a
//! leading literal run of `t - 17` bytes (with state = min(t-17, 4)); a
//! value of 17 is reserved as a bitstream-version marker. Values < 18
//! fall through to the normal grammar.
//!
//! # Safety
//!
//! The decoder is bounds-checked at every step and returns
//! [`Error::Malformed`] on any malformed token rather than panicking, so
//! it is safe to run on untrusted input. It contains no `unsafe`.
//!
//! # Example
//!
//! ```
//! // Four literal bytes via the bootstrap run, then the end-of-stream marker.
//! let stream = [21u8, b'a', b'b', b'c', b'd', 0x11, 0x00, 0x00];
//! let out = lzo1x::decompress(&stream, 4).unwrap();
//! assert_eq!(&out, b"abcd");
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::fmt;

/// The single failure mode of the decoder: the input is not a
/// well-formed LZO1X stream, or decoding it would exceed `max_out`.
///
/// The variant carries no position information by design — a corrupt
/// stream's first *detected* error is rarely where the corruption is,
/// so an offset would imply a precision the decoder does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The stream is truncated, contains an invalid token, references
    /// data before the start of the output, or decodes to more than the
    /// caller's `max_out` bound.
    Malformed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Malformed => f.write_str("malformed LZO1X stream"),
        }
    }
}

impl std::error::Error for Error {}

/// Result alias for [`decompress`].
pub type Result<T> = std::result::Result<T, Error>;

const E: Error = Error::Malformed;

/// Each `0x00` byte in a zero-run length extension contributes this much
/// to the running length.
const ZERO_RUN_INCREMENT: usize = 255;

/// Upper bound on a single extended length, as a defence against a
/// corrupt all-zeros stream. This is not a format limit: the grammar
/// permits arbitrarily long runs, but no real block is anywhere near
/// 16 MiB, so a length beyond it means the input is garbage rather than
/// merely large. Without this cap an all-zeros input loops until the
/// length overflows.
const MAX_EXTENDED_LENGTH: usize = 1 << 24;

/// Base distance for the `0 0 0 1 H L L L` long-distance match bucket.
/// A token in that bucket whose distance contribution above this base is
/// zero is the end-of-stream marker rather than a match; its length bits
/// are not part of that decision.
const LONG_MATCH_DISTANCE_BASE: usize = 16384;

/// Base distance for the three-byte match that follows a long literal
/// run (the `state == 4` case of the `t < 16` bucket).
const POST_LITERAL_MATCH_DISTANCE_BASE: usize = 2049;

/// Decompress an LZO1X stream.
///
/// `input` is the raw compressed block. `max_out` is an **upper bound**
/// on the decompressed length — for SquashFS that is the block size (or
/// 8 KiB for metadata blocks); for Btrfs it is the extent's uncompressed
/// length. The true output length is determined by the stream's own
/// end-of-stream marker, not by `max_out`; the bound exists so a corrupt
/// stream cannot drive an unbounded allocation.
///
/// # Errors
///
/// Returns [`Error::Malformed`] if the stream is truncated, contains an
/// invalid token, back-references before the start of the output, or
/// would decode to more than `max_out` bytes.
pub fn decompress(input: &[u8], max_out: usize) -> Result<Vec<u8>> {
    let mut d = Decoder {
        input,
        ip: 0,
        out: Vec::with_capacity(max_out.min(1 << 20)),
        max_out,
    };
    d.run()?;
    Ok(d.out)
}

struct Decoder<'a> {
    input: &'a [u8],
    ip: usize,
    out: Vec<u8>,
    max_out: usize,
}

impl Decoder<'_> {
    #[inline]
    fn next(&mut self) -> Result<u8> {
        let b = *self.input.get(self.ip).ok_or(E)?;
        self.ip += 1;
        Ok(b)
    }

    /// Read a little-endian 16-bit operand (used by the long-distance
    /// match buckets; its low 2 bits double as the trailing-literal
    /// state, the upper 14 bits as the distance contribution).
    #[inline]
    fn next_le16(&mut self) -> Result<u16> {
        let lo = self.next()? as u16;
        let hi = self.next()? as u16;
        Ok(lo | (hi << 8))
    }

    /// Zero-run length extension: consume a run of `0x00` bytes (each
    /// worth 255) followed by one non-zero byte worth its own value.
    /// `base` is the length already accounted for by the command byte.
    /// Bounded so a corrupt all-zeros stream can't loop forever.
    fn extend_length(&mut self, base: usize) -> Result<usize> {
        let mut len = base;
        loop {
            let b = self.next()?;
            if b != 0 {
                return len.checked_add(b as usize).ok_or(E);
            }
            // Cap the run length far below any legitimate block size so
            // a corrupt stream is rejected early rather than looping.
            len = len.checked_add(ZERO_RUN_INCREMENT).ok_or(E)?;
            if len > MAX_EXTENDED_LENGTH {
                return Err(E);
            }
        }
    }

    /// Copy a back-reference of `len` bytes from `dist` bytes behind the
    /// current end of output. Overlapping copies are byte-at-a-time (the
    /// classic LZ77 self-referential run), so `dist < len` is legal and
    /// is how the format encodes byte runs.
    fn copy_match(&mut self, dist: usize, len: usize) -> Result<()> {
        if dist == 0 || dist > self.out.len() {
            return Err(E);
        }
        if self.out.len() + len > self.max_out {
            return Err(E);
        }
        let start = self.out.len() - dist;
        for src in start..start + len {
            let b = self.out[src];
            self.out.push(b);
        }
        Ok(())
    }

    /// Copy `n` literal bytes straight from the input to the output.
    fn copy_literals(&mut self, n: usize) -> Result<()> {
        if self.out.len() + n > self.max_out {
            return Err(E);
        }
        let end = self.ip.checked_add(n).ok_or(E)?;
        let slice = self.input.get(self.ip..end).ok_or(E)?;
        self.out.extend_from_slice(slice);
        self.ip = end;
        Ok(())
    }

    fn run(&mut self) -> Result<()> {
        // ---- stream bootstrap: leading literal run ----
        // The first command byte, when >= 18, is itself a literal-run
        // length of (t - 17). 17 is a reserved version marker.
        let mut state: usize;
        let first = *self.input.get(self.ip).ok_or(E)?;
        if first >= 18 {
            self.ip += 1;
            let n = (first - 17) as usize;
            self.copy_literals(n)?;
            state = n.min(4);
        } else {
            state = 0;
        }

        loop {
            // The stream is self-delimiting: a well-formed LZO1X stream
            // ends with the end-of-stream marker, decoded inside the match
            // arm below. Reaching the end of the input without a marker is
            // malformed (`next()` returns an error).
            let t = self.next()?;

            if t >= 16 {
                // Match instruction. Decode by the top set bit.
                let (len, dist, new_state) = if t >= 128 {
                    // 1 L L D D D S S — 5..=8 byte match, short distance.
                    let h = self.next()? as usize;
                    let len = 5 + ((t >> 5) & 3) as usize;
                    let dist = (h << 3) + ((t >> 2) & 7) as usize + 1;
                    (len, dist, (t & 3) as usize)
                } else if t >= 64 {
                    // 0 1 L D D D S S — 3..=4 byte match, short distance.
                    let h = self.next()? as usize;
                    let len = 3 + ((t >> 5) & 1) as usize;
                    let dist = (h << 3) + ((t >> 2) & 7) as usize + 1;
                    (len, dist, (t & 3) as usize)
                } else if t >= 32 {
                    // 0 0 1 L L L L L — match, distance 1..=16384.
                    let base = (t & 31) as usize;
                    let len = if base == 0 {
                        self.extend_length(31)? + 2
                    } else {
                        base + 2
                    };
                    let op = self.next_le16()? as usize;
                    let dist = (op >> 2) + 1;
                    (len, dist, op & 3)
                } else {
                    // 0 0 0 1 H L L L — match, distance 16384..=49151,
                    // plus the end-of-stream marker.
                    let h = ((t >> 3) & 1) as usize;
                    let base = (t & 7) as usize;
                    let len = if base == 0 {
                        self.extend_length(7)? + 2
                    } else {
                        base + 2
                    };
                    let op = self.next_le16()? as usize;
                    let dist = LONG_MATCH_DISTANCE_BASE + (h << 14) + (op >> 2);
                    // End of stream, and the test is deliberately looser
                    // than the canonical marker. Any token in this bucket
                    // that contributes no distance above the base — H
                    // clear and the operand's upper 14 bits zero — ends
                    // the stream, whatever its length bits hold.
                    // `11 00 00` is the spelling encoders emit, but
                    // `13 00 00` and `17 00 00` terminate identically,
                    // which is what the grammar asks for: the distance is
                    // what discriminates a marker from a match, and a
                    // length is meaningless on a token that copies nothing.
                    //
                    // Do not "tighten" this to also require zero length
                    // bits. That would make the decoder stricter than the
                    // format and reject streams a conforming encoder may
                    // legitimately produce, and the two filesystems that
                    // decode through here — btrfs extents and SquashFS
                    // blocks — would fail to read rather than fail loudly.
                    // The test `end_of_stream_marker_ignores_its_length_bits`
                    // below pins the accepted set.
                    //
                    // `len` is computed above and discarded on this path,
                    // which is order rather than oversight: when the
                    // length field is zero its extension bytes sit *ahead
                    // of* the operand in the stream, so the length has to
                    // be decoded before the operand can be located at all.
                    if op >> 2 == 0 && h == 0 {
                        return Ok(());
                    }
                    (len, dist, op & 3)
                };

                self.copy_match(dist, len)?;
                state = new_state;
            } else {
                // t < 16: the `0 0 0 0 ...` bucket, three-way by `state`
                // (the number of literals the PREVIOUS instruction left
                // pending, where 4 is the sentinel meaning "a long literal
                // run just happened").
                if state == 0 {
                    // Long literal run. t==0 triggers the zero-run
                    // extension; otherwise the run is t + 3 bytes long.
                    // Afterwards state = 4 (the sentinel).
                    let n = if t == 0 {
                        self.extend_length(15)? + 3
                    } else {
                        t as usize + 3
                    };
                    self.copy_literals(n)?;
                    state = 4;
                    continue;
                }
                // Both remaining cases are a short match whose command byte
                // is `0 0 0 0 D D S S`: DD = (t >> 2) & 3 contributes to the
                // distance, SS = t & 3 becomes the next trailing-literal
                // state. One more byte H extends the distance.
                //   state 1..=3 -> a 2-byte match,  dist = (H<<2)+DD+1
                //   state 4     -> a 3-byte match,  dist = (H<<2)+DD+2049
                let h = self.next()? as usize;
                let dd = ((t >> 2) & 3) as usize;
                let (len, dist) = if state == 4 {
                    (3, (h << 2) + dd + POST_LITERAL_MATCH_DISTANCE_BASE)
                } else {
                    (2, (h << 2) + dd + 1)
                };
                self.copy_match(dist, len)?;
                state = (t & 3) as usize;
            }

            // After a match, copy `state` trailing literals immediately.
            if state > 0 {
                self.copy_literals(state)?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-of-stream marker: command byte 0x11 with a zero LE16 operand.
    const EOS: [u8; 3] = [0x11, 0x00, 0x00];

    /// Bootstrap literal run: a first byte >= 18 emits (t - 17) literals.
    #[test]
    fn literal_only_via_bootstrap() {
        // t = 17 + 4 = 21 -> 4 literals "abcd".
        let mut stream = vec![21u8, b'a', b'b', b'c', b'd'];
        stream.extend_from_slice(&EOS);
        assert_eq!(decompress(&stream, 4).unwrap(), b"abcd");
    }

    /// Literal run through the `t < 16` bucket at state 0, where the run
    /// length is t + 3. t = 2 -> 5 literals.
    #[test]
    fn literal_run_small_bucket() {
        let mut stream = vec![2u8, b'h', b'e', b'l', b'l', b'o'];
        stream.extend_from_slice(&EOS);
        assert_eq!(decompress(&stream, 5).unwrap(), b"hello");
    }

    /// A `0 0 1 L L L L L` match repeating earlier output.
    ///
    /// Stream: literal run of 4 ("wxyz"), then a match with len = 4 and
    /// dist = 4, which replays the whole run.
    #[test]
    fn match_repeats_prior_output() {
        let mut stream = vec![1u8, b'w', b'x', b'y', b'z'];
        // t = 0x22: bucket `001`, length bits = 2 -> len = 2 + 2 = 4.
        // LE16 operand 0x000C: dist = (12 >> 2) + 1 = 4, state = 0.
        stream.extend_from_slice(&[0x22, 0x0C, 0x00]);
        stream.extend_from_slice(&EOS);
        assert_eq!(decompress(&stream, 8).unwrap(), b"wxyzwxyz");
    }

    /// Overlapping match (dist < len): the classic LZ77 byte-run encoding.
    /// dist = 1 copies the final byte forward repeatedly.
    #[test]
    fn overlapping_run() {
        let mut stream = vec![1u8, b'Q', b'R', b'S', b'T'];
        // t = 0x22 -> len 4; LE16 0x0000 -> dist = 1, state = 0.
        // "QRST" + 4 bytes copied from 1 behind = "TTTT".
        stream.extend_from_slice(&[0x22, 0x00, 0x00]);
        stream.extend_from_slice(&EOS);
        assert_eq!(decompress(&stream, 8).unwrap(), b"QRSTTTTT");
    }

    /// The zero-run length extension: t = 0 in the literal bucket means
    /// "length continues", each 0x00 byte adding 255.
    #[test]
    fn zero_run_length_extension() {
        // t = 0 -> extend_length(15); a single terminating byte of 5
        // gives 15 + 5 = 20, then +3 -> a 23-byte literal run.
        let payload: Vec<u8> = (0..23u8).collect();
        let mut stream = vec![0u8, 5u8];
        stream.extend_from_slice(&payload);
        stream.extend_from_slice(&EOS);
        assert_eq!(decompress(&stream, 23).unwrap(), payload);
    }

    /// `1 L L D D D S S` (`t >= 128`) — a 5..=8 byte match at a short
    /// distance. Together with the `64..=127` bucket below this is where
    /// most of a real LZO1X stream lives, so it is the bucket a refactor is
    /// likeliest to break and the one that most needs a fixture that runs
    /// without an external encoder installed.
    ///
    /// t = 0x8C: len = 5 + ((0x8C >> 5) & 3) = 5, and the byte after the
    /// command byte is the distance extension H, so
    /// dist = (0 << 3) + ((0x8C >> 2) & 7) + 1 = 4. New state = 0x8C & 3 = 0.
    #[test]
    fn short_distance_match_five_byte_bucket() {
        let mut stream = vec![1u8, b'w', b'x', b'y', b'z'];
        stream.extend_from_slice(&[0x8C, 0x00]);
        stream.extend_from_slice(&EOS);
        assert_eq!(decompress(&stream, 9).unwrap(), b"wxyzwxyzw");
    }

    /// `0 1 L D D D S S` (`64..=127`) — a 3..=4 byte match at a short
    /// distance. The distance and state encodings are identical to the
    /// bucket above; only the length differs.
    ///
    /// t = 0x4C: len = 3 + ((0x4C >> 5) & 1) = 3,
    /// dist = (0 << 3) + ((0x4C >> 2) & 7) + 1 = 4.
    #[test]
    fn short_distance_match_three_byte_bucket() {
        let mut stream = vec![1u8, b'w', b'x', b'y', b'z'];
        stream.extend_from_slice(&[0x4C, 0x00]);
        stream.extend_from_slice(&EOS);
        assert_eq!(decompress(&stream, 7).unwrap(), b"wxyzwxy");
    }

    /// `t < 16` with a carried state of 1..=3 — the two-byte match. Reaching
    /// it needs a preceding match whose SS bits are non-zero, because the
    /// state is what selects this arm at all.
    #[test]
    fn two_byte_match_at_trailing_literal_state() {
        let mut stream = vec![1u8, b'w', b'x', b'y', b'z'];
        // 0x22 with LE16 0x000D: len = 4, dist = 4, and SS = 13 & 3 = 1, so
        // one trailing literal ('A') is copied immediately after the match
        // and the next instruction is decoded at state 1.
        stream.extend_from_slice(&[0x22, 0x0D, 0x00, b'A']);
        // t = 0x04 at state 1: DD = (4 >> 2) & 3 = 1 and H = 0x00, so
        // dist = (0 << 2) + 1 + 1 = 2 and len = 2. New state = 4 & 3 = 0.
        stream.extend_from_slice(&[0x04, 0x00]);
        stream.extend_from_slice(&EOS);
        assert_eq!(decompress(&stream, 11).unwrap(), b"wxyzwxyzAzA");
    }

    /// `t < 16` with the state-4 sentinel — the three-byte match that can
    /// only follow a long literal run, whose distance base is
    /// [`POST_LITERAL_MATCH_DISTANCE_BASE`].
    ///
    /// The fixture is kilobytes rather than bytes for a structural reason
    /// worth recording so nobody tries to shrink it: the smallest distance
    /// this arm can encode is 2049, and `copy_match` rejects a distance
    /// larger than the output produced so far, so the token is unreachable
    /// until ~2 KiB has already been emitted.
    #[test]
    fn three_byte_match_after_long_literal_run() {
        // Four bootstrap literals, then one overlapping match at dist 4 that
        // replays "wxyz" until the output is 2080 bytes long.
        let mut stream = vec![21u8, b'w', b'x', b'y', b'z'];
        // t = 0x20: the length field is zero, so the length comes from a
        // zero-run extension of base 31. Eight 0x00 bytes add 255 each and
        // the terminating 0x03 adds 3: 31 + 2040 + 3 = 2074, + 2 = 2076.
        // LE16 0x000C then gives dist = 4 and state = 0.
        stream.push(0x20);
        stream.extend_from_slice(&[0u8; 8]);
        stream.push(0x03);
        stream.extend_from_slice(&[0x0C, 0x00]);
        // A six-byte literal run, which is what arms the state-4 sentinel.
        stream.push(3u8);
        stream.extend_from_slice(b"ABCDEF");
        // t = 0x04 at state 4: DD = 1 and H = 0x01, so
        // dist = (1 << 2) + 1 + 2049 = 2054 and len = 3.
        stream.extend_from_slice(&[0x04, 0x01]);
        stream.extend_from_slice(&EOS);

        let mut expected = b"wxyz".repeat(520);
        expected.extend_from_slice(b"ABCDEF");
        // 2086 - 2054 = 32, a whole number of "wxyz" periods from the start.
        expected.extend_from_slice(b"wxy");
        assert_eq!(decompress(&stream, expected.len()).unwrap(), expected);
    }

    /// `0 0 0 1 H L L L` decoding as a match rather than as the end-of-stream
    /// marker. Structurally expensive for the same reason as the test above,
    /// one window larger: [`LONG_MATCH_DISTANCE_BASE`] is 16384, so nothing
    /// in this bucket can copy anything until 16 KiB of output exists.
    #[test]
    fn long_distance_match_is_not_the_end_of_stream_marker() {
        let mut stream = vec![21u8, b'w', b'x', b'y', b'z'];
        // Sixty-four 0x00 bytes plus a terminating 0x1F extend the length to
        // 31 + 16320 + 31 = 16382, + 2 = 16384, at dist 4: output 16388 bytes.
        stream.push(0x20);
        stream.extend_from_slice(&[0u8; 64]);
        stream.push(0x1F);
        stream.extend_from_slice(&[0x0C, 0x00]);
        // t = 0x13: H = 0 and LLL = 3, so len = 5. LE16 0x0005 gives a
        // distance contribution of 1 -> dist = 16385, and SS = 1, so a
        // single trailing literal ('Q') follows the match.
        stream.extend_from_slice(&[0x13, 0x05, 0x00, b'Q']);
        stream.extend_from_slice(&EOS);

        let mut expected = b"wxyz".repeat(4097);
        // 16388 - 16385 = 3, so the copy starts on the 'z' of the first period.
        expected.extend_from_slice(b"zwxyz");
        expected.push(b'Q');
        assert_eq!(decompress(&stream, expected.len()).unwrap(), expected);
    }

    /// The marker is keyed on the distance contribution alone. Every command
    /// byte in `0x11..=0x17` paired with a zero operand ends the stream, not
    /// just the canonical `11 00 00`, because the length bits play no part in
    /// the test. This is here to stop the check being "tightened" to also
    /// require zero length bits — that would reject streams the format
    /// permits, and both callers of this crate decode through this bucket.
    #[test]
    fn end_of_stream_marker_ignores_its_length_bits() {
        for t in 0x11u8..=0x17 {
            let stream = [21u8, b'a', b'b', b'c', b'd', t, 0x00, 0x00];
            assert_eq!(
                decompress(&stream, 4),
                Ok(b"abcd".to_vec()),
                "command byte {t:#04x} contributes no distance and must terminate",
            );
        }
    }

    /// The other half of that rule: a non-zero distance contribution is a
    /// match no matter how marker-shaped the rest of the token looks. 0x19
    /// sets H, so `19 00 00` asks for a match 32768 bytes back and fails for
    /// want of history rather than ending the stream.
    #[test]
    fn end_of_stream_bucket_with_h_set_is_a_match() {
        let stream = [21u8, b'a', b'b', b'c', b'd', 0x19, 0x00, 0x00];
        assert_eq!(decompress(&stream, 4), Err(Error::Malformed));
    }

    #[test]
    fn truncated_stream_errors() {
        // Claims a 4-byte literal run but supplies only 2 bytes.
        assert_eq!(decompress(&[21u8, b'a', b'b'], 4), Err(Error::Malformed));
    }

    #[test]
    fn missing_eos_marker_errors() {
        // Well-formed literal run, but the stream simply stops.
        assert_eq!(
            decompress(&[2u8, b'h', b'e', b'l', b'l', b'o'], 5),
            Err(Error::Malformed)
        );
    }

    #[test]
    fn match_before_any_output_errors() {
        // A match as the first token has no prior output to copy from.
        assert_eq!(decompress(&[0x22u8, 0x00, 0x00], 4), Err(Error::Malformed));
    }

    #[test]
    fn output_exceeding_max_out_errors() {
        // A 5-byte literal run against a 3-byte ceiling.
        let mut stream = vec![2u8, b'h', b'e', b'l', b'l', b'o'];
        stream.extend_from_slice(&EOS);
        assert_eq!(decompress(&stream, 3), Err(Error::Malformed));
    }

    #[test]
    fn empty_input_errors() {
        assert_eq!(decompress(&[], 16), Err(Error::Malformed));
    }

    /// An all-zeros stream must terminate via the zero-run cap rather
    /// than spinning forever.
    #[test]
    fn all_zeros_terminates() {
        let stream = vec![0u8; 4096];
        assert_eq!(decompress(&stream, 1 << 16), Err(Error::Malformed));
    }
}
