use sbpf_assembler::Token;
use sbpf_assembler::ast::AST;
use sbpf_assembler::astnode::{ASTNode, ROData};
use sbpf_assembler::parser::ParseResult;
use sbpf_assembler::section::DebugSection;
use sbpf_common::{
    inst_param::Number, instruction::Instruction, opcode::Opcode,
};

use either::Either;
use object::RelocationTarget::Symbol;
use object::{File, Object as _, ObjectSection as _, ObjectSymbol as _};

use std::collections::HashMap;

use crate::SbpfLinkerError;

pub fn parse_bytecode(bytes: &[u8]) -> Result<ParseResult, SbpfLinkerError> {
    let mut ast = AST::new();

    let obj = File::parse(bytes)?;

    // Find rodata section - could be .rodata, .rodata.str1.1, etc.
    let ro_sections = obj.sections().find(|s| {
        s.name().map(|name| name.starts_with(".rodata")).unwrap_or(false)
    });

    let mut rodata_table = HashMap::new();
    for ro_section in ro_sections.iter() {
        // only handle symbols in the .rodata section for now
        let mut rodata_offset = 0;
        for symbol in obj.symbols() {
            if symbol.section_index() == Some(ro_section.index())
                && symbol.size() > 0
            {
                let mut bytes = Vec::new();
                for i in 0..symbol.size() {
                    bytes.push(Number::Int(i64::from(
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
                    (symbol.section_index(), symbol.address()),
                    symbol.name().unwrap().to_owned(),
                );
                rodata_offset += symbol.size();
            }
        }
        ast.set_rodata_size(rodata_offset);
    }

    let mut debug_sections = Vec::default();
    for section in obj.sections() {
        if section.name() == Ok(".text") {
            // parse text section and build instruction nodes
            // lddw takes 16 bytes, other instructions take 8 bytes
            let mut offset = 0;
            while offset < section.data().unwrap().len() {
                let data = &section.data().unwrap()[offset..];
                let instruction = Instruction::from_bytes(data);
                if let Err(error) = instruction {
                    return Err(SbpfLinkerError::InstructionParseError(
                        error.to_string(),
                    ));
                }
                let node_len = match instruction.as_ref().unwrap().opcode {
                    Opcode::Lddw => 16,
                    _ => 8,
                };
                ast.nodes.push(ASTNode::Instruction {
                    instruction: instruction.unwrap(),
                    offset: offset as u64,
                });
                offset += node_len;
            }

            if ro_sections.iter().count() > 0 {
                // handle relocations
                for rel in section.relocations() {
                    // only handle relocations for symbols in the .rodata section for now
                    let symbol = match rel.1.target() {
                        Symbol(sym) => Some(obj.symbol_by_index(sym).unwrap()),
                        _ => None,
                    };
                    // addend is not explicit in the relocation entry, but implicitly encoded
                    // as the immediate value of the instruction
                    let addend = match ast
                        .get_instruction_at_offset(rel.0)
                        .unwrap()
                        .imm
                    {
                        Some(Either::Right(Number::Int(val))) => val,
                        _ => 0,
                    };

                    let key = (symbol.unwrap().section_index(), addend as u64);
                    if rodata_table.contains_key(&key) {
                        // Replace the immediate value with the rodata label
                        let ro_label = &rodata_table[&key];
                        let ro_label_name = ro_label.clone();
                        let node: &mut Instruction =
                            ast.get_instruction_at_offset(rel.0).unwrap();
                        node.imm = Some(Either::Left(ro_label_name));
                    }
                }
            } else if section.relocations().count() > 0 {
                panic!("Relocations found but no .rodata section");
            }
            ast.set_text_size(section.size());
        } else if let Ok(section_name) = section.name()
            && section_name.starts_with(".debug_")
        {
            // So we have debug sections, keep them around.
            debug_sections.push(DebugSection::new(
                section_name.into(),
                0, // will compute during emitting
                section.data().unwrap().to_vec(),
            ));
        }
    }

    let mut parse_result = ast
        .build_program(sbpf_assembler::SbpfArch::V0)
        .map_err(|errors| SbpfLinkerError::BuildProgramError { errors })?;

    parse_result.debug_sections = debug_sections;

    Ok(parse_result)
}
