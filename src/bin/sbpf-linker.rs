use std::{env, ffi::CString, fs, path::PathBuf, str::FromStr};

#[cfg(any(
    feature = "rust-llvm-19",
    feature = "rust-llvm-20",
    feature = "rust-llvm-21"
))]
use aya_rustc_llvm_proxy as _;
use bpf_linker::{Cpu, Linker, LinkerOptions, OptLevel, OutputType};
use clap::{Parser, error::ErrorKind};
use sbpf_linker::{SbpfLinkerError, link_program};

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(
        "optimization level needs to be between 0-3, s or z (instead was `{0}`)"
    )]
    InvalidOptimization(String),
    #[error("Clap Parse Error")]
    ClapParseError,
    #[error("SBPF Linker Error. Error detail: ({0}).")]
    SbpfLinkerError(#[from] SbpfLinkerError),
    #[error("Program Read Error. Error detail: ({msg}).")]
    ProgramReadError { msg: String },
    #[error("Program Write Error. Error detail: ({msg}).")]
    ProgramWriteError { msg: String },
    //     #[error("unknown emission type: `{0}` - expected one of: `llvm-bc`, `asm`, `llvm-ir`, `obj`")]
    //     InvalidOutputType(String),
}

#[derive(Copy, Clone, Debug)]
struct CliOptLevel(OptLevel);

impl FromStr for CliOptLevel {
    type Err = CliError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(match s {
            "0" => OptLevel::No,
            "1" => OptLevel::Less,
            "2" => OptLevel::Default,
            "3" => OptLevel::Aggressive,
            "s" => OptLevel::Size,
            "z" => OptLevel::SizeMin,
            _ => return Err(CliError::InvalidOptimization(s.to_string())),
        }))
    }
}

#[derive(Debug, Parser)]
#[command(version)]
struct CommandLine {
    /// LLVM target triple. When not provided, the target is inferred from the inputs
    #[clap(long)]
    target: Option<String>,

    /// Target BPF processor. Can be one of `generic`, `probe`, `v1`, `v2`, `v3`
    #[clap(long, default_value = "generic")]
    cpu: Cpu,

    /// Write output to <output>
    #[clap(short, long, required = true)]
    output: PathBuf,

    /// Emit BTF information
    #[clap(long)]
    btf: bool,

    /// Permit automatic insertion of `__bpf_trap` calls.
    /// See: <https://github.com/llvm/llvm-project/commit/ab391beb11f733b526b86f9df23734a34657d876>
    #[clap(long)]
    allow_bpf_trap: bool,

    /// Add a directory to the library search path
    #[clap(short = 'L', number_of_values = 1)]
    libs: Vec<PathBuf>,

    /// Optimization level. 0-3, s, or z
    #[clap(short = 'O', default_value = "2")]
    optimize: Vec<CliOptLevel>,

    /// Export the symbols specified in the file `path`. The symbols must be separated by new lines
    #[clap(long, value_name = "path")]
    export_symbols: Option<PathBuf>,

    /// Try hard to unroll loops. Useful when targeting kernels that don't support loops
    #[clap(long)]
    unroll_loops: bool,

    /// Ignore `noinline`/`#[inline(never)]`. Useful when targeting kernels that don't support function calls
    #[clap(long)]
    ignore_inline_never: bool,

    /// Dump the final IR module to the given `path` before generating the code
    #[clap(long, value_name = "path")]
    dump_module: Option<PathBuf>,

    /// Extra command line arguments to pass to LLVM
    #[clap(long, value_name = "args", use_value_delimiter = true, action = clap::ArgAction::Append)]
    llvm_args: Vec<CString>,

    /// Disable passing --bpf-expand-memcpy-in-order to LLVM.
    #[clap(long)]
    disable_expand_memcpy_in_order: bool,

    /// Disable exporting `memcpy`, `memmove`, `memset`, `memcmp` and `bcmp`. Exporting
    /// those is commonly needed when LLVM does not manage to expand memory
    /// intrinsics to a sequence of loads and stores.
    #[clap(long)]
    disable_memory_builtins: bool,

    /// Input files. Can be object files or static libraries
    #[clap(required = true)]
    inputs: Vec<PathBuf>,

    /// Comma separated list of symbols to export. See also `--export-symbols`
    #[clap(long, value_name = "symbols", use_value_delimiter = true, action = clap::ArgAction::Append)]
    export: Vec<String>,

    /// Whether to treat LLVM errors as fatal.
    #[clap(long, action = clap::ArgAction::Set, default_value_t = true)]
    fatal_errors: bool,

    // The options below are for wasm-ld compatibility
    #[clap(long = "debug", hide = true)]
    _debug: bool,
}

fn main() -> Result<(), CliError> {
    if env::args().len() == 1 {
        CommandLine::parse_from(&["sbpf-linker", "--help"]);
        return Ok(());
    }

    let args: Vec<String> = env::args()
        .map(|arg| if arg == "-flavor" { "--flavor".to_string() } else { arg })
        .collect();

    let CommandLine {
        target,
        cpu,
        output,
        btf,
        allow_bpf_trap,
        libs,
        optimize,
        export_symbols,
        unroll_loops,
        ignore_inline_never,
        dump_module,
        llvm_args,
        disable_expand_memcpy_in_order,
        disable_memory_builtins,
        inputs,
        export,
        fatal_errors,
        _debug,
    } = match Parser::try_parse_from(args) {
        Ok(command_line) => command_line,
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                print!("{err}");
                return Ok(());
            }
            _ => {
                // Let Clap handle its own error display for better formatting
                eprintln!("{err}");
                return Err(CliError::ClapParseError);
            }
        },
    };

    let export_symbols =
        export_symbols.map(fs::read_to_string).transpose().map_err(|e| {
            CliError::SbpfLinkerError(SbpfLinkerError::ObjectFileReadError(e))
        })?;

    // TODO: the data is owned by this call frame; we could make this zero-alloc.
    let export_symbols = export_symbols
        .as_deref()
        .into_iter()
        .flat_map(str::lines)
        .map(str::to_owned)
        .chain(export)
        .map(Into::into)
        .collect();

    let optimize = match *optimize.as_slice() {
        [] => unreachable!("emit has a default value"),
        [.., CliOptLevel(optimize)] => optimize,
    };

    let mut linker = Linker::new(LinkerOptions {
        target,
        cpu,
        cpu_features: String::new(),
        inputs,
        output: output.clone(),
        output_type: OutputType::Object,
        libs,
        optimize,
        export_symbols,
        unroll_loops,
        ignore_inline_never,
        dump_module,
        llvm_args: llvm_args
            .into_iter()
            .map(|cstring| cstring.into_string().unwrap_or_default())
            .collect(),
        disable_expand_memcpy_in_order,
        disable_memory_builtins,
        btf,
        allow_bpf_trap,
    });

    linker.link().map_err(|e| {
        CliError::SbpfLinkerError(SbpfLinkerError::LinkerError(e))
    })?;

    if fatal_errors && linker.has_errors() {
        return Err(CliError::SbpfLinkerError(
            SbpfLinkerError::LlvmDiagnosticError,
        ));
    }

    let program = std::fs::read(&output)
        .map_err(|e| CliError::ProgramReadError { msg: e.to_string() })?;
    let bytecode =
        link_program(&program).map_err(CliError::SbpfLinkerError)?;

    let src_name = std::path::Path::new(&output)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("main");
    let output_path = std::path::Path::new(&output)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(format!("{src_name}.so"));
    std::fs::write(output_path, bytecode)
        .map_err(|e| CliError::ProgramWriteError { msg: e.to_string() })?;

    Ok(())
}
