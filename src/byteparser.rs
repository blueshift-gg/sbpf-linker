use sbpf_assembler::Token;
use sbpf_assembler::ast::AST;
use sbpf_assembler::astnode::{ASTNode, Data, GlobalDecl, Label, ROData};
use sbpf_assembler::parser::ParseResult;
use sbpf_assembler::section::DebugSection;
use sbpf_common::{
    inst_param::Number, instruction::Instruction, opcode::Opcode,
};

use either::Either;
use object::RelocationTarget::Symbol;
use object::{File, Object as _, ObjectSection as _, ObjectSymbol as _};

use std::collections::HashMap;

use crate::{SbpfLinkerError, SbpfLinkerOptions};

fn symbol_name<'a, T>(symbol: &T) -> String
where
    T: object::ObjectSymbol<'a>,
{
    match symbol.name() {
        Ok(name) if !name.is_empty() => name.to_owned(),
        _ => "<anonymous>".to_owned(),
    }
}

fn relocation_symbol_name<'a, T>(
    obj: &'a File<'a>,
    symbol: &T,
    address: u64,
) -> String
where
    T: object::ObjectSymbol<'a>,
{
    let name = symbol_name(symbol);
    if name != "<anonymous>" {
        return name;
    }

    if let Some(section_index) = symbol.section_index()
        && let Some(named_symbol) = obj.symbols().find(|candidate| {
            candidate.section_index() == Some(section_index)
                && candidate.address() == address
                && matches!(candidate.name(), Ok(name) if !name.is_empty())
        })
    {
        return named_symbol.name().unwrap().to_owned();
    }

    name
}

fn is_writable_data_section(name: &str) -> bool {
    name == ".data" || name == ".bss"
}

pub fn parse_bytecode(
    bytes: &[u8],
    options: SbpfLinkerOptions,
) -> Result<ParseResult, SbpfLinkerError> {
    let mut ast = AST::new();

    let obj = File::parse(bytes)?;

    // Track all .rodata* inputs, but keep AST rodata layout packed by node size.
    let mut ro_sections = HashMap::new();
    let mut data_sections = HashMap::new();
    let mut bss_sections = HashMap::new();
    for section in obj.sections().filter(|section| {
        section.name().map(|name| name.starts_with(".rodata")).unwrap_or(false)
    }) {
        ro_sections.insert(section.index(), section);
    }
    for section in obj.sections().filter(|section| {
        section.name().map(|name| name == ".data").unwrap_or(false)
    }) {
        data_sections.insert(section.index(), section);
    }
    for section in obj.sections().filter(|section| {
        section.name().map(|name| name == ".bss").unwrap_or(false)
    }) {
        bss_sections.insert(section.index(), section);
    }

    let text_section = obj
        .sections()
        .find(|s| s.name().map(|name| name == ".text").unwrap_or(false));

    let mut relocated_symbol_table = HashMap::new();
    let mut rodata_offset = 0;
    let mut data_offset = 0;

    for symbol in obj.symbols() {
        if let Some(ro_section) = symbol
            .section_index()
            .and_then(|section_index| ro_sections.get(&section_index))
        {
            if symbol.size() == 0 {
                continue;
            }
            let mut bytes = Vec::new();
            for i in 0..symbol.size() {
                bytes.push(Number::Int(i64::from(
                    ro_section.data().unwrap()
                        [(symbol.address() + i) as usize],
                )));
            }
            ast.rodata_nodes.push(ASTNode::ROData {
                rodata: ROData {
                    name: symbol_name(&symbol),
                    args: vec![
                        Token::Directive(String::from("byte"), 0..1),
                        Token::VectorLiteral(bytes.clone(), 0..1),
                    ],
                    span: 0..1,
                },
                offset: rodata_offset,
            });
            relocated_symbol_table.insert(
                (symbol.section_index(), symbol.address()),
                symbol_name(&symbol),
            );
            rodata_offset += symbol.size();
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

    for data_section in data_sections.values() {
        let mut section_symbols: Vec<_> = obj
            .symbols()
            .filter(|symbol| {
                symbol.section_index() == Some(data_section.index())
                    && symbol.size() > 0
            })
            .collect();
        section_symbols.sort_by_key(|symbol| symbol.address());

        for symbol in section_symbols {
            let start = symbol.address() as usize;
            let end = start + symbol.size() as usize;
            ast.data_nodes.push(ASTNode::Data {
                data: Data::initialized(
                    symbol_name(&symbol),
                    data_section.data().unwrap()[start..end].to_vec(),
                    0..1,
                ),
                offset: data_offset,
            });
            relocated_symbol_table.insert(
                (symbol.section_index(), symbol.address()),
                symbol_name(&symbol),
            );
            data_offset += symbol.size();
        }
    }

    for bss_section in bss_sections.values() {
        let mut section_symbols: Vec<_> = obj
            .symbols()
            .filter(|symbol| {
                symbol.section_index() == Some(bss_section.index())
                    && symbol.size() > 0
            })
            .collect();
        section_symbols.sort_by_key(|symbol| symbol.address());

        for symbol in section_symbols {
            ast.data_nodes.push(ASTNode::Data {
                data: Data::zeroed(symbol_name(&symbol), symbol.size(), 0..1),
                offset: data_offset,
            });
            relocated_symbol_table.insert(
                (symbol.section_index(), symbol.address()),
                symbol_name(&symbol),
            );
            data_offset += symbol.size();
        }
    }

    let mut debug_sections = Vec::default();
    ast.set_rodata_size(rodata_offset);
    ast.set_data_size(data_offset);

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
                    let symbol_label = relocation_symbol_name(
                        &obj,
                        &symbol,
                        addend.try_into().unwrap_or(symbol.address()),
                    );
                    let symbol_section = symbol
                        .section_index()
                        .and_then(|index| obj.section_by_index(index).ok());
                    if let Some(symbol_section) = symbol_section.as_ref() {
                        let section_name =
                            symbol_section.name().unwrap_or("<unknown>");
                        if is_writable_data_section(section_name)
                            && !options.allow_writable_data
                        {
                            return Err(
                                SbpfLinkerError::UnsupportedWritableDataRelocation {
                                    symbol: symbol_label,
                                    section: section_name.to_owned(),
                                },
                            );
                        }
                    }

                    let key = (symbol.section_index(), addend as u64);
                    if let Some(label) = relocated_symbol_table.get(&key) {
                        node.imm = Some(Either::Left(label.clone()));
                    } else {
                        return Err(SbpfLinkerError::InstructionParseError(
                            format!(
                                "relocation in lddw does not resolve to a supported data symbol: `{}`",
                                symbol_label,
                            ),
                        ));
                    }
                } else if node.opcode == Opcode::Call {
                    node.imm = Some(Either::Left(symbol_name(&symbol)));
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
        .build_program(options.sbpf_version)
        .map_err(|errors| SbpfLinkerError::BuildProgramError { errors })?;

    parse_result.debug_sections = debug_sections;

    Ok(parse_result)
}

#[cfg(test)]
mod tests {
    use super::parse_bytecode;
    use crate::SbpfLinkerOptions;
    use either::Either;
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationEncoding,
        RelocationFlags, RelocationKind, SectionKind, SymbolFlags, SymbolKind,
        SymbolScope,
    };
    use sbpf_assembler::{SbpfArch, astnode::ASTNode};
    use sbpf_common::{
        inst_param::{Number, Register},
        instruction::Instruction,
        opcode::Opcode,
    };

    fn build_test_object_with_out_of_order_data_symbols() -> Vec<u8> {
        let mut object = Object::new(
            BinaryFormat::Elf,
            Architecture::Bpf,
            Endianness::Little,
        );
        let text_section = object.add_section(
            Vec::new(),
            b".text".to_vec(),
            SectionKind::Text,
        );
        let data_section = object.add_section(
            Vec::new(),
            b".data".to_vec(),
            SectionKind::Data,
        );

        let mut text_bytes = Instruction {
            opcode: Opcode::Lddw,
            dst: Some(Register { n: 1 }),
            src: None,
            off: None,
            imm: Some(Either::Right(Number::Int(4))),
            span: 0..1,
        }
        .to_bytes()
        .unwrap();
        text_bytes.extend(
            Instruction {
                opcode: Opcode::Exit,
                dst: None,
                src: None,
                off: None,
                imm: None,
                span: 0..1,
            }
            .to_bytes()
            .unwrap(),
        );
        object.append_section_data(text_section, &text_bytes, 8);
        object.append_section_data(
            data_section,
            &[0x11, 0x22, 0x33, 0x44, 0xaa, 0xbb, 0xcc, 0xdd],
            1,
        );

        object.add_symbol(Symbol {
            name: b"entrypoint".to_vec(),
            value: 0,
            size: text_bytes.len() as u64,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(text_section),
            flags: SymbolFlags::None,
        });

        // Intentionally insert symbols out of address order to make sure
        // writable data packing follows symbol addresses, not ELF symbol order.
        object.add_symbol(Symbol {
            name: b"bar".to_vec(),
            value: 4,
            size: 4,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(data_section),
            flags: SymbolFlags::None,
        });
        object.add_symbol(Symbol {
            name: b"foo".to_vec(),
            value: 0,
            size: 4,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(data_section),
            flags: SymbolFlags::None,
        });

        let data_section_symbol = object.section_symbol(data_section);
        object
            .add_relocation(
                text_section,
                Relocation {
                    offset: 0,
                    symbol: data_section_symbol,
                    addend: 0,
                    flags: RelocationFlags::Generic {
                        kind: RelocationKind::Absolute,
                        encoding: RelocationEncoding::Generic,
                        size: 64,
                    },
                },
            )
            .unwrap();

        object.write().unwrap()
    }

    #[test]
    fn test_parse_bytecode_packs_data_symbols_by_address() {
        let parse_result = parse_bytecode(
            &build_test_object_with_out_of_order_data_symbols(),
            SbpfLinkerOptions {
                allow_writable_data: true,
                sbpf_version: SbpfArch::V3,
            },
        )
        .unwrap();

        let data_nodes: Vec<_> = parse_result
            .data_section
            .get_nodes()
            .iter()
            .filter_map(|node| {
                if let ASTNode::Data { data, offset } = node {
                    Some((data.name.as_str(), *offset, data.bytes.clone()))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            data_nodes,
            vec![
                ("foo", 0, vec![0x11, 0x22, 0x33, 0x44]),
                ("bar", 4, vec![0xaa, 0xbb, 0xcc, 0xdd]),
            ]
        );

        let lddw_imm = parse_result
            .code_section
            .get_nodes()
            .iter()
            .find_map(|node| {
                if let ASTNode::Instruction { instruction, .. } = node
                    && instruction.opcode == Opcode::Lddw
                {
                    instruction.imm.clone()
                } else {
                    None
                }
            })
            .expect("missing lddw");
        assert_eq!(lddw_imm, Either::Right(Number::Addr(180)));
    }
}
