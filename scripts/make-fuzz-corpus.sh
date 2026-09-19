#!/usr/bin/env bash
# Rebuild fuzz/corpus from streams the reference encoder produced.
#
# The seeds are not random bytes and they are not this crate's own
# output. They are LZO1X streams emitted by `lzop` -- the reference
# implementation this crate is checked against in tests/oracle_lzop.rs
# -- lifted out of its container, which is where a mutation has
# somewhere interesting to land. A random byte string is refused by the
# first token and never reaches the match-copy loop that matters.
#
# Payloads are chosen for the shapes that produce different tokens: long
# literal runs, long matches, short matches at short distances, and
# incompressible data, which the encoder stores rather than compresses.
#
# Usage: scripts/make-fuzz-corpus.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d "${TMPDIR:-/tmp}/lzo1x-fuzz-corpus.XXXXXX")"
trap 'rm -rf "$work"' EXIT

command -v lzop >/dev/null || { echo "lzop is not on PATH" >&2; exit 1; }

# One payload per token shape the encoder can emit.
#
# 64 KiB each, not more. LZO1X-1's longest back-reference distance is
# 49151 bytes, so 64 KiB already reaches every distance bucket the
# format has -- and the gate decodes each of these tens of thousands of
# times, where a larger payload buys no new token shape and a great deal
# of wall clock.
python3 - "$work" <<'PY'
import os, sys, random
work = sys.argv[1]
random.seed(20260919)  # fixed, so the corpus is the same everywhere

def write(name, data):
    with open(os.path.join(work, name), 'wb') as f:
        f.write(data)

# Long matches at a short distance: the run-length path.
write('zeros', b'\x00' * 65_536)
# Long literal runs with nothing to match: the literal-length escape.
write('random', bytes(random.getrandbits(8) for _ in range(65_536)))
# Ordinary text: a mixture of short matches and short literals.
write('text', (b'the quick brown fox jumps over the lazy dog. ' * 1400))
# Matches at exactly the distance-bucket boundaries.
write('periodic', bytes(i % 251 for i in range(65_536)))
# A short input, where the trailing-literal handling is all there is.
write('tiny', b'aaaaaaaabbbbbbbbaaaaaaaa')
# Highly repetitive with a long period: long distances.
write('blocks', (bytes(random.getrandbits(8) for _ in range(4096)) * 16))
PY

rm -rf "$here/fuzz/corpus"
mkdir -p "$here/fuzz/corpus/decompress" "$here/fuzz/corpus/roundtrip"

# The round-trip target's input is a payload to be compressed, not a
# compressed stream, so it gets its own seeds: the same payloads, cut to
# 8 KiB. Every token shape is reachable in that much, and these are
# committed to the repository.
python3 - "$work" "$here/fuzz/corpus/roundtrip" <<'PY'
import os, sys
work, outdir = sys.argv[1], sys.argv[2]
for name in sorted(os.listdir(work)):
    src = os.path.join(work, name)
    if not os.path.isfile(src) or name.endswith('.lzo'):
        continue
    with open(src, 'rb') as f:
        data = f.read(8192)
    with open(os.path.join(outdir, name + '.bin'), 'wb') as f:
        f.write(data)
PY

for payload in "$work"/*; do
    name="$(basename "$payload")"
    for level in 1 6 9; do
        lzop -"$level" -f -o "$work/$name.$level.lzo" "$payload"
        python3 - "$work/$name.$level.lzo" "$here/fuzz/corpus/decompress" "$name-level$level" <<'PY'
import struct, sys, os

path, outdir, stem = sys.argv[1], sys.argv[2], sys.argv[3]
data = open(path, 'rb').read()

MAGIC = bytes([0x89, 0x4c, 0x5a, 0x4f, 0x00, 0x0d, 0x0a, 0x1a, 0x0a])
assert data[:9] == MAGIC, "not an lzop file"

F_H_FILTER = 0x00000800
F_ADLER32_D = 0x00000001
F_ADLER32_C = 0x00000002
F_CRC32_D = 0x00000100
F_CRC32_C = 0x00000200

i = 9
def u8():
    global i
    v = data[i]; i += 1; return v
def u16():
    global i
    v = struct.unpack_from('>H', data, i)[0]; i += 2; return v
def u32():
    global i
    v = struct.unpack_from('>I', data, i)[0]; i += 4; return v

version = u16()
u16()                      # library version
if version >= 0x0940:
    u16()                  # version needed to extract
u8()                       # method
if version >= 0x0940:
    u8()                   # level
flags = u32()
if flags & F_H_FILTER:
    u32()                  # filter
u32()                      # mode
u32()                      # mtime low
if version >= 0x0940:
    u32()                  # mtime high
fname_len = u8()           # file name
i += fname_len
u32()                      # header checksum

d_csum = flags & (F_ADLER32_D | F_CRC32_D) != 0
c_csum = flags & (F_ADLER32_C | F_CRC32_C) != 0

kept = 0
while True:
    uncompressed_len = u32()
    if uncompressed_len == 0:
        break
    compressed_len = u32()
    if d_csum:
        u32()
    # A block the encoder could not shrink is STORED -- the payload is
    # the original bytes, not an LZO1X stream -- and carries no
    # compressed-side checksum. Asking once keeps the two answers from
    # disagreeing and desyncing the cursor.
    stored = compressed_len >= uncompressed_len
    if c_csum and not stored:
        u32()
    payload = data[i:i + compressed_len]
    i += compressed_len
    if stored:
        continue           # not an LZO1X stream; nothing to seed with
    # The decode length travels with the stream: `decompress` takes a
    # `max_out`, and a seed is only useful if something knows what this
    # one decodes to. Four bytes, little-endian, stripped back off by
    # the harness.
    out = os.path.join(outdir, f"{stem}-block{kept}.bin")
    with open(out, 'wb') as f:
        f.write(struct.pack('<I', uncompressed_len))
        f.write(payload)
    kept += 1
PY
    done
done

echo "corpus rebuilt under fuzz/corpus:"
find "$here/fuzz/corpus" -type f | sort | sed "s#$here/##"
echo "total: $(find "$here/fuzz/corpus" -type f | wc -l) seeds"
