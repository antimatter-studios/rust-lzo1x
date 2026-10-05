//! `rust-lzo1x`: the command-line tool, one multi-call binary.
//!
//! Installed as `rust-lzo1x` and linked as `lzo1x`. The dispatch, the
//! `--version`, `doctor`, the output contract, the man pages and the
//! completions are `fs_core::cli` (am-fs-core's `cli` feature), the same
//! plumbing every tool in the family uses; this file is only the tool.
//!
//! `lzo1x` handles `.lzo` files the way `gzip` handles `.gz` files (#39):
//! `lzo1x FILE` makes `FILE.lzo` and removes `FILE`, `-d` reverses it, `-k`
//! keeps the input, `-c` writes to standard output, and with no file it
//! filters standard input to standard output. The files are the ones the
//! reference compressor reads and writes ([`lzo1x::lzop`]).
//!
//! `--raw` is the other mode: one raw LZO1X block, no container, which is
//! what a btrfs extent or a SquashFS block holds. The format does not
//! record a raw block's size, so decompressing one takes `--size`.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{value_parser, Arg, ArgAction, ArgMatches, Command as Cmd};
use fs_core::cli::{self, CliError, Json, Outcome, Tool};
use lzo1x::lzop::{self, FileError, Header};

static FAMILY: cli::Family = cli::Family {
    repo: "rust-lzo1x",
    crate_name: env!("CARGO_PKG_NAME"),
    version: env!("CARGO_PKG_VERSION"),
    about: "LZO1X tools: .lzo files and raw LZO1X blocks",
    install_hints: &[
        "`cargo install am-lzo1x --features cli` from crates.io",
        "`brew install antimatter-studios/tap/rust-lzo1x`",
    ],
    tools: &[TOOL],
};

const TOOL: Tool = Tool {
    name: "lzo1x",
    verb: "lzo1x",
    section: 1,
    usage_exit: fs_core::cli::output::EXIT_USAGE,
    about: "Compress and decompress .lzo files and raw LZO1X blocks",
    command,
    run,
};

const SUFFIX: &str = ".lzo";

fn flag(id: &'static str, short: char, long: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .short(short)
        .long(long)
        .help(help)
        .action(ArgAction::SetTrue)
}

fn command() -> Cmd {
    let mut cmd = Cmd::new("lzo1x")
        .about("Compress and decompress .lzo files and raw LZO1X blocks")
        .long_about(
            "Compress and decompress .lzo files, the way gzip handles .gz files: \
             `lzo1x FILE` makes FILE.lzo and removes FILE, `lzo1x -d FILE.lzo` gives FILE \
             back with its mode and modification time, -k keeps the input, -c writes to \
             standard output, and with no FILE it filters standard input to standard \
             output. The files are the ones lzop reads and writes.\n\n\
             --raw works on one raw LZO1X block instead: no header, no checksums, which is \
             what a btrfs extent or a SquashFS block holds. Its size is not in the stream, \
             so --raw -d needs --size.",
        )
        .arg(
            Arg::new("files")
                .value_name("FILE")
                .num_args(0..)
                .value_parser(value_parser!(OsString)),
        )
        .arg(flag("decompress", 'd', "decompress", "Decompress"))
        .arg(flag(
            "stdout",
            'c',
            "stdout",
            "Write to standard output; keep the input",
        ))
        .arg(flag("keep", 'k', "keep", "Keep the input file"))
        .arg(flag(
            "force",
            'f',
            "force",
            "Overwrite an existing output file",
        ))
        .arg(flag(
            "test",
            't',
            "test",
            "Check an .lzo file's checksums and blocks; write nothing",
        ))
        .arg(flag(
            "list",
            'l',
            "list",
            "List an .lzo file's name and sizes",
        ))
        .arg(
            Arg::new("output")
                .short('o')
                .long("output")
                .value_name("FILE")
                .help("Write to FILE (one input only)")
                .value_parser(value_parser!(OsString)),
        )
        .arg(
            Arg::new("raw")
                .long("raw")
                .help("One raw LZO1X block: no container")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("size")
                .long("size")
                .value_name("BYTES")
                .help("With --raw -d: how many bytes the block decompresses to, at most")
                .value_parser(value_parser!(u64)),
        );
    for level in 1..=9u8 {
        let id: &'static str =
            ["1", "2", "3", "4", "5", "6", "7", "8", "9"][usize::from(level - 1)];
        cmd = cmd.arg(
            Arg::new(id)
                .short(char::from(b'0' + level))
                .help(if level == 1 {
                    "Compression level, recorded in the file (-1 to -9)"
                } else {
                    ""
                })
                .hide(level != 1)
                .action(ArgAction::SetTrue),
        );
    }
    cmd.args(fs_core::cli::format_args()).after_help(
        "Examples:\n  \
         lzo1x notes.txt                      makes notes.txt.lzo, removes notes.txt\n  \
         lzo1x -d notes.txt.lzo               gives notes.txt back\n  \
         lzo1x -k -9 big.bin                  keeps big.bin\n  \
         tar cf - dir | lzo1x > dir.tar.lzo   standard input to standard output\n  \
         lzo1x -l dir.tar.lzo                 name and sizes\n  \
         lzo1x --raw -o block.lzo1x input.bin\n  \
         lzo1x --raw -d --size 4096 -o output.bin block.lzo1x",
    )
}

fn fail(path: &Path, what: impl std::fmt::Display) -> CliError {
    CliError::failed(format!("{}: {what}", path.display()))
}

fn level_of(m: &ArgMatches) -> u8 {
    (1..=9u8)
        .rev()
        .find(|l| m.get_flag(["1", "2", "3", "4", "5", "6", "7", "8", "9"][usize::from(l - 1)]))
        .unwrap_or(5)
}

fn run(m: &ArgMatches) -> Result<Outcome, CliError> {
    let files: Vec<PathBuf> = m
        .get_many::<OsString>("files")
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .collect();
    let output = m.get_one::<OsString>("output").map(PathBuf::from);
    if output.is_some() && files.len() > 1 {
        return Err(CliError::usage(
            "-o names one output, and more than one input was given",
        ));
    }
    if m.get_flag("raw") {
        return raw(m, &files, output.as_deref());
    }
    let stdin_only = files.is_empty() || (files.len() == 1 && files[0] == Path::new("-"));
    if m.get_flag("list") {
        return list(&files);
    }
    if m.get_flag("test") {
        return test(&files);
    }
    if stdin_only && output.is_none() {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        let stdout = io::stdout();
        let mut out = BufWriter::new(stdout.lock());
        if m.get_flag("decompress") {
            let header = lzop::read_header(&mut input)
                .map_err(|e| CliError::failed(format!("stdin: {e}")))?;
            lzop::decompress(&mut input, &mut out, &header)
                .map_err(|e| CliError::failed(format!("stdin: {e}")))?;
        } else {
            let mut header = Header::new(b"", 0o100644, 0, level_of(m));
            header.flags |= lzop::flags::STDOUT;
            lzop::compress(&mut input, &mut out, &header)
                .map_err(|e| CliError::failed(format!("stdin: {e}")))?;
        }
        out.flush()
            .map_err(|e| CliError::failed(format!("stdout: {e}")))?;
        return Ok(Outcome::done());
    }

    let mut done = Vec::new();
    let mut errors = Vec::new();
    for file in &files {
        let result = if m.get_flag("decompress") {
            decompress_file(m, file, output.as_deref())
        } else {
            compress_file(m, file, output.as_deref())
        };
        match result {
            Ok(Some(report)) => done.push(report),
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    if let Some(first) = errors.into_iter().next() {
        return Err(first);
    }
    if m.get_flag("stdout") {
        return Ok(Outcome::done());
    }
    Ok(Outcome::report(Json::object([("files", Json::Arr(done))])).with_text(String::new()))
}

/// Open `path` for writing, refusing to replace a file unless forced.
fn create(path: &Path, force: bool) -> Result<File, CliError> {
    if !force && path.exists() {
        return Err(fail(path, "exists; use -f to overwrite it"));
    }
    File::create(path).map_err(|e| fail(path, e))
}

fn compress_file(
    m: &ArgMatches,
    file: &Path,
    output: Option<&Path>,
) -> Result<Option<Json>, CliError> {
    let meta = std::fs::metadata(file).map_err(|e| fail(file, e))?;
    if !meta.is_file() {
        return Err(fail(file, "not a regular file"));
    }
    let name = file
        .file_name()
        .map(OsStr::to_string_lossy)
        .unwrap_or_default();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let header = Header::new(
        name.as_bytes(),
        0o100000 | (meta.permissions().mode() & 0o7777),
        mtime,
        level_of(m),
    );
    let mut input = BufReader::new(File::open(file).map_err(|e| fail(file, e))?);
    if m.get_flag("stdout") {
        let stdout = io::stdout();
        let mut out = BufWriter::new(stdout.lock());
        let mut header = header;
        header.flags |= lzop::flags::STDOUT;
        lzop::compress(&mut input, &mut out, &header).map_err(|e| fail(file, e))?;
        out.flush().map_err(|e| fail(file, e))?;
        return Ok(None);
    }
    let target = output.map(Path::to_path_buf).unwrap_or_else(|| {
        let mut s = file.as_os_str().to_os_string();
        s.push(SUFFIX);
        PathBuf::from(s)
    });
    let mut out = BufWriter::new(create(&target, m.get_flag("force"))?);
    let totals = lzop::compress(&mut input, &mut out, &header)
        .and_then(|t| out.flush().map(|_| t).map_err(FileError::from))
        .map_err(|e| {
            let _ = std::fs::remove_file(&target);
            fail(file, e)
        })?;
    drop(out);
    let _ = std::fs::set_permissions(
        &target,
        std::fs::Permissions::from_mode(meta.permissions().mode() & 0o7777),
    );
    if !m.get_flag("keep") && output.is_none() {
        std::fs::remove_file(file).map_err(|e| fail(file, e))?;
    }
    Ok(Some(Json::object([
        ("input", Json::from(file.display().to_string().as_str())),
        ("output", Json::from(target.display().to_string().as_str())),
        ("bytes_in", Json::from(totals.uncompressed)),
        ("bytes_out", Json::from(totals.compressed)),
    ])))
}

fn decompress_file(
    m: &ArgMatches,
    file: &Path,
    output: Option<&Path>,
) -> Result<Option<Json>, CliError> {
    let target = match output {
        Some(o) => Some(o.to_path_buf()),
        None if m.get_flag("stdout") => None,
        None => {
            let s = file.to_string_lossy();
            match s.strip_suffix(SUFFIX) {
                Some(stem) if !stem.is_empty() => Some(PathBuf::from(stem)),
                _ => {
                    return Err(fail(
                        file,
                        format!(
                        "does not end in {SUFFIX}, so there is no name to give back; use -c or -o"
                    ),
                    ))
                }
            }
        }
    };
    let mut input = BufReader::new(File::open(file).map_err(|e| fail(file, e))?);
    let header = lzop::read_header(&mut input).map_err(|e| fail(file, e))?;
    let Some(target) = target else {
        let stdout = io::stdout();
        let mut out = BufWriter::new(stdout.lock());
        lzop::decompress(&mut input, &mut out, &header).map_err(|e| fail(file, e))?;
        out.flush().map_err(|e| fail(file, e))?;
        return Ok(None);
    };
    let mut out = BufWriter::new(create(&target, m.get_flag("force"))?);
    // A file that fails a checksum part-way is removed, not left half
    // written beside a reason.
    let totals = lzop::decompress(&mut input, &mut out, &header)
        .and_then(|t| out.flush().map(|_| t).map_err(FileError::from))
        .map_err(|e| {
            let _ = std::fs::remove_file(&target);
            fail(file, e)
        })?;
    drop(out);
    let mode = header.mode & 0o7777;
    if mode != 0 {
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode));
    }
    if header.mtime != 0 {
        if let Ok(f) = File::options().write(true).open(&target) {
            let _ = f
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(header.mtime));
        }
    }
    if !m.get_flag("keep") && output.is_none() {
        std::fs::remove_file(file).map_err(|e| fail(file, e))?;
    }
    Ok(Some(Json::object([
        ("input", Json::from(file.display().to_string().as_str())),
        ("output", Json::from(target.display().to_string().as_str())),
        ("bytes_in", Json::from(totals.compressed)),
        ("bytes_out", Json::from(totals.uncompressed)),
    ])))
}

fn open_lzo(file: &Path) -> Result<(BufReader<Box<dyn Read>>, Header), CliError> {
    let reader: Box<dyn Read> = if file == Path::new("-") {
        Box::new(io::stdin())
    } else {
        Box::new(File::open(file).map_err(|e| fail(file, e))?)
    };
    let mut input = BufReader::new(reader);
    let header = lzop::read_header(&mut input).map_err(|e| fail(file, e))?;
    Ok((input, header))
}

fn inputs(files: &[PathBuf]) -> Vec<PathBuf> {
    if files.is_empty() {
        vec![PathBuf::from("-")]
    } else {
        files.to_vec()
    }
}

fn list(files: &[PathBuf]) -> Result<Outcome, CliError> {
    let mut rows = Vec::new();
    let mut text = Vec::new();
    for file in inputs(files) {
        let (mut input, header) = open_lzo(&file)?;
        let totals = lzop::scan(&mut input, &header).map_err(|e| fail(&file, e))?;
        let name = String::from_utf8_lossy(&header.name).into_owned();
        text.push(format!(
            "{}\t{}\t{}",
            totals.compressed, totals.uncompressed, name
        ));
        rows.push(Json::object([
            ("file", Json::from(file.display().to_string().as_str())),
            ("name", Json::from(name.as_str())),
            ("compressed", Json::from(totals.compressed)),
            ("uncompressed", Json::from(totals.uncompressed)),
            ("blocks", Json::from(totals.blocks)),
            ("method", Json::from(u64::from(header.method))),
            ("level", Json::from(u64::from(header.level))),
            ("mtime", Json::from(header.mtime)),
        ]));
    }
    Ok(Outcome::report(Json::object([("files", Json::Arr(rows))])).with_text(text.join("\n")))
}

fn test(files: &[PathBuf]) -> Result<Outcome, CliError> {
    let mut rows = Vec::new();
    let mut text = Vec::new();
    for file in inputs(files) {
        let (mut input, header) = open_lzo(&file)?;
        let totals =
            lzop::decompress(&mut input, &mut io::sink(), &header).map_err(|e| fail(&file, e))?;
        text.push(format!("{}: ok", file.display()));
        rows.push(Json::object([
            ("file", Json::from(file.display().to_string().as_str())),
            ("ok", Json::from(true)),
            ("uncompressed", Json::from(totals.uncompressed)),
        ]));
    }
    Ok(Outcome::report(Json::object([("files", Json::Arr(rows))])).with_text(text.join("\n")))
}

/// One raw block, either way.
fn raw(m: &ArgMatches, files: &[PathBuf], output: Option<&Path>) -> Result<Outcome, CliError> {
    if files.len() > 1 {
        return Err(CliError::usage("--raw works on one block: give one input"));
    }
    let input_path = files.first().cloned().unwrap_or_else(|| PathBuf::from("-"));
    let data = if input_path == Path::new("-") {
        let mut buf = Vec::new();
        io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| CliError::failed(format!("stdin: {e}")))?;
        buf
    } else {
        std::fs::read(&input_path).map_err(|e| fail(&input_path, e))?
    };
    let out = if m.get_flag("decompress") {
        let size = *m.get_one::<u64>("size").ok_or_else(|| {
            CliError::usage("--raw -d needs --size: a raw block does not record its size")
        })?;
        let bound = usize::try_from(size)
            .map_err(|_| CliError::usage(format!("--size {size} does not fit in memory here")))?;
        lzo1x::decompress(&data, bound).map_err(|e| fail(&input_path, e))?
    } else {
        lzo1x::compress(&data)
    };
    match output {
        Some(path) => {
            std::fs::write(path, &out).map_err(|e| fail(path, e))?;
            Ok(Outcome::report(Json::object([
                (
                    "input",
                    Json::from(input_path.display().to_string().as_str()),
                ),
                ("output", Json::from(path.display().to_string().as_str())),
                ("bytes_in", Json::from(data.len() as u64)),
                ("bytes_out", Json::from(out.len() as u64)),
            ]))
            .with_text(String::new()))
        }
        None => {
            io::stdout()
                .write_all(&out)
                .map_err(|e| CliError::failed(format!("stdout: {e}")))?;
            Ok(Outcome::done())
        }
    }
}

fn main() -> ExitCode {
    cli::main(&FAMILY)
}
