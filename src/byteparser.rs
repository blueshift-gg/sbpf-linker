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

const BPF_TRAP: &str = "__bpf_trap";
const ABORT_SYSCALL: &str = "abort";

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

pub fn parse_bytecode(
    bytes: &[u8],
    options: ProgramOptions,
) -> Result<ProgramLayout, Vec<SbpfLinkerError>> {
    let ProgramOptions { optimization, arch, stack_frame_size } = options;
    let mut ast = AST::new();
    let mut errors: Vec<SbpfLinkerError> = Vec::new();

    let obj = File::parse(bytes).map_err(|error| vec![error.into()])?;

    // Track all read-only sections including .rodata* and .data.rel.ro* sections.
    // .data.rel.ro* is read-only after load-time pointer patching and can be
    // an lddw relocation target just like .rodata*.
    let ro_sections: HashMap<_, _> = obj
        .sections()
        .filter(|section| {
            section.name().is_ok_and(|name| {
                name.starts_with(".rodata") || name.starts_with(".data.rel.ro")
            })
        })
        .map(|section| (section.index(), section))
        .collect();

    let mut text_section_bases = HashMap::new();
    let mut text_size = 0u64;
    for section in obj.sections().filter(|section| {
        section.name().is_ok_and(|name| name.starts_with(".text"))
    }) {
        text_section_bases.insert(section.index(), text_size);
        text_size += section.size();
    }
    let mut pending_rodata: Vec<RodataEntry> = Vec::new();

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

            let data = ro_section.data().unwrap();
            let bytes: Vec<Number> = (0..symbol.size())
                .map(|i| {
                    Number::Int(i64::from(
                        data[(symbol.address() + i) as usize],
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

        let gap_entry = |start: u64, end: u64| RodataEntry {
            section_index: *section_index,
            address: start,
            address_out: 0,
            size: end - start,
            name: format!(".rodata.__anon_{:#x}_{start:#x}", section_index.0),
            bytes: section_data[start as usize..end as usize]
                .iter()
                .map(|&b| Number::Int(i64::from(b)))
                .collect(),
        };

        let mut cursor = 0u64;
        for entry in &section_entries {
            if cursor < entry.address {
                synthetic_rodata.push(gap_entry(cursor, entry.address));
            }
            cursor = cursor.max(entry.address + entry.size);
        }

        if cursor < section_size {
            synthetic_rodata.push(gap_entry(cursor, section_size));
        }
    }

    pending_rodata.extend(synthetic_rodata);
    pending_rodata.sort_by_key(|e| (e.section_index.0, e.address));

    // Calculate each rodata section's aligned output offset.
    let mut rodata_size = 0u64;
    let mut previous_entry: Option<&mut RodataEntry> = None;

    for entries in
        pending_rodata.chunk_by_mut(|a, b| a.section_index == b.section_index)
    {
        let section = &ro_sections[&entries[0].section_index];
        let section_base =
            rodata_size.next_multiple_of(section.align().max(1));

        // Check if padding bytes are required before the next rodata section and apply if needed.
        let padding = (section_base - rodata_size) as usize;
        if let Some(previous) = previous_entry.take()
            && padding > 0
        {
            previous
                .bytes
                .resize(previous.bytes.len() + padding, Number::Int(0));
        }

        for entry in &mut *entries {
            entry.address_out = section_base + entry.address;
        }

        rodata_size = section_base + section.size();
        previous_entry = entries.last_mut();
    }

    // Map each rodata relocation to its output offset and target label.
    let mut rodata_target_labels: HashMap<u64, String> = HashMap::new();
    let mut rodata_target_nodes = Vec::new();
    for (section_index, ro_section) in &ro_sections {
        let section_name = ro_section.name().unwrap_or("<invalid>");
        let section_data =
            ro_section.data().map_err(|error| vec![error.into()])?;
        for (relocation_address, rel) in ro_section.relocations() {
            let relocation_error =
                |detail: &str| SbpfLinkerError::RodataRelocationError {
                    section: section_name.to_owned(),
                    address: relocation_address,
                    detail: detail.to_owned(),
                };

            let result: Result<(), SbpfLinkerError> = (|| {
                let Symbol(symbol_index) = rel.target() else {
                    return Err(relocation_error("invalid relocation target"));
                };
                let symbol = obj.symbol_by_index(symbol_index)?;
                let target_section =
                    symbol.section_index().ok_or_else(|| {
                        relocation_error("relocation target has no section")
                    })?;
                let addend = if rel.has_implicit_addend() {
                    let stored = section_data
                        .get(
                            relocation_address as usize
                                ..relocation_address as usize + 8,
                        )
                        .ok_or_else(|| {
                            relocation_error(
                                "relocation location out of bounds",
                            )
                        })?;
                    i64::from_le_bytes(stored.try_into().unwrap())
                } else {
                    rel.addend()
                };
                let relocation_offset = find_rodata_entry(
                    &pending_rodata,
                    *section_index,
                    relocation_address,
                )
                .map(|entry| {
                    entry.address_out + (relocation_address - entry.address)
                })
                .ok_or_else(|| {
                    relocation_error("relocation location is not rodata")
                })?;

                let target = symbol.address().wrapping_add(addend as u64);

                let target_name = resolve_rodata_label(
                    target_section,
                    target,
                    &pending_rodata,
                    &mut rodata_target_labels,
                    &mut rodata_target_nodes,
                )
                .or_else(|| {
                    resolve_text_label(
                        target_section,
                        target,
                        &text_section_bases,
                        text_size,
                        &mut labels_by_offset,
                        &mut synthetic_labels_by_offset,
                    )
                })
                .ok_or_else(|| {
                    relocation_error("relocation target is not rodata or text")
                })?;

                // Add the relocation to the AST.
                ast.add_rodata_relocation(relocation_offset, target_name);
                Ok(())
            })();
            if let Err(error) = result {
                errors.push(error);
            }
        }
    }

    let mut debug_sections = Vec::default();
    ast.set_rodata_size(rodata_size);

    for section in obj.sections() {
        if let Some(&section_base) = text_section_bases.get(&section.index()) {
            let section_data = section.data().unwrap();
            // parse text section and build instruction nodes
            // lddw takes 16 bytes, other instructions take 8 bytes
            let mut offset = 0;
            while offset < section_data.len() {
                let data = &section_data[offset..];
                match decode_instruction_for_arch(data, arch) {
                    Ok(instruction) => {
                        let node_len = match instruction.opcode {
                            Opcode::Lddw => 16,
                            _ => 8,
                        };
                        ast.nodes.push(ASTNode::Instruction {
                            instruction,
                            offset: section_base + offset as u64,
                        });
                        offset += node_len;
                    }
                    Err(error) => {
                        errors.push(SbpfLinkerError::InstructionParseError(
                            error.to_string(),
                        ));
                        return Err(errors);
                    }
                }
            }

            // handle relocations
            let section_name = section.name().unwrap_or("<invalid>");
            for (rel_offset, rel) in section.relocations() {
                let result: Result<(), SbpfLinkerError> = (|| {
                    // handle relocations for call targets and rodata referenced by lddw
                    let Symbol(sym) = rel.target() else {
                        return Ok(());
                    };
                    let symbol = obj.symbol_by_index(sym)?;

                    let Some(node) = ast
                        .get_instruction_at_offset(section_base + rel_offset)
                    else {
                        return Err(SbpfLinkerError::InstructionParseError(
                            format!(
                                "relocation at offset {rel_offset:#x} in section {section_name} does not point at an instruction"
                            ),
                        ));
                    };

                    // An implicit addend is stored in the instruction immediate.
                    let addend = match node.imm {
                        Some(Either::Right(
                            Number::Int(val) | Number::Addr(val),
                        )) if rel.has_implicit_addend() => val,
                        _ => rel.addend(),
                    };

                    if node.opcode == Opcode::Lddw {
                        let relocation_error = |detail: &str| {
                            SbpfLinkerError::LddwRelocationError {
                                section: section_name.to_owned(),
                                address: rel_offset,
                                detail: detail.to_owned(),
                            }
                        };

                        let target_section =
                            symbol.section_index().ok_or_else(|| {
                                relocation_error(
                                    "relocation target has no section",
                                )
                            })?;
                        let target = symbol
                            .address()
                            .checked_add_signed(addend)
                            .ok_or_else(|| {
                                relocation_error("relocation target overflow")
                            })?;

                        let target_name = resolve_rodata_label(
                            target_section,
                            target,
                            &pending_rodata,
                            &mut rodata_target_labels,
                            &mut rodata_target_nodes,
                        )
                        .or_else(|| {
                            resolve_text_label(
                                target_section,
                                target,
                                &text_section_bases,
                                text_size,
                                &mut labels_by_offset,
                                &mut synthetic_labels_by_offset,
                            )
                        })
                        .ok_or_else(|| {
                            relocation_error(
                                "relocation target is not rodata or text",
                            )
                        })?;
                        // Replace the immediate value with the rodata label
                        node.imm = Some(Either::Left(target_name));
                    } else if node.opcode == Opcode::Call {
                        if symbol.kind() == object::SymbolKind::Section {
                            let target_section_base =
                                symbol.section_index().and_then(|idx| {
                                    text_section_bases.get(&idx).copied()
                                });

                            let resolved_target_offset = target_section_base
                                .zip(addend.checked_add(1))
                                .and_then(|(section_base, slots)| {
                                    let slots = u64::try_from(slots).ok()?;
                                    let local = slots
                                        .checked_mul(8)?
                                        .checked_add(symbol.address())?;
                                    section_base.checked_add(local)
                                })
                                .filter(|target| *target < text_size);

                            let Some(target_offset) = resolved_target_offset
                            else {
                                return Err(
                                    SbpfLinkerError::UnresolvedSectionCallRelocation {
                                        section: section_name.to_owned(),
                                        abs_off: section_base + rel_offset,
                                        addend,
                                    },
                                );
                            };
                            let target_name = text_label_at(
                                target_offset,
                                &mut labels_by_offset,
                                &mut synthetic_labels_by_offset,
                                || {
                                    format!(
                                        ".__sbpf_section_call_{target_offset:x}"
                                    )
                                },
                            );

                            node.imm = Some(Either::Left(target_name));
                        } else {
                            let name = symbol.name().unwrap_or("");
                            assert!(
                                !name.is_empty(),
                                "non-STT_SECTION call target has empty name"
                            );
                            // LLVM lowers `llvm.trap` to a call to the
                            // kernel kfunc `__bpf_trap`, which SBPF does not
                            // provide. Unless the program defines the symbol
                            // itself, bind it to the `abort` syscall.
                            let name =
                                if name == BPF_TRAP && symbol.is_undefined() {
                                    ABORT_SYSCALL
                                } else {
                                    name
                                };
                            node.imm = Some(Either::Left(name.to_owned()));
                        }
                    }
                    Ok(())
                })();

                if let Err(error) = result {
                    errors.push(error);
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

    ast.rodata_nodes.extend(rodata_target_nodes);
    for entry in pending_rodata {
        ast.rodata_nodes.push(ASTNode::ROData {
            rodata: ROData {
                name: entry.name,
                args: vec![
                    Token::Directive(String::from("byte"), 0..1),
                    Token::VectorLiteral(entry.bytes, 0..1),
                ],
                span: 0..1,
            },
            offset: entry.address_out,
        });
    }

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
        errors.push(SbpfLinkerError::StackArgOverlap {
            function: overlap.function,
            local_start: overlap.local_stack.start,
            local_end: overlap.local_stack.end,
            argument_start: overlap.incoming_args.start,
            argument_end: overlap.incoming_args.end,
        });
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let mut compile_errors =
        rewrite_r11_stack_args(&mut ast, stack_frame_size)
            .err()
            .unwrap_or_default();

    match build_program(ast, arch, optimization) {
        Ok(mut parse_result) => {
            if compile_errors.is_empty() {
                parse_result.debug_sections = debug_sections;
                Ok(parse_result)
            } else {
                Err(SbpfLinkerError::from_compile_errors(compile_errors))
            }
        }
        Err(errors) => {
            compile_errors.extend(errors);
            Err(SbpfLinkerError::from_compile_errors(compile_errors))
        }
    }
}

fn find_rodata_entry(
    entries: &[RodataEntry],
    section: SectionIndex,
    address: u64,
) -> Option<&RodataEntry> {
    entries.iter().find(|entry| {
        entry.section_index == section
            && (entry.address..entry.address + entry.size).contains(&address)
    })
}

fn resolve_rodata_label(
    section: SectionIndex,
    address: u64,
    entries: &[RodataEntry],
    labels: &mut HashMap<u64, String>,
    nodes: &mut Vec<ASTNode>,
) -> Option<String> {
    let entry = find_rodata_entry(entries, section, address)?;
    let offset = entry.address_out + address - entry.address;
    if offset == entry.address_out {
        return Some(entry.name.clone());
    }
    if let Some(name) = labels.get(&offset) {
        return Some(name.clone());
    }

    let name = format!(".rodata.__at__{offset:#x}");
    nodes.push(ASTNode::ROData {
        rodata: ROData {
            name: name.clone(),
            args: vec![
                Token::Directive(String::from("byte"), 0..1),
                Token::VectorLiteral(Vec::new(), 0..1),
            ],
            span: 0..1,
        },
        offset,
    });
    labels.insert(offset, name.clone());
    Some(name)
}

fn resolve_text_label(
    section: SectionIndex,
    address: u64,
    section_bases: &HashMap<SectionIndex, u64>,
    text_size: u64,
    labels: &mut HashMap<u64, String>,
    synthetic_labels: &mut HashMap<u64, String>,
) -> Option<String> {
    let offset = section_bases
        .get(&section)?
        .checked_add(address)
        .filter(|offset| *offset < text_size)?;
    Some(text_label_at(offset, labels, synthetic_labels, || {
        format!(".text.__at__{offset:#x}")
    }))
}

/// Returns the label at text `offset`, synthesizing one if none exists.
fn text_label_at(
    offset: u64,
    labels: &mut HashMap<u64, String>,
    synthetic_labels: &mut HashMap<u64, String>,
    synthetic_name: impl FnOnce() -> String,
) -> String {
    if let Some(name) = labels.get(&offset) {
        return name.clone();
    }

    let name =
        synthetic_labels.entry(offset).or_insert_with(synthetic_name).clone();
    labels.insert(offset, name.clone());
    name
}
