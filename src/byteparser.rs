use crate::{ProgramOptions, SbpfLinkerError};

use sbpf_assembler::ast::{AST, build_program};
use sbpf_assembler::astnode::{ASTNode, GlobalDecl, Label, ROData};
use sbpf_assembler::section::DebugSection;
use sbpf_assembler::{ProgramLayout, SbpfArch, Token};
use sbpf_common::{
    inst_param::Number, instruction::Instruction, opcode::Opcode,
};

use either::Either;
use object::RelocationTarget::Symbol;
use object::{
    File, Object as _, ObjectSection as _, ObjectSymbol as _, SectionIndex,
};

use std::collections::HashMap;

use crate::fuse_args_stack::{
    FunctionRange, diagnose_stack_arg_overlaps, rewrite_r11_stack_args,
};

fn decode_instruction_for_arch(
    data: &[u8],
    arch: SbpfArch,
) -> Result<Instruction, sbpf_common::errors::SBPFError> {
    if arch.is_v3() {
        Instruction::from_bytes_sbpf_v3(data)
    } else {
        Instruction::from_bytes(data)
    }
}

// Staged rodata region. We collect these before emitting so we can sort by
// address and fill anonymous gaps before the AST is built.
struct RodataEntry {
    section_index: SectionIndex,
    // Offset within the original input section
    address: u64,
    // Offset within the combined rodata section in the output
    address_out: u64,
    size: u64,
    name: String,
    bytes: Vec<Number>,
}

enum ResolvedTarget {
    Rodata { offset: u64 },
    Text { offset: u64 },
}

pub fn parse_bytecode(
    bytes: &[u8],
    options: ProgramOptions,
) -> Result<ProgramLayout, SbpfLinkerError> {
    let ProgramOptions {
        optimization,
        arch,
        stack_frame_size,
        stack_frame_gaps,
    } = options;
    let mut ast = AST::new();

    let obj = File::parse(bytes)?;

    // Track all read-only sections including .rodata* and .data.rel.ro* sections.
    // .data.rel.ro* is read-only after load-time pointer patching and can be
    // an lddw relocation target just like .rodata*.
    let mut ro_sections = HashMap::new();
    for section in obj.sections().filter(|section| {
        section
            .name()
            .map(|name| {
                name.starts_with(".rodata") || name.starts_with(".data.rel.ro")
            })
            .unwrap_or(false)
    }) {
        ro_sections.insert(section.index(), section);
    }

    let mut text_section_bases = HashMap::new();
    let mut text_size = 0u64;
    for section in obj.sections().filter(|section| {
        section.name().map(|name| name.starts_with(".text")).unwrap_or(false)
    }) {
        text_section_bases.insert(section.index(), text_size);
        text_size += section.size();
    }
    let mut pending_rodata: Vec<RodataEntry> = Vec::new();
    let mut rodata_table: HashMap<(Option<SectionIndex>, u64), String> =
        HashMap::new();

    let mut function_starts = Vec::new();
    for symbol in obj.symbols() {
        if let Some(ro_section) = symbol
            .section_index()
            .and_then(|section_index| ro_sections.get(&section_index))
        {
            // STT_SECTION symbols have size == 0; anonymous gaps they cover
            // are handled by the gap-fill pass below.
            if symbol.kind() == object::SymbolKind::Section {
                continue;
            }
            assert!(
                symbol.size() > 0,
                "non-STT_SECTION rodata symbol has size 0"
            );

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
                address_out: 0,
                size: symbol.size(),
                name: symbol.name().unwrap().to_owned(),
                bytes,
            });
        } else if let Some(section_index) = symbol.section_index()
            && let Some(section_base) = text_section_bases.get(&section_index)
        {
            let sym_name = symbol.name().unwrap_or("");
            if sym_name.is_empty() {
                continue;
            }
            ast.nodes.push(ASTNode::Label {
                label: Label { name: sym_name.to_owned(), span: 0..1 },
                offset: section_base + symbol.address(),
            });
            if symbol.kind() == object::SymbolKind::Text {
                ast.add_function_entry(sym_name.to_owned());
                function_starts.push((
                    section_base + symbol.address(),
                    sym_name.to_owned(),
                ));
            }
            if sym_name == "entrypoint" {
                ast.nodes.push(ASTNode::GlobalDecl {
                    global_decl: GlobalDecl {
                        entry_label: sym_name.to_owned(),
                        span: 0..1,
                    },
                });
            }
        }
    }

    // Mapping from offset to known labels
    let mut labels_by_offset: HashMap<u64, String> = HashMap::new();
    for node in &ast.nodes {
        if let ASTNode::Label { label, offset } = node {
            labels_by_offset
                .entry(*offset)
                .or_insert_with(|| label.name.clone());
        }
    }
    // Mapping from offset to synthetic labels
    let mut synthetic_labels_by_offset: HashMap<u64, String> = HashMap::new();

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
                    address_out: 0,
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
                address_out: 0,
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

    // Calculate each rodata entry's output offset.
    let mut rodata_size = 0u64;
    for entry in &mut pending_rodata {
        entry.address_out = rodata_size;
        rodata_size += entry.size;
    }

    // Function to resolve an input section address to it's offset in the output rodata.
    let resolve_rodata_output_offset =
        |section: SectionIndex, input_address: u64| {
            pending_rodata.iter().find_map(|entry| {
                (entry.section_index == section
                    && (entry.address..entry.address + entry.size)
                        .contains(&input_address))
                .then(|| entry.address_out + (input_address - entry.address))
            })
        };

    // Map each rodata relocation to its output offset and target label.
    let mut rodata_target_labels: HashMap<u64, String> = HashMap::new();
    for (section_index, ro_section) in &ro_sections {
        let section_name = ro_section.name().unwrap_or("<invalid>");
        let section_data = ro_section.data()?;
        for (relocation_address, rel) in ro_section.relocations() {
            let Symbol(symbol_index) = rel.target() else {
                return Err(SbpfLinkerError::RodataRelocationError {
                    section: section_name.to_owned(),
                    address: relocation_address,
                    detail: "invalid relocation target".to_owned(),
                });
            };
            let symbol = obj.symbol_by_index(symbol_index)?;
            let target_section = symbol.section_index().ok_or_else(|| {
                SbpfLinkerError::RodataRelocationError {
                    section: section_name.to_owned(),
                    address: relocation_address,
                    detail: "relocation target has no section".to_owned(),
                }
            })?;
            let addend = if rel.has_implicit_addend() {
                let stored = section_data
                    .get(
                        relocation_address as usize
                            ..relocation_address as usize + 8,
                    )
                    .ok_or_else(|| SbpfLinkerError::RodataRelocationError {
                        section: section_name.to_owned(),
                        address: relocation_address,
                        detail: "relocation location out of bounds".to_owned(),
                    })?;
                i64::from_le_bytes(stored.try_into().unwrap())
            } else {
                rel.addend()
            };
            let relocation_offset = resolve_rodata_output_offset(
                *section_index,
                relocation_address,
            )
            .ok_or_else(|| {
                SbpfLinkerError::RodataRelocationError {
                    section: section_name.to_owned(),
                    address: relocation_address,
                    detail: "relocation location is not rodata".to_owned(),
                }
            })?;

            let target = symbol.address().wrapping_add(addend as u64);

            // Find the relocation target (in rodata or text section).
            let resolved_target = if let Some(offset) =
                resolve_rodata_output_offset(target_section, target)
            {
                ResolvedTarget::Rodata { offset }
            } else {
                let text_base = text_section_bases
                    .get(&target_section)
                    .copied()
                    .ok_or_else(|| SbpfLinkerError::RodataRelocationError {
                        section: section_name.to_owned(),
                        address: relocation_address,
                        detail: "relocation target is not rodata or text"
                            .to_owned(),
                    })?;
                let offset = text_base
                    .checked_add(target)
                    .filter(|offset| *offset < text_size)
                    .ok_or_else(|| SbpfLinkerError::RodataRelocationError {
                        section: section_name.to_owned(),
                        address: relocation_address,
                        detail: "relocation target is not rodata or text"
                            .to_owned(),
                    })?;

                ResolvedTarget::Text { offset }
            };

            // Get or create a label for the target.
            let target_name = match resolved_target {
                ResolvedTarget::Rodata { offset } => {
                    let entry = pending_rodata
                        .iter()
                        .find(|entry| {
                            (entry.address_out..entry.address_out + entry.size)
                                .contains(&offset)
                        })
                        .ok_or_else(|| {
                            SbpfLinkerError::RodataRelocationError {
                                section: section_name.to_owned(),
                                address: relocation_address,
                                detail: "relocation target is not rodata"
                                    .to_owned(),
                            }
                        })?;

                    if entry.address_out == offset {
                        entry.name.clone()
                    } else if let Some(name) =
                        rodata_target_labels.get(&offset)
                    {
                        name.clone()
                    } else {
                        let name = format!(".rodata.__at__{offset:#x}");
                        ast.rodata_nodes.push(ASTNode::ROData {
                            rodata: ROData {
                                name: name.clone(),
                                args: vec![
                                    Token::Directive(
                                        String::from("byte"),
                                        0..1,
                                    ),
                                    Token::VectorLiteral(Vec::new(), 0..1),
                                ],
                                span: 0..1,
                            },
                            offset,
                        });
                        rodata_target_labels.insert(offset, name.clone());
                        name
                    }
                }
                ResolvedTarget::Text { offset } => {
                    if let Some(name) = labels_by_offset.get(&offset) {
                        name.clone()
                    } else {
                        let name = synthetic_labels_by_offset
                            .entry(offset)
                            .or_insert_with(|| {
                                format!(".text.__at__{offset:#x}")
                            })
                            .clone();
                        labels_by_offset.insert(offset, name.clone());
                        name
                    }
                }
            };

            // Add the relocation to the AST.
            ast.add_rodata_relocation(relocation_offset, target_name);
        }
    }

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
            offset: entry.address_out,
        });
        rodata_table
            .insert((Some(entry.section_index), entry.address), entry.name);
    }

    let mut debug_sections = Vec::default();
    ast.set_rodata_size(rodata_size);

    for section in obj.sections() {
        if let Some(section_base) = text_section_bases.get(&section.index()) {
            let section_base = *section_base;
            let section_data = section.data().unwrap();
            // parse text section and build instruction nodes
            // lddw takes 16 bytes, other instructions take 8 bytes
            let mut offset = 0;
            while offset < section_data.len() {
                let data = &section_data[offset..];
                let instruction = decode_instruction_for_arch(data, arch);
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
                    offset: section_base + offset as u64,
                });
                offset += node_len;
            }

            // handle relocations
            let section_name =
                section.name().unwrap_or("<invalid>").to_owned();
            for rel in section.relocations() {
                let rel_target = rel.1.target();
                let rel_addend = rel.1.addend();
                let rel_has_implicit_addend = rel.1.has_implicit_addend();

                // handle relocations for call targets and rodata referenced by lddw
                let symbol = match rel_target {
                    Symbol(sym) => obj.symbol_by_index(sym).unwrap(),
                    _ => continue,
                };

                let node: &mut Instruction = ast
                    .get_instruction_at_offset(section_base + rel.0)
                    .unwrap();

                if node.opcode == Opcode::Lddw {
                    // addend is not explicit in the relocation entry, but implicitly
                    // encoded as the immediate value of the instruction
                    let addend = match node.imm {
                        Some(Either::Right(Number::Int(val))) => val,
                        _ => 0,
                    };

                    let key = (symbol.section_index(), addend as u64);
                    if rodata_table.contains_key(&key) {
                        // Replace the immediate value with the rodata label
                        let ro_label = rodata_table[&key].clone();
                        node.imm = Some(Either::Left(ro_label));
                    } else {
                        panic!("relocation in lddw is not in .rodata");
                    }
                } else if node.opcode == Opcode::Call {
                    if symbol.kind() == object::SymbolKind::Section {
                        let addend_i64 = if rel_has_implicit_addend {
                            // If relocation uses implicit addend, use `node.imm`
                            match &node.imm {
                                Some(Either::Right(
                                    Number::Int(val) | Number::Addr(val),
                                )) => *val,
                                _ => rel_addend,
                            }
                        } else {
                            // Otherwise use explicit relocation addend
                            rel_addend
                        };

                        let target_section_base =
                            symbol.section_index().and_then(|idx| {
                                text_section_bases.get(&idx).copied()
                            });

                        let resolved_target_offset = target_section_base
                            .zip(addend_i64.checked_add(1))
                            .and_then(|(section_base, slots)| {
                                let slots = u64::try_from(slots).ok()?;
                                let local = slots
                                    .checked_mul(8)?
                                    .checked_add(symbol.address())?;
                                section_base.checked_add(local)
                            })
                            .filter(|target| *target < text_size);

                        let target_name = if let Some(target_offset) =
                            resolved_target_offset
                        {
                            if let Some(existing_name) =
                                labels_by_offset.get(&target_offset)
                            {
                                // Use known label
                                existing_name.clone()
                            } else {
                                // If label is not known, create and use a synthetic label
                                let synthetic_name =
                                    synthetic_labels_by_offset
                                        .entry(target_offset)
                                        .or_insert_with(|| {
                                            format!(
                                                ".__sbpf_section_call_{target_offset:x}"
                                            )
                                        })
                                        .clone();
                                labels_by_offset.insert(
                                    target_offset,
                                    synthetic_name.clone(),
                                );
                                synthetic_name
                            }
                        } else {
                            return Err(
                                SbpfLinkerError::UnresolvedSectionCallRelocation {
                                    section: section_name.clone(),
                                    abs_off: section_base + rel.0,
                                    addend: addend_i64,
                                },
                            );
                        };

                        node.imm = Some(Either::Left(target_name));
                    } else {
                        let name = symbol.name().unwrap_or("");
                        assert!(
                            !name.is_empty(),
                            "non-STT_SECTION call target has empty name"
                        );
                        node.imm = Some(Either::Left(name.to_owned()));
                    }
                }
            }
        } else if let Ok(section_name) = section.name()
            && section_name.starts_with(".debug_")
        {
            // So we have debug sections, keep them around.
            debug_sections.push(DebugSection::new(
                section_name,
                0, // will compute during emitting
                section.data().unwrap().to_vec(),
            ));
        }
    }

    if !synthetic_labels_by_offset.is_empty() {
        // Add synthetic labels to AST
        let mut synthetic_labels =
            synthetic_labels_by_offset.into_iter().collect::<Vec<_>>();
        synthetic_labels.sort_by_key(|(offset, _)| *offset);
        for (offset, name) in synthetic_labels {
            ast.nodes.push(ASTNode::Label {
                label: Label { name, span: 0..1 },
                offset,
            });
        }
    }

    ast.set_text_size(text_size);

    // Sort ast.nodes in source order: each label immediately before the
    // instruction at the same byte offset. The CFG builder expects source-order
    // input and no longer sorts internally. Non-label/instruction nodes
    // (GlobalDecl, etc.) are kept at the front in their original order.
    {
        let (mut metadata, mut text): (Vec<ASTNode>, Vec<ASTNode>) =
            std::mem::take(&mut ast.nodes).into_iter().partition(|n| {
                !matches!(
                    n,
                    ASTNode::Label { .. } | ASTNode::Instruction { .. }
                )
            });
        text.sort_by_key(|node| match node {
            ASTNode::Label { offset, .. } => (*offset, 0u8),
            ASTNode::Instruction { offset, .. } => (*offset, 1u8),
            _ => unreachable!(),
        });
        metadata.append(&mut text);
        ast.nodes = metadata;
    }

    function_starts.sort_by_key(|(start, _)| *start);
    // Function aliases can produce multiple STT_FUNC symbols at the same
    // address. Keep one entry per address when constructing function ranges.
    function_starts.dedup_by_key(|(start, _)| *start);
    let functions = function_starts
        .iter()
        .enumerate()
        .map(|(index, (start, name))| FunctionRange {
            name: name.clone(),
            start: *start,
            end: function_starts
                .get(index + 1)
                .map_or(text_size, |(next_start, _)| *next_start),
        })
        .collect::<Vec<_>>();

    for overlap in
        diagnose_stack_arg_overlaps(&ast, stack_frame_size, &functions)
    {
        tracing::error!(
            function = %overlap.function,
            local_start = overlap.local_stack.start,
            local_end = overlap.local_stack.end,
            argument_start = overlap.incoming_args.start,
            argument_end = overlap.incoming_args.end,
            "local stack variable overlaps incoming spilled-argument region"
        );
    }

    rewrite_r11_stack_args(&mut ast, stack_frame_size, stack_frame_gaps)
        .map_err(|errors| SbpfLinkerError::BuildProgramError { errors })?;

    let mut parse_result = build_program(ast, arch, optimization)
        .map_err(|errors| SbpfLinkerError::BuildProgramError { errors })?;

    parse_result.debug_sections = debug_sections;

    Ok(parse_result)
}
