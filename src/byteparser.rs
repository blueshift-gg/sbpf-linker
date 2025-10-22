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
    assert!(rodata_count <= 1, "Multiple .rodata sections found");

    let mut rodata_table = HashMap::new();
    if let Some(ref ro_section) = ro_section {
        // only handle symbols in the .rodata section for now
        let mut rodata_offset = 0;
        for symbol in obj.symbols() {
            if symbol.section_index() == Some(ro_section.index())
                && symbol.size() > 0
            {
                let mut bytes = Vec::new();
                for i in 0..symbol.size() {
                    bytes.push(ImmediateValue::Int(i64::from(
                        ro_section.data().unwrap()
                            [(symbol.address() + i) as usize],
                    )));
                }
                ast.rodata_nodes.push(ASTNode::ROData {
                    rodata: ROData {
                        name: symbol.name().unwrap().to_owned(),
                        args: vec![
                            Token::Directive(String::from("byte"), 0..1), //
                            Token::VectorLiteral(bytes.clone(), 0..1),
                        ],
                        span: 0..1,
                    },
                    offset: rodata_offset,
                });
                rodata_table.insert(
                    symbol.address(),
                    symbol.name().unwrap().to_owned(),
                );
                rodata_offset += symbol.size();
            }
        }
        ast.set_rodata_size(rodata_offset);
    }

    for section in obj.sections() {
        if section.name() == Ok(".text") {
            // parse text section and build instruction nodes
            // lddw takes 16 bytes, other instructions take 8 bytes
            let mut offset = 0;
            while offset < section.data().unwrap().len() {
                let node_len =
                    match Opcode::from_u8(section.data().unwrap()[offset]) {
                        Some(Opcode::Lddw) => 16,
                        _ => 8,
                    };
                let node = &section.data().unwrap()[offset..offset + node_len];
                let instruction = Instruction::from_bytes(node);
                if let Err(error) = instruction {
                    return Err(SbpfLinkerError::InstructionParseError(
                        error.to_string(),
                    ));
                } else {
                    ast.nodes.push(ASTNode::Instruction {
                        instruction: instruction.unwrap(),
                        offset: offset as u64,
                    });
                }
                offset += node_len;
            }

            if let Some(ref ro_section) = ro_section {
                // handle relocations
                for rel in section.relocations() {
                    // only handle relocations for symbols in the .rodata section for now
                    let symbol = match rel.1.target() {
                        Symbol(sym) => Some(obj.symbol_by_index(sym).unwrap()),
                        _ => None,
                    };

                    if symbol.unwrap().section_index()
                        == Some(ro_section.index())
                    {
                        // addend is not explicit in the relocation entry, but implicitly encoded
                        // as the immediate value of the instruction
                        let addend = match ast
                            .get_instruction_at_offset(rel.0)
                            .unwrap()
                            .operands
                            .last()
                            .unwrap()
                            .clone()
                        {
                            Token::ImmediateValue(
                                ImmediateValue::Int(val),
                                _,
                            ) => val,
                            _ => 0,
                        };

                        // Replace the immediate value with the rodata label
                        let ro_label = &rodata_table[&(addend as u64)];
                        let ro_label_name = ro_label.clone();
                        let node: &mut Instruction =
                            ast.get_instruction_at_offset(rel.0).unwrap();
                        let last_idx = node.operands.len() - 1;
                        node.operands[last_idx] =
                            Token::Identifier(ro_label_name, 0..1);
                    }
                }
            } else if section.relocations().count() > 0 {
                panic!("Relocations found but no .rodata section");
            }
            ast.set_text_size(section.size());
        }
    }

    ast.build_program()
        .map_err(|errors| SbpfLinkerError::BuildProgramError { errors })
}
