pub mod byteparser;
use std::io;

use bpf_linker::LinkerError;
use byteparser::parse_bytecode;

use sbpf_assembler::{CompileError, Program, SbpfArch};

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
    #[error("Instruction Parse Error. Error detail: ({0}).")]
    InstructionParseError(String),
    #[error(
        "Relocation to writable section `{section}` for symbol `{symbol}` is disabled. Re-run with `--enable-writable-data` to allow it."
    )]
    UnsupportedWritableDataRelocation { symbol: String, section: String },
    #[error("Writable data sections (.data/.bss) require `--sbpf v3`.")]
    UnsupportedWritableDataVersion,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SbpfLinkerOptions {
    pub allow_writable_data: bool,
    pub sbpf_version: SbpfArch,
}

pub fn link_program(
    source: &[u8],
    options: SbpfLinkerOptions,
) -> Result<Vec<u8>, SbpfLinkerError> {
    let parse_result = parse_bytecode(source, options)?;
    if parse_result.data_section.get_size() > 0
        && !options.sbpf_version.is_v3()
    {
        return Err(SbpfLinkerError::UnsupportedWritableDataVersion);
    }
    let program = Program::from_parse_result(parse_result, None);
    let bytecode = program.emit_bytecode();

    Ok(bytecode)
}
