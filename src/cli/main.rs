//! `rust-lzo1x`: the command-line tool, one multi-call binary.
//!
//! Installed as `rust-lzo1x` and linked as `lzo1x`. The dispatch, the
//! `--version`, `doctor`, the output contract, the man pages and the
//! completions are `fs_core::cli` (am-fs-core's `cli` feature), the same
//! plumbing every tool in the family uses; this file is only the tool.
//!
//! It works on raw LZO1X blocks, deliberately: no container, no header, no
//! framing. That is what a btrfs extent or a SquashFS block holds, and what
//! this crate's library API takes and returns. `decompress` needs the
//! uncompressed size because the format does not carry it; callers always
//! know it from somewhere else (a btrfs extent header, a SquashFS block
//! table), so the decoder takes it as a bound rather than guessing.

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{value_parser, Arg, ArgMatches, Command as Cmd};
use fs_core::cli::{self, CliError, Json, Outcome, Tool};

static FAMILY: cli::Family = cli::Family {
    repo: "rust-lzo1x",
    crate_name: env!("CARGO_PKG_NAME"),
    version: env!("CARGO_PKG_VERSION"),
    about: "LZO1X tools: compress and decompress raw LZO1X blocks",
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
    about: "Compress and decompress raw LZO1X blocks",
    command,
    run,
};

fn command() -> Cmd {
    let files = |cmd: Cmd| {
        cmd.arg(
            Arg::new("input")
                .value_name("IN")
                .required(true)
                .value_parser(value_parser!(OsString)),
        )
        .arg(
            Arg::new("output")
                .value_name("OUT")
                .required(true)
                .value_parser(value_parser!(OsString)),
        )
    };
    Cmd::new("lzo1x")
        .about("Compress and decompress raw LZO1X blocks")
        .long_about(
            "Compress and decompress raw LZO1X blocks: no container, no header, no \
             framing, which is what a btrfs extent or a SquashFS block holds.\n\n\
             The format does not record the uncompressed size, so `decompress` is told \
             it, and refuses a stream that holds more.",
        )
        .subcommand_required(true)
        .subcommand(
            files(Cmd::new("compress").about("Compress IN into the raw block OUT"))
                .after_help("Examples:\n  lzo1x compress input.bin block.lzo1x"),
        )
        .subcommand(
            files(Cmd::new("decompress").about("Decompress the raw block IN into OUT"))
                .arg(
                    Arg::new("size")
                        .value_name("UNCOMPRESSED-SIZE")
                        .help("How many bytes the block decompresses to, at most")
                        .required(true)
                        .value_parser(value_parser!(u64)),
                )
                .after_help("Examples:\n  lzo1x decompress block.lzo1x output.bin 4096"),
        )
        .args(fs_core::cli::format_args())
        .after_help(
            "Examples:\n  \
             lzo1x compress   input.bin  block.lzo1x\n  \
             lzo1x decompress block.lzo1x output.bin 4096   the size is not in the stream",
        )
}

fn name(m: &ArgMatches, id: &str) -> String {
    m.get_one::<OsString>(id)
        .expect("clap requires it")
        .to_string_lossy()
        .into_owned()
}

fn read(path: &str) -> Result<Vec<u8>, CliError> {
    std::fs::read(path).map_err(|e| CliError::failed(format!("{path}: {e}")))
}

fn write(path: &str, bytes: &[u8]) -> Result<(), CliError> {
    std::fs::write(path, bytes).map_err(|e| CliError::failed(format!("{path}: {e}")))
}

fn run(matches: &ArgMatches) -> Result<Outcome, CliError> {
    let (verb, m) = matches.subcommand().expect("clap requires a subcommand");
    let (input, output) = (name(m, "input"), name(m, "output"));
    let data = read(&input)?;
    let out = match verb {
        "compress" => lzo1x::compress(&data),
        "decompress" => {
            let size = *m.get_one::<u64>("size").expect("clap requires it");
            let bound = usize::try_from(size).map_err(|_| {
                CliError::usage(format!("{size} bytes does not fit in memory here"))
            })?;
            lzo1x::decompress(&data, bound)
                .map_err(|e| CliError::failed(format!("{input}: {e}")))?
        }
        other => unreachable!("clap knows no subcommand {other}"),
    };
    write(&output, &out)?;
    Ok(Outcome::report(Json::object([
        ("input", Json::from(input.as_str())),
        ("output", Json::from(output.as_str())),
        ("bytes_in", Json::from(data.len() as u64)),
        ("bytes_out", Json::from(out.len() as u64)),
    ]))
    .with_text(String::new()))
}

fn main() -> ExitCode {
    cli::main(&FAMILY)
}
