//! LZO1X encoder.
//!
//! # Where this comes from
//!
//! The grammar is not read from anyone else's source. It is the grammar
//! this crate's decoder already implements, run backwards — see the
//! instruction dispatch in `lib.rs`, where every bucket is spelled out
//! with the field layout it decodes. That makes the provenance of this
//! file the same as the decoder's: clean-room, MIT, no derivation from
//! the GPL reference implementation.
//!
//! Inverting a decoder is a much weaker obligation than writing one. A
//! decoder must accept every stream a conforming encoder can emit; an
//! encoder need only emit streams a conforming decoder accepts, and is
//! free to use a subset of the grammar. This one does exactly that.
//!
//! # Why round-tripping through our own decoder proves almost nothing
//!
//! Our decoder is deliberately more permissive than the format in at
//! least one place (see the end-of-stream comment in `lib.rs`). An
//! encoder checked only against it could emit a stream that this crate
//! reads back perfectly and the kernel refuses. The tests here are the
//! cheap first filter; the contract that actually matters is the
//! cross-validation against the reference tool, which decompresses what
//! this module produces.
//!
//! # The subset this encoder emits
//!
//! Two of the four match buckets, and both literal forms:
//!
//! | bucket | command byte  | used for                          |
//! |--------|---------------|-----------------------------------|
//! | C      | `0 0 1 LLLLL` | every match with distance ≤ 16384  |
//! | D      | `0 0 0 1 HLLL`| every match with distance > 16384  |
//!
//! The two short-distance buckets (`1 LLDDDSS` and `0 1 LDDDSS`) encode
//! a 3..8 byte match one byte more cheaply than bucket C does. Leaving
//! them out costs ratio, not correctness, and keeps the number of
//! encoding paths — each of which is a place to get the field packing
//! wrong — at two instead of four.

use crate::{LONG_MATCH_DISTANCE_BASE, ZERO_RUN_INCREMENT};

/// Shortest run this encoder will emit as a match rather than as
/// literals. Bucket C can express a 3-byte match; below that a match
/// token costs more bytes than the literals it replaces.
const MIN_MATCH: usize = 3;

/// Longest match this encoder will emit. Not a format limit — the
/// zero-run length extension has no ceiling — but the decoder rejects a
/// single extended length above `MAX_EXTENDED_LENGTH` as a corruption
/// signal, so staying well under it keeps every stream this encoder
/// produces inside what this crate will read back.
const MAX_MATCH: usize = 1 << 16;

/// Largest back-reference distance the grammar can express: bucket D
/// reaches `16384 + (1 << 14) + 16383`.
const MAX_DISTANCE: usize = LONG_MATCH_DISTANCE_BASE + (1 << 14) + 0x3FFF;

/// Largest distance bucket C can express, and therefore the boundary
/// between the two encoding paths.
const MAX_BUCKET_C_DISTANCE: usize = 1 << 14;

/// Number of trailing bytes always left as literals.
///
/// **This is insurance, not a requirement — measured, not assumed.**
/// Setting it to 0 and re-running the cross-validation gate leaves all
/// 68 checks passing, so the reference decompressor accepts a stream
/// that ends on a match. Any comment claiming otherwise would be
/// folklore; this one was, until the mutation was actually run.
///
/// It stays at 3 anyway, for a reason the gate cannot check. LZO ships
/// two decompressors: a bounds-checked one and a faster one that copies
/// in machine words and is documented as needing slack past the end of
/// the stream. The reference CLI, and therefore this gate, exercises
/// the checked one. Reserving three bytes costs a handful of bytes on
/// output nobody measures and keeps what this encoder emits inside what
/// both variants can read, rather than only the one we can test.
const MIN_TRAILING_LITERALS: usize = 3;

/// Trailing literals that fit in a match token's two `SS` bits, and so
/// cost nothing to carry.
const MAX_INLINE_LITERALS: usize = 3;

/// Literal-run length that the *first* command byte can carry directly,
/// as `length + 17`. A first byte below 18 is not a literal run at all,
/// which is what caps this at `255 - 17`.
const MAX_BOOTSTRAP_LITERALS: usize = 255 - 17;

/// Hash table size, as a power of two. 8192 entries over a 3-byte key:
/// big enough that distinct trigrams mostly land apart, small enough to
/// stay in cache. Only affects how many matches are found, never
/// whether the output is valid.
const HASH_BITS: u32 = 13;
const HASH_SIZE: usize = 1 << HASH_BITS;

/// Knuth-style multiplicative hash of the three bytes at `p`.
#[inline]
fn hash3(window: &[u8]) -> usize {
    let key = ((window[0] as u32) << 16) | ((window[1] as u32) << 8) | (window[2] as u32);
    (key.wrapping_mul(0x1E35_A7BD) >> (32 - HASH_BITS)) as usize
}

/// One emitted instruction. Building the whole sequence before encoding
/// it is what makes the trailing-literal rule expressible: a match token
/// carries the length of the literal run that FOLLOWS it, so the encoder
/// cannot emit a match until it knows what comes after.
enum Token {
    /// A back-reference, and the literal run that follows it.
    Match { len: usize, dist: usize },
    /// A literal run, given as a range into the input.
    Literals { start: usize, len: usize },
}

/// Compress `input` into an LZO1X stream.
///
/// Infallible: every input has a valid encoding, and an input with no
/// exploitable redundancy simply comes back larger than it went in —
/// there is a per-literal-run overhead the format cannot avoid. Callers
/// that care should compare lengths and store the original when this is
/// longer, which is what both filesystems using this format do.
///
/// The output always ends with a literal run and the end-of-stream
/// marker, and [`crate::decompress`] will read it back.
pub fn compress(input: &[u8]) -> Vec<u8> {
    let tokens = parse(input);
    emit(input, &tokens)
}

/// Walk the input, turning it into an alternating sequence of literal
/// runs and matches.
fn parse(input: &[u8]) -> Vec<Token> {
    let mut tokens = Vec::new();
    // Position + 1, so that zero means "nothing recorded here yet" and
    // the table needs no separate occupancy bitmap.
    let mut table = vec![0u32; HASH_SIZE];

    // Everything from here on must stay literal, so no match may start
    // at or after it and none may extend into it.
    let search_end = input.len().saturating_sub(MIN_TRAILING_LITERALS);

    let mut pos = 0;
    let mut literal_start = 0;

    while pos + MIN_MATCH <= search_end {
        let slot = hash3(&input[pos..]);
        let candidate = table[slot] as usize;
        table[slot] = (pos + 1) as u32;

        let found = candidate
            .checked_sub(1)
            .map(|c| (c, pos - c))
            .filter(|&(_, dist)| dist > 0 && dist <= MAX_DISTANCE)
            .filter(|&(c, _)| input[c..c + MIN_MATCH] == input[pos..pos + MIN_MATCH]);

        let Some((candidate, dist)) = found else {
            pos += 1;
            continue;
        };

        let len = match_length(input, candidate, pos, search_end);
        if len < MIN_MATCH {
            pos += 1;
            continue;
        }

        if pos > literal_start {
            tokens.push(Token::Literals {
                start: literal_start,
                len: pos - literal_start,
            });
        }
        tokens.push(Token::Match { len, dist });

        // Record the interior positions too, so a later match can point
        // into the middle of this one.
        for interior in pos + 1..pos + len {
            if interior + MIN_MATCH <= input.len() {
                table[hash3(&input[interior..])] = (interior + 1) as u32;
            }
        }
        pos += len;
        literal_start = pos;
    }

    if literal_start < input.len() {
        tokens.push(Token::Literals {
            start: literal_start,
            len: input.len() - literal_start,
        });
    }
    tokens
}

/// How far the bytes at `candidate` and `pos` agree, bounded so the
/// match neither runs into the reserved trailing literals nor exceeds
/// what one length field should carry.
fn match_length(input: &[u8], candidate: usize, pos: usize, search_end: usize) -> usize {
    let limit = (search_end - pos).min(MAX_MATCH);
    let mut len = 0;
    while len < limit && input[candidate + len] == input[pos + len] {
        len += 1;
    }
    len
}

/// Turn the token sequence into bytes.
fn emit(input: &[u8], tokens: &[Token]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() / 2 + 64);
    let mut index = 0;

    // The stream opens with a literal run if there is one, and its
    // encoding is not the same as a mid-stream run's: there is no
    // preceding match to have set the decoder's state, so the first
    // command byte is read as a length directly.
    if let Some(Token::Literals { start, len }) = tokens.first() {
        emit_leading_literals(&mut out, &input[*start..*start + *len]);
        index = 1;
    }

    while index < tokens.len() {
        let Token::Match { len, dist } = tokens[index] else {
            // Two literal runs cannot be adjacent — `parse` alternates —
            // so reaching here would mean the token stream is malformed.
            unreachable!("literal run not preceded by a match");
        };

        // How many literals follow decides how this match is encoded, so
        // it has to be known before the token is written.
        let following = match tokens.get(index + 1) {
            Some(Token::Literals { start, len }) => Some((*start, *len)),
            _ => None,
        };
        let (lit_start, lit_len) = following.unwrap_or((0, 0));
        let inline = if lit_len <= MAX_INLINE_LITERALS {
            lit_len
        } else {
            0
        };

        emit_match(&mut out, len, dist, inline);
        if inline > 0 {
            out.extend_from_slice(&input[lit_start..lit_start + inline]);
        } else if lit_len > 0 {
            emit_literal_run(&mut out, &input[lit_start..lit_start + lit_len]);
        }

        index += if following.is_some() { 2 } else { 1 };
    }

    emit_end_of_stream(&mut out);
    out
}

/// The stream's opening literal run.
fn emit_leading_literals(out: &mut Vec<u8>, literals: &[u8]) {
    if literals.is_empty() {
        return;
    }
    if literals.len() <= MAX_BOOTSTRAP_LITERALS {
        out.push((literals.len() + 17) as u8);
        out.extend_from_slice(literals);
    } else {
        // Too long for the first byte to carry. A first byte below 18
        // leaves the decoder in state 0, where the ordinary long
        // literal run encoding applies — so this is the same shape as a
        // mid-stream run, and reaches the same code.
        emit_literal_run(out, literals);
    }
}

/// A literal run in the `state == 0` encoding: `0 0 0 0 LLLL`, where a
/// zero length field means the length arrives as an extension.
///
/// Only valid for runs of 4 or more. Shorter runs have no encoding here
/// at all — they ride in the preceding match's `SS` bits instead.
fn emit_literal_run(out: &mut Vec<u8>, literals: &[u8]) {
    debug_assert!(
        literals.len() >= 4,
        "a run shorter than 4 has no long-form encoding; it belongs in a match's SS bits"
    );
    if literals.len() <= 18 {
        out.push((literals.len() - 3) as u8);
    } else {
        out.push(0);
        emit_length_extension(out, literals.len() - 18);
    }
    out.extend_from_slice(literals);
}

/// The zero-run length extension: `remainder` expressed as some number
/// of `0x00` bytes worth 255 each, then one non-zero byte.
///
/// `remainder` is what is left after the command byte's own contribution,
/// and must be at least 1 — the terminating byte cannot be zero, because
/// zero is what marks a continuation.
fn emit_length_extension(out: &mut Vec<u8>, remainder: usize) {
    debug_assert!(remainder >= 1, "an extension must carry at least one");
    let mut left = remainder;
    while left > ZERO_RUN_INCREMENT {
        out.push(0);
        left -= ZERO_RUN_INCREMENT;
    }
    out.push(left as u8);
}

/// A back-reference, with `inline` trailing literals promised to follow.
fn emit_match(out: &mut Vec<u8>, len: usize, dist: usize, inline: usize) {
    debug_assert!((MIN_MATCH..=MAX_MATCH).contains(&len));
    debug_assert!((1..=MAX_DISTANCE).contains(&dist));
    debug_assert!(inline <= MAX_INLINE_LITERALS);

    if dist <= MAX_BUCKET_C_DISTANCE {
        // 0 0 1 L L L L L, five length bits over a base of 2, then a
        // 14-bit distance in the operand.
        if len <= 33 {
            out.push(0x20 | (len - 2) as u8);
        } else {
            out.push(0x20);
            emit_length_extension(out, len - 33);
        }
        push_operand(out, dist - 1, inline);
    } else {
        // 0 0 0 1 H L L L. H is the 15th distance bit, lifted out of the
        // command byte; the three length bits sit over a base of 2.
        let offset = dist - LONG_MATCH_DISTANCE_BASE;
        let high_bit = ((offset >> 14) & 1) as u8;
        let command = 0x10 | (high_bit << 3);
        if len <= 9 {
            out.push(command | (len - 2) as u8);
        } else {
            out.push(command);
            emit_length_extension(out, len - 9);
        }
        push_operand(out, offset & 0x3FFF, inline);
    }
}

/// The little-endian 16-bit operand shared by both match buckets:
/// fourteen bits of distance over two bits of trailing-literal count.
fn push_operand(out: &mut Vec<u8>, distance_bits: usize, inline: usize) {
    debug_assert!(distance_bits <= 0x3FFF);
    let operand = ((distance_bits as u16) << 2) | inline as u16;
    out.extend_from_slice(&operand.to_le_bytes());
}

/// The end-of-stream marker.
///
/// A bucket-D token contributing no distance above the base. `0x11` is
/// the spelling every encoder uses; the length bits are ignored on this
/// path because a token that copies nothing has no length.
fn emit_end_of_stream(out: &mut Vec<u8>) {
    out.extend_from_slice(&[0x11, 0x00, 0x00]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decompress;

    /// The first filter, not the contract. See the module docs: our own
    /// decoder is more permissive than the format in at least one place,
    /// so agreeing with it is necessary and nowhere near sufficient. The
    /// reference tool is the oracle that matters.
    fn round_trip(input: &[u8]) {
        let packed = compress(input);
        let unpacked = decompress(&packed, input.len().max(1)).unwrap_or_else(|e| {
            panic!("compress produced a stream our own decoder rejects: {e:?}")
        });
        assert_eq!(unpacked, input, "round trip changed the bytes");
    }

    #[test]
    fn an_empty_input_round_trips() {
        round_trip(b"");
    }

    /// Shorter than the reserved trailing literals, so no match can be
    /// considered at all and the whole thing is one literal run.
    #[test]
    fn inputs_shorter_than_the_trailing_reserve_round_trip() {
        for n in 0..=MIN_TRAILING_LITERALS {
            round_trip(&vec![b'x'; n]);
        }
    }

    #[test]
    fn incompressible_input_round_trips() {
        // A linear congruential sequence: reproducible, and with no
        // repeated trigrams for the matcher to find.
        let mut state = 0x12345678u32;
        let noise: Vec<u8> = (0..4096)
            .map(|_| {
                state = state.wrapping_mul(1103515245).wrapping_add(12345);
                (state >> 16) as u8
            })
            .collect();
        round_trip(&noise);
    }

    #[test]
    fn a_long_run_of_one_byte_round_trips() {
        round_trip(&vec![b'A'; 100_000]);
    }

    /// Exercises the zero-run length extension on both the literal and
    /// the match side, at and either side of the 255 boundary where an
    /// extra continuation byte appears.
    #[test]
    fn lengths_around_the_extension_boundary_round_trip() {
        for n in [253, 254, 255, 256, 257, 509, 510, 511] {
            round_trip(&vec![b'Z'; n]);
            let mut mixed = Vec::new();
            let mut state = 0x9E3779B9u32;
            for _ in 0..n {
                state = state.wrapping_mul(1103515245).wrapping_add(12345);
                mixed.push((state >> 16) as u8);
            }
            mixed.extend_from_slice(&mixed.clone());
            round_trip(&mixed);
        }
    }

    /// The boundary between the two match buckets. A distance of exactly
    /// 16384 must go through bucket C; 16385 through bucket D. Getting
    /// the comparison wrong by one produces a bucket-D token with a zero
    /// distance contribution, which IS the end-of-stream marker — so the
    /// stream would truncate silently rather than fail.
    #[test]
    fn distances_across_the_bucket_boundary_round_trip() {
        for gap in [
            MAX_BUCKET_C_DISTANCE - 1,
            MAX_BUCKET_C_DISTANCE,
            MAX_BUCKET_C_DISTANCE + 1,
            MAX_BUCKET_C_DISTANCE + 2,
            (1 << 15) - 1,
            1 << 15,
            (1 << 15) + 1,
            MAX_DISTANCE - 1,
            MAX_DISTANCE,
        ] {
            let marker = b"MATCHME!";
            let mut data = Vec::with_capacity(gap + 64);
            data.extend_from_slice(marker);
            let mut state = 0xDEADBEEFu32;
            while data.len() < gap {
                state = state.wrapping_mul(1103515245).wrapping_add(12345);
                data.push((state >> 16) as u8);
            }
            data.truncate(gap);
            data.extend_from_slice(marker);
            data.extend_from_slice(b"tail");
            round_trip(&data);
        }
    }

    /// Literal runs of 1..3 ride in the preceding match's `SS` bits, and
    /// runs of 4 or more take the long form. Both paths, and the
    /// boundary between them, in one input.
    #[test]
    fn literal_runs_on_both_sides_of_the_inline_limit_round_trip() {
        for gap_len in 0..8 {
            let block = b"REPEATED-BLOCK-OF-BYTES";
            let mut data = Vec::new();
            for filler in 0..6u8 {
                data.extend_from_slice(block);
                data.extend(std::iter::repeat_n(b'0' + filler, gap_len));
            }
            data.extend_from_slice(b"end");
            round_trip(&data);
        }
    }

    /// Every output must end with the marker, whatever the input.
    #[test]
    fn every_stream_ends_with_the_marker() {
        for input in [b"".as_slice(), b"a", b"abc", &[b'q'; 5000]] {
            let packed = compress(input);
            assert_eq!(
                &packed[packed.len() - 3..],
                &[0x11, 0x00, 0x00],
                "stream does not end with the end-of-stream marker"
            );
        }
    }

    /// Compressible input must actually compress. Not a correctness
    /// property, but an encoder that emits everything as literals would
    /// pass every test above.
    #[test]
    fn repetitive_input_gets_smaller() {
        let input = b"the quick brown fox jumps over the lazy dog. ".repeat(200);
        let packed = compress(&input);
        assert!(
            packed.len() * 4 < input.len(),
            "expected at least 4x on repetitive input, got {} -> {}",
            input.len(),
            packed.len()
        );
    }
}
