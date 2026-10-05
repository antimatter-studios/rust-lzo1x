//! The `.lzo` file: a header, LZO1X blocks of up to 256 KiB each with its
//! sizes and checksums, and an end marker (#39).
//!
//! This is the container the reference compressor writes, so a `.lzo`
//! file made by either tool is read by the other. It was established by
//! reading files that tool wrote, byte by byte, and is implemented from
//! that and nothing else: no code, structure or naming of the reference
//! tool is used here.
//!
//! # Layout
//!
//! All integers are big-endian.
//!
//! ```text
//! magic          9 bytes   89 4c 5a 4f 00 0d 0a 1a 0a
//! version        u16       the format version that wrote it (0x1040)
//! lib_version    u16       informational
//! version_needed u16       the oldest reader that can read it   (version >= 0x0940)
//! method         u8        1, 2 or 3: an LZO1X variant
//! level          u8        the compression level asked for      (version >= 0x0940)
//! flags          u32       which checksums are present, and the OS
//! filter         u32       only when flags has FILTER
//! mode           u32       the file's mode
//! mtime          u32 + u32 seconds, low then high                (high: version >= 0x0940)
//! name           u8 + n    the file's name, without its directory
//! checksum       u32       Adler-32 (or CRC-32 with H_CRC32) of everything from `version`
//! extra field    u32 + n + u32, only when flags has EXTRA_FIELD
//!
//! then for each block:
//! uncompressed   u32       0 ends the file
//! compressed     u32       equal to `uncompressed` when the block is stored as it was
//! d-checksum     u32       of the uncompressed bytes, when flags asks for one
//! c-checksum     u32       of the compressed bytes, when flags asks and the block was compressed
//! data           `compressed` bytes
//! ```

use std::fmt;
use std::io::{self, Read, Write};

/// The first nine bytes of every `.lzo` file.
pub const MAGIC: [u8; 9] = [0x89, b'L', b'Z', b'O', 0x00, 0x0d, 0x0a, 0x1a, 0x0a];
/// The uncompressed size of every block but the last.
pub const BLOCK_SIZE: usize = 256 * 1024;
/// The largest block a reader accepts: the most any writer produces.
const MAX_BLOCK: usize = 64 * 1024 * 1024;

/// The format version written, and the oldest a reader needs.
const VERSION: u16 = 0x1040;
const LIB_VERSION: u16 = 0x20a0;
const VERSION_NEEDED: u16 = 0x0940;
/// Newer than this, a file may use something this reader does not know.
const NEWEST_READABLE: u16 = 0x1040;

/// `flags` bits.
pub mod flags {
    /// Each block carries the Adler-32 of its uncompressed bytes.
    pub const ADLER32_D: u32 = 0x0000_0001;
    /// Each compressed block carries the Adler-32 of its compressed bytes.
    pub const ADLER32_C: u32 = 0x0000_0002;
    /// The file was written to standard output.
    pub const STDOUT: u32 = 0x0000_0008;
    /// An extra field follows the header.
    pub const EXTRA_FIELD: u32 = 0x0000_0040;
    /// Each block carries the CRC-32 of its uncompressed bytes.
    pub const CRC32_D: u32 = 0x0000_0100;
    /// Each compressed block carries the CRC-32 of its compressed bytes.
    pub const CRC32_C: u32 = 0x0000_0200;
    /// The file is one part of several; not supported.
    pub const MULTIPART: u32 = 0x0000_0400;
    /// A filter value follows the flags.
    pub const FILTER: u32 = 0x0000_0800;
    /// The header's checksum is CRC-32 rather than Adler-32.
    pub const H_CRC32: u32 = 0x0000_1000;
    /// The operating system field: Unix.
    pub const OS_UNIX: u32 = 0x0300_0000;
}

/// The `method` this crate writes: LZO1X. The three LZO1X variants (1, 2,
/// 3) share one stream grammar, so all three are read.
pub const METHOD_LZO1X_1: u8 = 1;
const METHODS: [u8; 3] = [1, 2, 3];

/// A file's header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// The format version that wrote it.
    pub version: u16,
    /// Which LZO1X variant compressed it: 1, 2 or 3.
    pub method: u8,
    /// The compression level asked for; informational.
    pub level: u8,
    /// Which checksums the blocks carry, and the [`flags`] beside them.
    pub flags: u32,
    /// The file's mode, as `stat` gives it.
    pub mode: u32,
    /// Seconds since the epoch.
    pub mtime: u64,
    /// The file's name, without its directory. Empty for a stream.
    pub name: Vec<u8>,
}

impl Header {
    /// A header for a new file, with the checksum the reference compressor
    /// writes by default (Adler-32 of each block's uncompressed bytes).
    pub fn new(name: &[u8], mode: u32, mtime: u64, level: u8) -> Header {
        Header {
            version: VERSION,
            method: METHOD_LZO1X_1,
            level,
            flags: flags::ADLER32_D | flags::OS_UNIX,
            mode,
            mtime,
            name: name[..name.len().min(255)].to_vec(),
        }
    }
}

/// What reading or writing a `.lzo` file can run into.
#[derive(Debug)]
pub enum FileError {
    /// The reader or writer failed.
    Io(io::Error),
    /// The first nine bytes are not the `.lzo` magic.
    NotLzo,
    /// The file ends inside a structure.
    Truncated,
    /// The header is malformed or uses something this reader does not know.
    Header(String),
    /// A checksum does not match the bytes it covers.
    Checksum {
        /// Which: the header, or block N's compressed or uncompressed bytes.
        what: String,
    },
    /// A block's LZO1X stream does not decode to the size it declares.
    Block {
        /// Which block, from 0.
        index: u64,
        /// What is wrong with it.
        reason: String,
    },
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileError::Io(e) => write!(f, "{e}"),
            FileError::NotLzo => f.write_str("not an .lzo file (no lzop magic at its start)"),
            FileError::Truncated => f.write_str("the file ends early: it is truncated"),
            FileError::Header(m) => write!(f, "the header: {m}"),
            FileError::Checksum { what } => {
                write!(f, "checksum mismatch: {what}; the file is damaged")
            }
            FileError::Block { index, reason } => write!(f, "block {index}: {reason}"),
        }
    }
}

impl std::error::Error for FileError {}

impl From<io::Error> for FileError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            FileError::Truncated
        } else {
            FileError::Io(e)
        }
    }
}

/// Totals over a file's blocks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    /// Bytes the blocks decompress to.
    pub uncompressed: u64,
    /// Bytes the blocks occupy.
    pub compressed: u64,
    /// How many blocks.
    pub blocks: u64,
}

/// Adler-32 of `data`, continuing from `adler` (1 to start).
pub fn adler32(adler: u32, data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (adler & 0xffff, adler >> 16);
    // 5552 bytes is the most that can be summed before `b` could overflow.
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// CRC-32 (IEEE, the one zlib uses) of `data`, continuing from `crc`
/// (0 to start).
pub fn crc32(crc: u32, data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    });
    let mut c = !crc;
    for &byte in data {
        c = table[((c ^ u32::from(byte)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

fn put16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

/// Write `header`.
pub fn write_header<W: Write>(out: &mut W, header: &Header) -> io::Result<()> {
    let mut h = Vec::new();
    put16(&mut h, header.version);
    put16(&mut h, LIB_VERSION);
    put16(
        &mut h,
        if header.flags & flags::H_CRC32 != 0 {
            0x1001
        } else {
            VERSION_NEEDED
        },
    );
    h.push(header.method);
    h.push(header.level);
    put32(&mut h, header.flags);
    put32(&mut h, header.mode);
    put32(&mut h, header.mtime as u32);
    put32(&mut h, (header.mtime >> 32) as u32);
    let name = &header.name[..header.name.len().min(255)];
    h.push(name.len() as u8);
    h.extend_from_slice(name);
    let sum = if header.flags & flags::H_CRC32 != 0 {
        crc32(0, &h)
    } else {
        adler32(1, &h)
    };
    out.write_all(&MAGIC)?;
    out.write_all(&h)?;
    out.write_all(&sum.to_be_bytes())
}

/// A reader that keeps every byte it hands out, so the header's checksum
/// can be computed over exactly what was read.
struct Recording<'a, R> {
    inner: &'a mut R,
    seen: Vec<u8>,
}

impl<R: Read> Recording<'_, R> {
    fn take(&mut self, n: usize) -> Result<Vec<u8>, FileError> {
        let mut buf = vec![0u8; n];
        self.inner.read_exact(&mut buf)?;
        self.seen.extend_from_slice(&buf);
        Ok(buf)
    }
    fn u8(&mut self) -> Result<u8, FileError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, FileError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, FileError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
}

fn read_u32<R: Read>(input: &mut R) -> Result<u32, FileError> {
    let mut b = [0u8; 4];
    input.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

/// Read and check a header.
pub fn read_header<R: Read>(input: &mut R) -> Result<Header, FileError> {
    let mut magic = [0u8; 9];
    match input.read_exact(&mut magic) {
        Ok(()) if magic == MAGIC => {}
        Ok(()) => return Err(FileError::NotLzo),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(FileError::NotLzo),
        Err(e) => return Err(FileError::Io(e)),
    }
    let mut r = Recording {
        inner: input,
        seen: Vec::new(),
    };
    let version = r.u16()?;
    let _lib_version = r.u16()?;
    let newer = version >= 0x0940;
    if newer {
        let needed = r.u16()?;
        if needed > NEWEST_READABLE {
            return Err(FileError::Header(format!(
                "it needs a reader of format version {needed:#06x}, newer than this one"
            )));
        }
    }
    let method = r.u8()?;
    if !METHODS.contains(&method) {
        return Err(FileError::Header(format!(
            "method {method} is not one of the LZO1X variants (1, 2, 3)"
        )));
    }
    let level = if newer { r.u8()? } else { 0 };
    let flags = r.u32()?;
    if flags & flags::MULTIPART != 0 {
        return Err(FileError::Header(
            "a multi-part file is not supported".into(),
        ));
    }
    if flags & flags::FILTER != 0 {
        let _filter = r.u32()?;
    }
    let mode = r.u32()?;
    let low = r.u32()?;
    let high = if newer { r.u32()? } else { 0 };
    let name_len = usize::from(r.u8()?);
    let name = r.take(name_len)?;
    let seen = std::mem::take(&mut r.seen);
    let stored = read_u32(r.inner)?;
    let computed = if flags & flags::H_CRC32 != 0 {
        crc32(0, &seen)
    } else {
        adler32(1, &seen)
    };
    if stored != computed {
        return Err(FileError::Checksum {
            what: "the header".into(),
        });
    }
    if flags & flags::EXTRA_FIELD != 0 {
        let len = read_u32(r.inner)? as usize;
        let mut extra = vec![0u8; len.min(MAX_BLOCK)];
        r.inner.read_exact(&mut extra)?;
        let _sum = read_u32(r.inner)?;
    }
    Ok(Header {
        version,
        method,
        level,
        flags,
        mode,
        mtime: u64::from(low) | (u64::from(high) << 32),
        name,
    })
}

/// The checksum `flags` asks for over a block's uncompressed bytes, if any.
fn d_sum(flags: u32, data: &[u8]) -> Option<u32> {
    if flags & flags::ADLER32_D != 0 {
        Some(adler32(1, data))
    } else if flags & flags::CRC32_D != 0 {
        Some(crc32(0, data))
    } else {
        None
    }
}

/// The checksum `flags` asks for over a block's compressed bytes, if any.
fn c_sum(flags: u32, data: &[u8]) -> Option<u32> {
    if flags & flags::ADLER32_C != 0 {
        Some(adler32(1, data))
    } else if flags & flags::CRC32_C != 0 {
        Some(crc32(0, data))
    } else {
        None
    }
}

/// Compress everything `input` holds into a `.lzo` file on `out`.
pub fn compress<R: Read, W: Write>(
    input: &mut R,
    out: &mut W,
    header: &Header,
) -> Result<Totals, FileError> {
    write_header(out, header)?;
    let mut totals = Totals::default();
    let mut block = vec![0u8; BLOCK_SIZE];
    loop {
        let n = fill(input, &mut block)?;
        if n == 0 {
            break;
        }
        let data = &block[..n];
        let packed = crate::compress(data);
        // A block that did not shrink is stored as it was.
        let payload: &[u8] = if packed.len() < n { &packed } else { data };
        out.write_all(&(n as u32).to_be_bytes())?;
        out.write_all(&(payload.len() as u32).to_be_bytes())?;
        if let Some(sum) = d_sum(header.flags, data) {
            out.write_all(&sum.to_be_bytes())?;
        }
        if payload.len() < n {
            if let Some(sum) = c_sum(header.flags, payload) {
                out.write_all(&sum.to_be_bytes())?;
            }
        }
        out.write_all(payload)?;
        totals.uncompressed += n as u64;
        totals.compressed += payload.len() as u64;
        totals.blocks += 1;
        if n < block.len() {
            break;
        }
    }
    out.write_all(&0u32.to_be_bytes())?;
    out.flush()?;
    Ok(totals)
}

/// Read until `buf` is full or the input ends; how many bytes were read.
fn fill<R: Read>(input: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match input.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Read a `.lzo` file's blocks after its header, check every checksum and
/// size, and write what they decompress to on `out`.
///
/// Pass [`io::sink`] to test a file without keeping its contents.
pub fn decompress<R: Read, W: Write>(
    input: &mut R,
    out: &mut W,
    header: &Header,
) -> Result<Totals, FileError> {
    blocks(input, header, |index, data| {
        out.write_all(data).map_err(FileError::from)?;
        let _ = index;
        Ok(())
    })
}

/// The totals a file's block headers declare, without decompressing: what
/// a listing shows.
pub fn scan<R: Read>(input: &mut R, header: &Header) -> Result<Totals, FileError> {
    let mut totals = Totals::default();
    loop {
        let dst = read_u32(input)? as usize;
        if dst == 0 {
            return Ok(totals);
        }
        let src = read_u32(input)? as usize;
        check_sizes(totals.blocks, dst, src)?;
        let mut skip = src as u64;
        if d_sum(header.flags, &[]).is_some() {
            skip += 4;
        }
        if src < dst && c_sum(header.flags, &[]).is_some() {
            skip += 4;
        }
        let copied = io::copy(&mut input.take(skip), &mut io::sink())?;
        if copied != skip {
            return Err(FileError::Truncated);
        }
        totals.uncompressed += dst as u64;
        totals.compressed += src as u64;
        totals.blocks += 1;
    }
}

fn check_sizes(index: u64, dst: usize, src: usize) -> Result<(), FileError> {
    if dst > MAX_BLOCK {
        return Err(FileError::Block {
            index,
            reason: format!("it declares {dst} bytes, more than any block holds"),
        });
    }
    if src == 0 || src > dst {
        return Err(FileError::Block {
            index,
            reason: format!("{src} compressed bytes for {dst} uncompressed is not a block"),
        });
    }
    Ok(())
}

fn blocks<R: Read, F>(input: &mut R, header: &Header, mut each: F) -> Result<Totals, FileError>
where
    F: FnMut(u64, &[u8]) -> Result<(), FileError>,
{
    let mut totals = Totals::default();
    loop {
        let index = totals.blocks;
        let dst = read_u32(input)? as usize;
        if dst == 0 {
            return Ok(totals);
        }
        let src = read_u32(input)? as usize;
        check_sizes(index, dst, src)?;
        let want_d = if d_sum(header.flags, &[]).is_some() {
            Some(read_u32(input)?)
        } else {
            None
        };
        let want_c = if src < dst && c_sum(header.flags, &[]).is_some() {
            Some(read_u32(input)?)
        } else {
            None
        };
        let mut payload = vec![0u8; src];
        input.read_exact(&mut payload)?;
        if let Some(want) = want_c {
            if c_sum(header.flags, &payload) != Some(want) {
                return Err(FileError::Checksum {
                    what: format!("block {index}'s compressed bytes"),
                });
            }
        }
        let data = if src < dst {
            let data = crate::decompress(&payload, dst).map_err(|e| FileError::Block {
                index,
                reason: e.to_string(),
            })?;
            if data.len() != dst {
                return Err(FileError::Block {
                    index,
                    reason: format!("it decodes to {} bytes and declares {dst}", data.len()),
                });
            }
            data
        } else {
            payload
        };
        if let Some(want) = want_d {
            if d_sum(header.flags, &data) != Some(want) {
                return Err(FileError::Checksum {
                    what: format!("block {index}'s uncompressed bytes"),
                });
            }
        }
        each(index, &data)?;
        totals.uncompressed += dst as u64;
        totals.compressed += src as u64;
        totals.blocks += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values zlib computes for the same inputs, so the two checksums are
    /// the ones the format means.
    #[test]
    fn the_checksums_are_zlibs() {
        assert_eq!(adler32(1, b""), 1);
        assert_eq!(adler32(1, b"Wikipedia"), 0x11e6_0398);
        assert_eq!(crc32(0, b""), 0);
        assert_eq!(
            crc32(0, b"The quick brown fox jumps over the lazy dog"),
            0x414f_a339
        );
    }

    /// The header the reference compressor wrote for a 24-byte file, read
    /// back field by field; its checksum is the Adler-32 it carried.
    #[test]
    fn a_header_round_trips() {
        let h = Header::new(b"small.txt", 0o100644, 1_791_209_726, 5);
        let mut bytes = Vec::new();
        write_header(&mut bytes, &h).unwrap();
        let back = read_header(&mut bytes.as_slice()).unwrap();
        assert_eq!(back, h);
    }

    #[test]
    fn a_file_round_trips_and_damage_is_found() {
        let data: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
        let h = Header::new(b"x", 0o100600, 1, 5);
        let mut file = Vec::new();
        let written = compress(&mut data.as_slice(), &mut file, &h).unwrap();
        assert_eq!(written.uncompressed, data.len() as u64);
        let mut r = file.as_slice();
        let header = read_header(&mut r).unwrap();
        let mut out = Vec::new();
        decompress(&mut r, &mut out, &header).unwrap();
        assert_eq!(out, data);

        let mut damaged = file.clone();
        let at = damaged.len() - 50;
        damaged[at] ^= 1;
        let mut r = damaged.as_slice();
        let header = read_header(&mut r).unwrap();
        assert!(decompress(&mut r, &mut io::sink(), &header).is_err());

        let cut = &file[..file.len() - 10];
        let mut r = cut;
        let header = read_header(&mut r).unwrap();
        assert!(matches!(
            decompress(&mut r, &mut io::sink(), &header),
            Err(FileError::Truncated)
        ));
    }
}
