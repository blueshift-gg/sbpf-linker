use sbpf_assembler::ast::AST;
use sbpf_assembler::astnode::{ASTNode, ROData};
use sbpf_assembler::instruction::Instruction;
use sbpf_assembler::lexer::{ImmediateValue, Token};
use sbpf_assembler::parser::ParseResult;
use sbpf_common::opcode::Opcode;

use object::RelocationTarget::Symbol;
use object::{File, Object as _, ObjectSection as _, ObjectSymbol as _};

use std::collections::HashMap;

use crate::SbpfLinkerError;
use crate::constants::{LDDW_INSTRUCTION_SIZE, STANDARD_INSTRUCTION_SIZE};

pub fn parse_bytecode(bytes: &[u8]) -> Result<ParseResult, SbpfLinkerError> {
    let mut ast = AST::new();

    let obj = File::parse(bytes)?;

    // Find rodata section - could be .rodata, .rodata.str1.1, etc.
    let ro_section = obj.sections().find(|s| {
        s.name().map(|name| name.starts_with(".rodata")).unwrap_or(false)
    });

    // Ensure there's only one .rodata section
    let rodata_count = obj
        .sections()
        .filter(|s| {
            s.name().map(|name| name.starts_with(".rodata")).unwrap_or(false)
        })
        .count();

    if rodata_count > 1 {
        return Err(SbpfLinkerError::InstructionParseError(
            "Multiple .rodata sections found".to_string(),
        ));
    }

    let mut rodata_table = HashMap::new();
    if let Some(ref ro_section) = ro_section {
        // Get rodata section data once and reuse
        let ro_data = ro_section.data().map_err(|e| {
            SbpfLinkerError::InstructionParseError(format!(
                "Failed to read .rodata section data: {}",
                e
            ))
        })?;

        // only handle symbols in the .rodata section for now
        let mut rodata_offset = 0;
        for symbol in obj.symbols() {
            if symbol.section_index() == Some(ro_section.index())
                && symbol.size() > 0
            {
                let symbol_name = symbol
                    .name()
                    .map_err(|e| {
                        SbpfLinkerError::InstructionParseError(format!(
                            "Failed to read symbol name: {}",
                            e
                        ))
                    })?
                    .to_owned();

                let mut bytes = Vec::with_capacity(symbol.size() as usize);
                for i in 0..symbol.size() {
                    bytes.push(ImmediateValue::Int(i64::from(
                        ro_data[(symbol.address() + i) as usize],
                    )));
                }

                ast.rodata_nodes.push(ASTNode::ROData {
                    rodata: ROData {
                        name: symbol_name.clone(),
                        args: vec![
                            Token::Directive(String::from("byte"), 0..1),
                            Token::VectorLiteral(bytes, 0..1),
                        ],
                        span: 0..1,
                    },
                    offset: rodata_offset,
                });
                rodata_table.insert(symbol.address(), symbol_name);
                rodata_offset += symbol.size();
            }
        }
        ast.set_rodata_size(rodata_offset);
    }

    for section in obj.sections() {
        if section.name() == Ok(".text") {
            // Get section data once and reuse
            let section_data = section.data().map_err(|e| {
                SbpfLinkerError::InstructionParseError(format!(
                    "Failed to read .text section data: {}",
                    e
                ))
            })?;

            // parse text section and build instruction nodes
            // lddw takes 16 bytes, other instructions take 8 bytes
            let mut offset = 0;
            while offset < section_data.len() {
                let node_len = match Opcode::from_u8(section_data[offset]) {
                    Some(Opcode::Lddw) => LDDW_INSTRUCTION_SIZE,
                    _ => STANDARD_INSTRUCTION_SIZE,
                };
                let node = &section_data[offset..offset + node_len];
                let instruction =
                    Instruction::from_bytes(node).map_err(|e| {
                        SbpfLinkerError::InstructionParseError(e.to_string())
                    })?;

                ast.nodes.push(ASTNode::Instruction {
                    instruction,
                    offset: offset as u64,
                });
                offset += node_len;
            }

            if let Some(ref ro_section) = ro_section {
                // handle relocations
                for rel in section.relocations() {
                    // only handle relocations for symbols in the .rodata section for now
                    let symbol = match rel.1.target() {
                        Symbol(sym) => {
                            obj.symbol_by_index(sym).map_err(|e| {
                                SbpfLinkerError::InstructionParseError(
                                    format!(
                                        "Failed to get symbol by index: {}",
                                        e
                                    ),
                                )
                            })?
                        }
                        _ => continue, // Skip non-symbol relocations
                    };

                    if symbol.section_index() == Some(ro_section.index()) {
                        // addend is not explicit in the relocation entry, but implicitly encoded
                        // as the immediate value of the instruction
                        let instruction = ast
                            .get_instruction_at_offset(rel.0)
                            .ok_or_else(|| {
                                SbpfLinkerError::InstructionParseError(
                                    format!(
                                        "No instruction found at offset {}",
                                        rel.0
                                    ),
                                )
                            })?;

                        let addend = match instruction.operands.last() {
                            Some(Token::ImmediateValue(
                                ImmediateValue::Int(val),
                                _,
                            )) => *val,
                            _ => 0,
                        };

                        // Replace the immediate value with the rodata labelA
                        let Some(ro_label) =
                            rodata_table.get(&(addend as u64))
                        else {
                            return Err(
                                SbpfLinkerError::InstructionParseError(
                                    format!(
                                        "Rodata label not found for addend {}",
                                        addend
                                    ),
                                ),
                            );
                        };

                        let node = ast.get_instruction_at_offset(rel.0)
                            .ok_or_else(|| {
                                SbpfLinkerError::InstructionParseError(
                                    format!("No instruction found at offset {} for patching", rel.0)
                                )
                            })?;
                        let last_idx = node.operands.len() - 1;
                        node.operands[last_idx] =
                            Token::Identifier(ro_label.clone(), 0..1);
                    }
                }
            } else if section.relocations().count() > 0 {
                return Err(SbpfLinkerError::InstructionParseError(
                    "Relocations found but no .rodata section".to_string(),
                ));
            }
            ast.set_text_size(section.size());
        }
    }

    ast.build_program()
        .map_err(|errors| SbpfLinkerError::BuildProgramError { errors })
}
