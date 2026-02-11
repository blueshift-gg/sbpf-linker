pub mod byteparser;
use std::io;

use bpf_linker::LinkerError;
use byteparser::parse_bytecode;

use sbpf_assembler::{CompileError, Program};

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
}

pub fn link_program(source: &[u8]) -> Result<Vec<u8>, SbpfLinkerError> {
    let parse_result = parse_bytecode(source)?;
    let program = Program::from_parse_result(parse_result, None);
    let bytecode = program.emit_bytecode();

    Ok(bytecode)
}
