#[cfg(feature = "rust-llvm")]
use aya_rustc_llvm_proxy as _;

pub mod byteparser;
pub mod fuse_args_stack;
use std::collections::HashSet;
use std::io;

use bpf_linker::LinkerError;
use byteparser::parse_bytecode;

use sbpf_assembler::{CompileError, Program};
pub use sbpf_assembler::{OptimizationConfig, SbpfArch};

#[derive(thiserror::Error, Debug)]
pub enum SbpfLinkerError {
    #[error("Error opening object file. Error detail: ({0}).")]
    ObjectFileOpenError(#[from] object::Error),
    #[error("Error reading object file. Error detail: ({0}).")]
    ObjectFileReadError(#[from] io::Error),
    #[error("Linker Error. Error detail: ({0}).")]
    LinkerError(#[from] LinkerError),
    #[error("LLVM issued diagnostic with error severity.")]
    LlvmDiagnosticError,
    #[error("Build Program Error. Error details: {errors:?}.")]
    BuildProgramError { errors: Vec<CompileError> },
    #[error(
        "Symbol {symbol} is missing. Export it with the `--export={symbol}` flag."
    )]
    MissingExportSymbol { symbol: String },
    #[error("Instruction Parse Error. Error detail: ({0}).")]
    InstructionParseError(String),
    #[error(
        "Unresolved section call relocation at section={section} abs_off={abs_off:#x} addend={addend}"
    )]
    UnresolvedSectionCallRelocation {
        section: String,
        abs_off: u64,
        addend: i64,
    },
    #[error(
        "Error handling lddw relocation in section={section} address={address:#x}: {detail}"
    )]
    LddwRelocationError { section: String, address: u64, detail: String },
    #[error(
        "Error handling rodata relocation in section={section} address={address:#x}: {detail}"
    )]
    RodataRelocationError { section: String, address: u64, detail: String },
}

impl SbpfLinkerError {
    pub fn from_compile_errors(errors: Vec<CompileError>) -> Vec<Self> {
        let mut linker_errors = Vec::new();

        let mut undefined_labels = HashSet::new();
        let mut other_errors = Vec::new();
        for error in errors {
            match &error {
                CompileError::UndefinedLabel { label, .. } => {
                    if undefined_labels.insert(label.clone()) {
                        linker_errors.push(Self::MissingExportSymbol {
                            symbol: label.clone(),
                        });
                    }
                }
                _ => other_errors.push(error),
            }
        }
        if !other_errors.is_empty() {
            linker_errors
                .push(Self::BuildProgramError { errors: other_errors });
        }

        linker_errors
    }

    pub fn format_errors(errors: &[Self]) -> String {
        errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n")
    }
}

#[derive(Debug, Clone)]
pub struct ProgramOptions {
    pub optimization: OptimizationConfig,
    pub arch: SbpfArch,
    pub stack_frame_size: i32,
}

impl ProgramOptions {
    pub fn new(
        optimization: OptimizationConfig,
        arch: SbpfArch,
        stack_frame_size: i32,
    ) -> Self {
        Self { optimization, arch, stack_frame_size }
    }
}

pub fn link_program(
    source: &[u8],
    options: ProgramOptions,
) -> Result<Vec<u8>, Vec<SbpfLinkerError>> {
    let parse_result = parse_bytecode(source, options)?;
    let program = Program::from_parse_result(parse_result, None);

    Ok(program.emit_bytecode())
}
