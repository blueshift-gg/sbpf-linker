use sbpf_assembler::Token;
use sbpf_assembler::ast::AST;
use sbpf_assembler::astnode::{ASTNode, GlobalDecl, Label, ROData};
use sbpf_assembler::parser::ParseResult;
use sbpf_assembler::section::DebugSection;
use sbpf_common::{
    inst_param::Number, instruction::Instruction, opcode::Opcode,
};

use either::Either;
use object::RelocationTarget::Symbol;
use object::{
    File, Object as _, ObjectSection as _, ObjectSymbol as _, SectionIndex,
};

use std::collections::HashMap;

use crate::SbpfLinkerError;

// Staged rodata region. We collect these before emitting so we can sort by
// address and fill anonymous gaps before the AST is built.
struct RodataEntry {
    section_index: SectionIndex,
    address: u64,
    size: u64,
    name: String,
    bytes: Vec<Number>,
}

pub fn parse_bytecode(bytes: &[u8]) -> Result<ParseResult, SbpfLinkerError> {
    let mut ast = AST::new();

    let obj = File::parse(bytes)?;

    // Track all .rodata* inputs, but keep AST rodata layout packed by node size.
    let mut ro_sections = HashMap::new();
    for section in obj.sections().filter(|section| {
        section
            .name()
            .map(|name| name.starts_with(".rodata"))
            .unwrap_or(false)
    }) {
        ro_sections.insert(section.index(), section);
    }

    let text_section = obj
        .sections()
        .find(|s| s.name().map(|name| name == ".text").unwrap_or(false));

    let mut pending_rodata: Vec<RodataEntry> = Vec::new();
    let mut rodata_table: HashMap<(Option<SectionIndex>, u64), String> =
        HashMap::new();

    for symbol in obj.symbols() {
        if let Some(ro_section) = symbol
            .section_index()
            .and_then(|section_index| ro_sections.get(&section_index))
        {
            // STT_SECTION symbols have size == 0; anonymous gaps they cover
            // are handled by the gap-fill pass below.
            if symbol.size() == 0 {
                continue;
            }
            let bytes: Vec<Number> = (0..symbol.size())
                .map(|i| {
                    Number::Int(i64::from(
                        ro_section.data().unwrap()
                            [(symbol.address() + i) as usize],
                    ))
                })
                .collect();
            pending_rodata.push(RodataEntry {
                section_index: ro_section.index(),
                address: symbol.address(),
                size: symbol.size(),
                name: symbol.name().unwrap().to_owned(),
                bytes,
            });
        } else if let Some(_) = text_section
            .iter()
            .find(|s| symbol.section_index() == Some(s.index()))
        {
            ast.nodes.push(ASTNode::Label {
                label: Label {
                    name: symbol.name().unwrap().to_owned(),
                    span: 0..1,
                },
                offset: symbol.address(),
            });
            if symbol.name().unwrap() == "entrypoint" {
                ast.nodes.push(ASTNode::GlobalDecl {
                    global_decl: GlobalDecl {
                        entry_label: symbol.name().unwrap().to_owned(),
                        span: 0..1,
                    },
                });
            }
        }
    }

    // Gap-fill pass: synthesize rodata entries for byte ranges not covered by
    // any named symbol (e.g. compiler-generated lookup tables).
    let mut synthetic_rodata: Vec<RodataEntry> = Vec::new();
    for (section_index, ro_section) in &ro_sections {
        let section_data = ro_section.data().unwrap();
        let section_size = section_data.len() as u64;

        let mut section_entries: Vec<&RodataEntry> = pending_rodata
            .iter()
            .filter(|e| &e.section_index == section_index)
            .collect();
        section_entries.sort_by_key(|e| e.address);

        let mut cursor = 0u64;
        for entry in &section_entries {
            if cursor < entry.address {
                let gap_bytes: Vec<Number> = section_data
                    [cursor as usize..entry.address as usize]
                    .iter()
                    .map(|&b| Number::Int(i64::from(b)))
                    .collect();
                synthetic_rodata.push(RodataEntry {
                    section_index: *section_index,
                    address: cursor,
                    size: entry.address - cursor,
                    name: format!(
                        ".rodata.__anon_{:#x}_{:#x}",
                        section_index.0, cursor
                    ),
                    bytes: gap_bytes,
                });
            }
            cursor = cursor.max(entry.address + entry.size);
        }

        if cursor < section_size {
            let gap_bytes: Vec<Number> = section_data[cursor as usize..]
                .iter()
                .map(|&b| Number::Int(i64::from(b)))
                .collect();
            synthetic_rodata.push(RodataEntry {
                section_index: *section_index,
                address: cursor,
                size: section_size - cursor,
                name: format!(
                    ".rodata.__anon_{:#x}_{:#x}",
                    section_index.0, cursor
                ),
                bytes: gap_bytes,
            });
        }
    }

    pending_rodata.extend(synthetic_rodata);
    pending_rodata.sort_by_key(|e| (e.section_index.0, e.address));

    let mut rodata_offset = 0u64;
    for entry in pending_rodata {
        ast.rodata_nodes.push(ASTNode::ROData {
            rodata: ROData {
                name: entry.name.clone(),
                args: vec![
                    Token::Directive(String::from("byte"), 0..1),
                    Token::VectorLiteral(entry.bytes, 0..1),
                ],
                span: 0..1,
            },
            offset: rodata_offset,
        });
        rodata_table
            .insert((Some(entry.section_index), entry.address), entry.name);
        rodata_offset += entry.size;
    }

    let mut debug_sections = Vec::default();
    ast.set_rodata_size(rodata_offset);

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

            // handle relocations
            for rel in section.relocations() {
                // only handle relocations for symbols in the .rodata section for now
                let symbol = match rel.1.target() {
                    Symbol(sym) => obj.symbol_by_index(sym).unwrap(),
                    _ => continue,
                };

                let node: &mut Instruction =
                    ast.get_instruction_at_offset(rel.0).unwrap();

                if node.opcode == Opcode::Lddw {
                    // addend is not explicit in the relocation entry, but implicitly
                    // encoded as the immediate value of the instruction
                    let addend = match node.imm {
                        Some(Either::Right(Number::Int(val))) => val,
                        _ => 0,
                    };

                    let key = (
                        symbol.section_index(),
                        addend as u64,
                    );
                    if rodata_table.contains_key(&key) {
                        // Replace the immediate value with the rodata label
                        let ro_label = rodata_table[&key].clone();
                        node.imm = Some(Either::Left(ro_label));
                    } else {
                        panic!("relocation in lddw is not in .rodata");
                    }
                } else if node.opcode == Opcode::Call {
                    node.imm = Some(Either::Left(
                        symbol.name().unwrap().to_owned(),
                    ));
                }
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
