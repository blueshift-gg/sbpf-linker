use std::{
    env,
    ffi::{CStr, CString, c_void},
    fs, io,
    path::{Component, Path, PathBuf},
    ptr,
    str::FromStr,
};

use bpf_linker::{
    Cpu, Linker, LinkerInput, LinkerOptions, OptLevel, OutputType,
};
use clap::{
    Parser,
    builder::{PathBufValueParser, TypedValueParser as _},
    error::ErrorKind,
};
use thiserror::Error;
use tracing::{Level, info};
use tracing_subscriber::{EnvFilter, fmt::MakeWriter, prelude::*};
use tracing_tree::HierarchicalLayer;

use sbpf_linker::{
    OptimizationConfig, ProgramOptions, SbpfArch, SbpfLinkerError,
    link_program,
};

const MATH_BUILTINS: [&str; 47] = [
    "__multi3",
    "__udivti3",
    "__umodti3",
    "__divti3",
    "__modti3",
    // f64 arithmetic, conversions, comparisons
    "__adddf3",
    "__subdf3",
    "__negdf2",
    "__muldf3",
    "__divdf3",
    "__floatundidf",
    "__fixunsdfdi",
    "__gedf2",
    "__gtdf2",
    "__floatunsidf",
    "__floatdidf",
    "__floatsidf",
    "__fixunsdfsi",
    "__fixdfdi",
    "__fixdfsi",
    "__ltdf2",
    "__ledf2",
    "__eqdf2",
    "__nedf2",
    "__unorddf2",
    // f32/f64 conversions
    "__extendsfdf2",
    "__truncdfsf2",
    // f32 arithmetic, conversions, comparisons
    "__addsf3",
    "__subsf3",
    "__negsf2",
    "__mulsf3",
    "__divsf3",
    "__floatunsisf",
    "__floatsisf",
    "__floatdisf",
    "__floatundisf",
    "__ltsf2",
    "__lesf2",
    "__eqsf2",
    "__nesf2",
    "__gtsf2",
    "__gesf2",
    "__unordsf2",
    "__fixunssfdi",
    "__fixsfdi",
    "__fixunssfsi",
    "__fixsfsi",
];

#[derive(Debug, Error)]
enum CliError {
    #[error(
        "optimization level needs to be between 0-3, s or z (instead was `{0}`)"
    )]
    InvalidOptimization(String),
    #[error(
        "unknown emission type: `{0}` - expected one of: `llvm-bc`, `asm`, `llvm-ir`, `obj`"
    )]
    InvalidOutputType(String),
    #[error("unknown architecture: `{0}` - expected one of: `v0`, `v3`")]
    InvalidArch(String),
    #[error(
        "sBPF architecture `v0` only supports CPU architectures `generic`, `v1`, or `v2` (instead was `{0}`)"
    )]
    UnsupportedV0Cpu(Cpu),
    #[error(
        "invalid `--bpf-stack-size` value `{0}`; expected a positive value that fits in i32"
    )]
    InvalidStackFrameSize(String),
    #[error("missing required `--bpf-stack-size=<value>` LLVM argument")]
    MissingStackFrameSize,
    #[error("Program Write Error. Error detail: ({msg}).")]
    ProgramWriteError { msg: String },
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

#[derive(Copy, Clone, Debug)]
struct CliOutputType(OutputType);

impl FromStr for CliOutputType {
    type Err = CliError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(match s {
            "llvm-bc" => OutputType::Bitcode,
            "asm" => OutputType::Assembly,
            "llvm-ir" => OutputType::LlvmAssembly,
            "obj" => OutputType::Object,
            _ => return Err(CliError::InvalidOutputType(s.to_string())),
        }))
    }
}

#[derive(Copy, Clone, Debug)]
struct CliArch(SbpfArch);

impl FromStr for CliArch {
    type Err = CliError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(match s {
            "v0" => SbpfArch::V0,
            "v3" => SbpfArch::V3,
            _ => return Err(CliError::InvalidArch(s.to_string())),
        }))
    }
}

fn parent_and_file_name(p: PathBuf) -> anyhow::Result<(PathBuf, PathBuf)> {
    let mut comps = p.components();
    let file_name = match comps.next_back() {
        Some(Component::Normal(p)) => p,
        Some(p) => anyhow::bail!("unexpected path component {p:?}"),
        None => anyhow::bail!("unexpected empty path"),
    };
    Ok((comps.as_path().to_path_buf(), PathBuf::from(file_name)))
}

/// Symbols `solana-compiler-builtins` defines, grouped by category. The crate is
/// treated as linked when at least one symbol from every category is defined.
const SOLANA_BUILTINS_FINGERPRINT: [&[&CStr]; 3] = [
    &[c"memcpy", c"memset", c"memmove"],
    &[c"__multi3", c"__udivti3", c"__umodti3"],
    &[c"__adddf3", c"__muldf3", c"__divdf3"],
];

/// Returns which [`SOLANA_BUILTINS_FINGERPRINT`] categories `bitcode` defines,
/// regardless of linkage. Anything that isn't valid bitcode defines none.
fn defined_fingerprint_categories(
    bitcode: &[u8],
) -> [bool; SOLANA_BUILTINS_FINGERPRINT.len()] {
    use bpf_linker::llvm_sys::{
        bit_reader::LLVMParseBitcodeInContext2,
        core::{
            LLVMContextCreate, LLVMContextDispose,
            LLVMContextSetDiagnosticHandler,
            LLVMCreateMemoryBufferWithMemoryRange, LLVMDisposeMemoryBuffer,
            LLVMDisposeModule, LLVMGetNamedFunction, LLVMIsDeclaration,
        },
        prelude::LLVMDiagnosticInfoRef,
    };

    extern "C" fn ignore_diagnostic(_: LLVMDiagnosticInfoRef, _: *mut c_void) {
    }

    let mut defined = [false; SOLANA_BUILTINS_FINGERPRINT.len()];
    if !matches!(bitcode.get(..4), Some(b"BC\xC0\xDE" | b"\xDE\xC0\x17\x0B")) {
        return defined;
    }
    // SAFETY: the buffer borrows `bitcode`, which outlives it, and the module,
    // buffer and context are each disposed exactly once, module first.
    unsafe {
        let context = LLVMContextCreate();
        // The default handler exits the process on malformed bitcode.
        LLVMContextSetDiagnosticHandler(
            context,
            Some(ignore_diagnostic),
            ptr::null_mut(),
        );
        let buffer = LLVMCreateMemoryBufferWithMemoryRange(
            bitcode.as_ptr().cast(),
            bitcode.len(),
            c"input".as_ptr(),
            0,
        );
        let mut module = ptr::null_mut();
        if LLVMParseBitcodeInContext2(context, buffer, &mut module) == 0 {
            for (defined, names) in
                defined.iter_mut().zip(SOLANA_BUILTINS_FINGERPRINT)
            {
                *defined = names.iter().any(|name| {
                    let function = LLVMGetNamedFunction(module, name.as_ptr());
                    !function.is_null() && LLVMIsDeclaration(function) == 0
                });
            }
            LLVMDisposeModule(module);
        }
        LLVMDisposeMemoryBuffer(buffer);
        LLVMContextDispose(context);
    }
    defined
}

/// Whether inputs include `solana-compiler-builtins`, which might come from
///
/// 1. solana-compiler-builtins crate
/// 2. hashed rlib rustc passes for a cargo dependency
/// 3. unhashed copy `cargo build-sbpf` caches
/// 4. per-crate object rustc passes with thin LTO
/// 5. merged object rustc passes with fat LTO, recognized by the symbols it defines
fn builtins_linked(inputs: &[PathBuf]) -> bool {
    let file_names =
        || inputs.iter().filter_map(|input| input.file_name()?.to_str());
    if file_names().any(|name| name.contains("solana_compiler_builtins")) {
        return true;
    }
    // Rustc passes rlibs unless cross-crate LTO merged them into objects.
    if file_names().any(|name| name.ends_with(".rlib")) {
        return false;
    }
    let mut linked = [false; SOLANA_BUILTINS_FINGERPRINT.len()];
    for input in inputs {
        let Ok(bitcode) = fs::read(input) else { continue };
        for (linked, defined) in
            linked.iter_mut().zip(defined_fingerprint_categories(&bitcode))
        {
            *linked |= defined;
        }
        if linked.iter().all(|&linked| linked) {
            return true;
        }
    }
    false
}

fn warn_if_builtins_missing(inputs: &[PathBuf]) {
    if !builtins_linked(inputs) {
        eprintln!(
            "warning: `solana-compiler-builtins` was not linked. Code \
             generation may fail for operations that require libcalls, and \
             some operations may run less efficiently on the SVM. To fix \
             this, run `cargo add solana-compiler-builtins`, then add \
             `use solana_compiler_builtins as _;` to `src/lib.rs`."
        );
    }
}

#[derive(Debug, Parser)]
#[command(version)]
struct CommandLine {
    /// LLVM target triple. When not provided, the target is inferred from the inputs
    #[clap(long)]
    target: Option<CString>,

    /// Target BPF processor. Can be one of `generic`, `probe`, `v1`, `v2`, `v3`, `v4`.
    /// Rustc forwards this from `-C target-cpu`; direct invocations default to `v2`.
    #[clap(long, default_value = "v2")]
    cpu: Cpu,

    /// Enable or disable CPU features. Rustc forwards this from `-C target-feature`.
    /// Use +feature to enable a feature, or -feature to disable it. For example
    /// --cpu-features=+allows-misaligned-mem-access,+alu32,-dwarfris
    /// Can be repeated; the values are joined in order, so a later setting of a
    /// feature overrides an earlier one.
    #[clap(
        long,
        value_name = "features",
        allow_hyphen_values = true,
        action = clap::ArgAction::Append
    )]
    cpu_features: Vec<CString>,

    /// Write output to <output>
    #[clap(short, long)]
    output: PathBuf,

    /// Output type. Can be one of `llvm-bc`, `asm`, `llvm-ir`, `obj`
    #[clap(long, default_value = "obj")]
    emit: Vec<CliOutputType>,

    /// Emit BTF information. Can get DWARF symbols only if BTF is enabled and if requested from `rustc` with `-C debuginfo=N`
    #[clap(long)]
    btf: bool,

    /// Permit automatic insertion of __bpf_trap calls.
    /// See: https://github.com/llvm/llvm-project/commit/ab391beb11f733b526b86f9df23734a34657d876
    #[clap(long)]
    allow_bpf_trap: bool,

    /// Add a directory to the library search path
    #[clap(short = 'L', number_of_values = 1)]
    _libs: Vec<PathBuf>,

    /// Optimization level. 0-3, s, or z
    #[clap(short = 'O', default_value = "2")]
    optimize: Vec<CliOptLevel>,

    /// Export the symbols specified in the file `path`. The symbols must be separated by new lines
    #[clap(long, value_name = "path")]
    export_symbols: Option<PathBuf>,

    /// Output logs to the given `path`
    #[clap(
        long,
        value_name = "path",
        value_parser = PathBufValueParser::new().try_map(parent_and_file_name),
    )]
    log_file: Option<(PathBuf, PathBuf)>,

    /// Set the log level. If not specified, no logging is used. Can be one of
    /// `error`, `warn`, `info`, `debug`, `trace`.
    #[clap(long, value_name = "level")]
    log_level: Option<Level>,

    /// Try hard to unroll loops. Useful when targeting kernels that don't support loops
    #[clap(long)]
    unroll_loops: bool,

    /// Ignore `noinline`/`#[inline(never)]`. Useful when targeting kernels that don't support function calls
    #[clap(long)]
    ignore_inline_never: bool,

    /// Dump the final IR module to the given `path` before generating the code
    #[clap(long, value_name = "path")]
    dump_module: Option<PathBuf>,

    /// Write CFG .dot dumps to this directory
    #[clap(long, value_name = "dir")]
    dump_cfg_dir: Option<PathBuf>,

    /// Enable SBPF assembler optimizations
    #[clap(long, default_value_t = true, action = clap::ArgAction::Set)]
    sbpf_optimize: bool,

    /// sBPF target architecture. Can be one of `v0`, `v3`
    #[clap(long, default_value = "v3", hide = true)]
    arch: CliArch,

    /// Extra command line arguments to pass to LLVM
    #[clap(long, value_name = "args", use_value_delimiter = true, action = clap::ArgAction::Append)]
    llvm_args: Vec<CString>,

    /// Disable passing --bpf-expand-memcpy-in-order to LLVM.
    #[clap(long, default_value_t = true, hide = true, action = clap::ArgAction::Set)]
    disable_expand_memcpy_in_order: bool,

    /// Disable exporting memcpy, memmove, memset, memcmp and bcmp. Exporting
    /// those is commonly needed when LLVM does not manage to expand memory
    /// intrinsics to a sequence of loads and stores.
    #[clap(long, hide = true)]
    disable_memory_builtins: bool,

    /// Disable exporting compiler-provided math builtins.
    #[clap(long, hide = true)]
    disable_math_builtins: bool,

    /// Input files. Can be object files or static libraries
    #[clap(required = true)]
    inputs: Vec<PathBuf>,

    /// Comma separated list of symbols to export. See also `--export-symbols`
    #[clap(long, value_name = "symbols", use_value_delimiter = true, action = clap::ArgAction::Append)]
    export: Vec<String>,

    /// Whether to treat LLVM errors as fatal.
    #[clap(long, action = clap::ArgAction::Set, default_value_t = true)]
    fatal_errors: bool,

    /// The options below are for wasm-ld compatibility
    #[clap(long = "debug", hide = true)]
    _debug: bool,

    /// Strips the `lib` prefix from the output file and places it in the `target/deploy` directory for deployment
    #[clap(long, default_value_t = true, hide = true, action = clap::ArgAction::Set)]
    deploy: bool,
}

/// Returns a [`HierarchicalLayer`](tracing_tree::HierarchicalLayer) for the
/// given `writer`.
fn tracing_layer<W>(writer: W) -> HierarchicalLayer<W>
where
    W: for<'writer> MakeWriter<'writer> + 'static,
{
    const TRACING_IDENT: usize = 2;
    HierarchicalLayer::new(TRACING_IDENT)
        .with_indent_lines(true)
        .with_writer(writer)
}

/// Joins every `--cpu-features` value into the one feature string LLVM takes.
fn join_cpu_features(features: &[CString]) -> CString {
    let features = features
        .iter()
        .map(|features| features.as_bytes())
        .filter(|features| !features.is_empty())
        .collect::<Vec<_>>()
        .join(&b',');
    CString::new(features).expect("no value contains a NUL byte")
}

fn process_cli_options<I>(args: I) -> anyhow::Result<CommandLine>
where
    I: Iterator<Item = String>,
{
    let cli: CommandLine = match Parser::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => match err.kind() {
            ErrorKind::DisplayHelp => {
                print!("{err}");
                std::process::exit(0);
            }
            ErrorKind::DisplayVersion => {
                print!("{err}");
                #[cfg(feature = "rust-llvm")]
                {
                    println!("LLVM dynamically linked at runtime");
                    println!(
                        "Warning: source builds are for sbpf-linker \
                         developers only and require nightly 100+; \
                         use `cargo binstall sbpf-linker` when \
                         building Solana programs."
                    );
                }
                #[cfg(not(feature = "rust-llvm"))]
                {
                    let (mut major, mut minor, mut patch) = (0, 0, 0);
                    // SAFETY: LLVMGetVersion only writes to the three valid output pointers.
                    unsafe {
                        bpf_linker::llvm_sys::core::LLVMGetVersion(
                            &mut major, &mut minor, &mut patch,
                        );
                    }
                    println!("LLVM {major}.{minor}.{patch}");
                }
                std::process::exit(0);
            }
            _ => return Err(err.into()),
        },
    };
    if matches!(cli.arch.0, SbpfArch::V0)
        && !matches!(cli.cpu, Cpu::Generic | Cpu::V1 | Cpu::V2)
    {
        return Err(CliError::UnsupportedV0Cpu(cli.cpu).into());
    }

    Ok(cli)
}

fn main() -> anyhow::Result<()> {
    let args = env::args().map(|arg| {
        if arg == "-flavor" { "--flavor".to_string() } else { arg }
    });

    let cli = process_cli_options(args)?;

    let CommandLine {
        cpu,
        cpu_features,
        target,
        output,
        btf,
        allow_bpf_trap,
        export_symbols,
        log_file,
        log_level,
        llvm_args,
        unroll_loops,
        ignore_inline_never,
        dump_module,
        dump_cfg_dir,
        sbpf_optimize,
        arch,
        disable_expand_memcpy_in_order,
        disable_memory_builtins,
        disable_math_builtins,
        inputs,
        export,
        fatal_errors,
        deploy,
        ..
    } = cli;

    let _guard = {
        let filter = EnvFilter::from_default_env();
        let filter = match log_level {
            None => filter,
            Some(log_level) => filter.add_directive(log_level.into()),
        };
        let subscriber_registry = tracing_subscriber::registry().with(filter);
        match log_file {
            Some((parent, file_name)) => {
                let file_appender =
                    tracing_appender::rolling::never(parent, file_name);
                let (non_blocking, guard) =
                    tracing_appender::non_blocking(file_appender);
                let subscriber = subscriber_registry
                    .with(tracing_layer(io::stdout))
                    .with(tracing_layer(non_blocking));
                tracing::subscriber::set_global_default(subscriber)?;
                Some(guard)
            }
            None => {
                let subscriber =
                    subscriber_registry.with(tracing_layer(io::stderr));
                tracing::subscriber::set_global_default(subscriber)?;
                None
            }
        }
    };

    info!("command line: {:?}", env::args().collect::<Vec<_>>().join(" "));

    let export_symbols = export_symbols.map(fs::read_to_string).transpose()?;
    let math_builtins =
        if disable_math_builtins { &[][..] } else { &MATH_BUILTINS[..] };
    let export_symbols = export_symbols
        .as_deref()
        .into_iter()
        .flat_map(str::lines)
        .chain(export.iter().map(String::as_str))
        .chain(math_builtins.iter().copied());

    let output_type = match *cli.emit.as_slice() {
        [] => unreachable!("emit has a default value"),
        [CliOutputType(output_type), ..] => output_type,
    };

    let optimize = match *cli.optimize.as_slice() {
        [] => unreachable!("optimize has a default value"),
        [.., CliOptLevel(optimize)] => optimize,
    };

    fn stack_frame_size_arg(arg: &CStr) -> Option<&str> {
        let arg = arg.to_str().ok()?;
        let arg = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-'))?;
        arg.strip_prefix("bpf-stack-size=")
    }
    let stack_frame_size_result = llvm_args
        .iter()
        .rev()
        .find_map(|arg| stack_frame_size_arg(arg))
        .ok_or(CliError::MissingStackFrameSize)
        .and_then(|arg| {
            arg.parse::<i32>()
                .ok()
                .filter(|size| *size > 0)
                .ok_or_else(|| CliError::InvalidStackFrameSize(arg.to_owned()))
        });

    let cpu_features = join_cpu_features(&cpu_features);
    let mut linker = Linker::new(LinkerOptions {
        target,
        cpu,
        cpu_features,
        optimize,
        unroll_loops,
        ignore_inline_never,
        llvm_args,
        disable_expand_memcpy_in_order,
        disable_memory_builtins,
        btf,
        allow_bpf_trap,
    });
    let stack_frame_size = stack_frame_size_result?;

    if let Some(path) = dump_module {
        linker.set_dump_module_path(path);
    }

    warn_if_builtins_missing(&inputs);

    let inputs =
        inputs.iter().map(|p| LinkerInput::new_from_file(p.as_path()));

    linker.link_to_file(inputs, &output, output_type, export_symbols)?;

    if fatal_errors && linker.has_errors() {
        return Err(anyhow::anyhow!(
            "LLVM issued diagnostic with error severity"
        ));
    }

    let program = fs::read(&output)?;
    let sbpf_optimization = if sbpf_optimize {
        match dump_cfg_dir {
            Some(dir) => OptimizationConfig::enabled().with_cfg_dump_dir(dir),
            None => OptimizationConfig::enabled(),
        }
    } else {
        OptimizationConfig::disabled()
    };
    let bytecode = link_program(
        &program,
        ProgramOptions::new(sbpf_optimization, arch.0, stack_frame_size),
    )
    .map_err(|errors| {
        anyhow::anyhow!("{}", SbpfLinkerError::format_errors(&errors))
    })?;

    let src_name =
        output.file_stem().and_then(|s| s.to_str()).unwrap_or("main");

    let output_path = output
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{src_name}.so"));
    fs::write(&output_path, &bytecode)
        .map_err(|e| CliError::ProgramWriteError { msg: e.to_string() })?;

    // Remove "lib" from the artifact and put it in target/deploy
    if deploy {
        let final_object = src_name.strip_prefix("lib").unwrap_or(src_name);
        let deploy_path = PathBuf::from("target").join("deploy");
        fs::create_dir_all(&deploy_path).map_err(|e| {
            CliError::ProgramWriteError {
                msg: format!("failed to create deploy directory: {e}"),
            }
        })?;
        let deploy_file = deploy_path.join(format!("{final_object}.so"));
        fs::write(&deploy_file, &bytecode).map_err(|e| {
            CliError::ProgramWriteError {
                msg: format!("failed to write deploy artifact: {e}"),
            }
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_input_args() {
        let args = [
            "sbpf-linker",
            "--export",
            "foo",
            "--export",
            "bar",
            "symbols.o",
            "rcgu.o",
            "-L",
            "target/debug/deps",
            "-L",
            "target/debug",
            "-L",
            "/home/foo/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu/lib",
            "-o",
            "/tmp/bin.s",
            "--target=bpf",
            "--emit=asm",
        ]
        .into_iter()
        .map(|s| s.to_string());
        let CommandLine {
            disable_expand_memcpy_in_order,
            disable_math_builtins,
            deploy,
            sbpf_optimize,
            arch,
            ..
        } = process_cli_options(args).unwrap();
        assert!(disable_expand_memcpy_in_order);
        assert!(!disable_math_builtins);
        assert!(deploy);
        assert!(sbpf_optimize);
        assert!(matches!(arch.0, SbpfArch::V3));
    }

    #[test]
    fn test_explicit_overrides_of_default_flags() {
        let args = [
            "sbpf-linker",
            "input.o",
            "-o",
            "/tmp/bin.so",
            "--emit=llvm-ir",
            "--deploy=false",
            "--fatal-errors=false",
            "--disable-expand-memcpy-in-order=false",
            "--sbpf-optimize=false",
        ]
        .into_iter()
        .map(|s| s.to_string());
        let CommandLine {
            emit,
            deploy,
            fatal_errors,
            disable_expand_memcpy_in_order,
            sbpf_optimize,
            output,
            ..
        } = process_cli_options(args).unwrap();

        assert_eq!(emit.len(), 1);
        assert!(matches!(emit[0], CliOutputType(OutputType::LlvmAssembly)));
        assert!(!deploy);
        assert!(!fatal_errors);
        assert!(!disable_expand_memcpy_in_order);
        assert!(!sbpf_optimize);
        assert_eq!(output, PathBuf::from("/tmp/bin.so"));
    }

    #[test]
    fn test_boolean_and_optional_flags() {
        let args = [
            "sbpf-linker",
            "input.o",
            "-o",
            "/tmp/bin.o",
            "--target=bpfel-unknown-none",
            "--btf",
            "--allow-bpf-trap",
            "--unroll-loops",
            "--ignore-inline-never",
            "--disable-memory-builtins",
            "--disable-math-builtins",
            "--log-level=debug",
            "--export-symbols=/tmp/exports.txt",
            "--dump-module=/tmp/module.ll",
        ]
        .into_iter()
        .map(|s| s.to_string());
        let CommandLine {
            target,
            btf,
            allow_bpf_trap,
            unroll_loops,
            ignore_inline_never,
            disable_memory_builtins,
            disable_math_builtins,
            log_level,
            export_symbols,
            dump_module,
            inputs,
            ..
        } = process_cli_options(args).unwrap();

        assert_eq!(
            target.as_deref().map(|t| t.to_bytes()),
            Some(b"bpfel-unknown-none".as_slice())
        );
        assert!(btf);
        assert!(allow_bpf_trap);
        assert!(unroll_loops);
        assert!(ignore_inline_never);
        assert!(disable_memory_builtins);
        assert!(disable_math_builtins);
        assert_eq!(log_level, Some(Level::DEBUG));
        assert_eq!(export_symbols, Some(PathBuf::from("/tmp/exports.txt")));
        assert_eq!(dump_module, Some(PathBuf::from("/tmp/module.ll")));
        assert_eq!(inputs, vec![PathBuf::from("input.o")]);
    }

    #[test]
    fn test_sbpf_v0_rejects_unsupported_cpu_arches() {
        for unsupported_cpu in ["probe", "v3", "v4"] {
            let args = vec![
                "sbpf-linker".to_string(),
                "input.o".to_string(),
                "-o".to_string(),
                "/tmp/bin.o".to_string(),
                "--arch=v0".to_string(),
                format!("--cpu={unsupported_cpu}"),
            ]
            .into_iter();

            let error = process_cli_options(args).unwrap_err();
            assert!(error.to_string().contains(
                "sBPF architecture `v0` only supports CPU architectures"
            ));
        }
    }

    #[test]
    fn test_hidden_flags_are_still_accepted() {
        let help = CommandLine::try_parse_from(["sbpf-linker", "--help"])
            .unwrap_err()
            .to_string();
        assert!(!help.contains("--arch"));
        assert!(!help.contains("--disable-memory-builtins"));
        assert!(!help.contains("--disable-math-builtins"));

        let cli = CommandLine::try_parse_from([
            "sbpf-linker",
            "input.o",
            "-o",
            "/tmp/bin.o",
            "--arch=v0",
        ])
        .unwrap();
        assert!(matches!(cli.arch.0, SbpfArch::V0));
    }

    #[test]
    fn test_repeated_cpu_features_are_joined_in_order() {
        let parse = |features: &[&str]| {
            let args = ["sbpf-linker", "input.o", "-o", "/tmp/bin.o"]
                .into_iter()
                .chain(features.iter().copied())
                .map(String::from);
            let CommandLine { cpu_features, .. } =
                process_cli_options(args).unwrap();
            join_cpu_features(&cpu_features)
        };

        assert_eq!(parse(&[]), CString::default());
        assert_eq!(
            parse(&["--cpu-features", "+alu32"]),
            CString::new("+alu32").unwrap()
        );
        // Rustc passes the value as a separate argument, `-C link-arg` usually
        // with `=`. A later setting of a feature overrides an earlier one in
        // LLVM, so the order is kept.
        assert_eq!(
            parse(&[
                "--cpu-features=+allows-misaligned-mem-access",
                "--cpu-features",
                "+alu32,-allows-misaligned-mem-access",
                "--cpu-features=",
            ]),
            CString::new(
                "+allows-misaligned-mem-access,+alu32,-allows-misaligned-mem-access"
            )
            .unwrap()
        );
    }
}
