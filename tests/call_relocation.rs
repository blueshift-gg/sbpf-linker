
use object::write::{Object, Relocation, Symbol, SymbolSection};
use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind,
    SymbolFlags, SymbolKind, SymbolScope,
};
use sbpf_linker::link_program;

// BPF call instruction: opcode=0x85, src=0, dst=0, off=0, imm=addend
// imm encodes the addend -- byte offset of target within .text.unlikely.
fn call_insn(addend: i32) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[0] = 0x85;
    b[4..8].copy_from_slice(&addend.to_le_bytes());
    b
}

fn exit_insn() -> [u8; 8] {
    let mut b = [0u8; 8];
    b[0] = 0x95;
    b
}

// Minimal cold function bytes: mov r0, 99; exit
fn cold_fn_bytes() -> [u8; 16] {
    let mut b = [0u8; 16];
    // mov64 r0, 99
    b[0] = 0xb7;
    b[4..8].copy_from_slice(&99i32.to_le_bytes());
    // exit
    b[8] = 0x95;
    b
}

/// Builds an ELF where .text has a call instruction with an R_BPF_64_32
/// relocation targeting the STT_SECTION symbol for .text.unlikely., with
/// addend=target_offset pointing at the named cold function within that
/// section.
fn build_call_reloc_elf(cold_fn_name: &str, target_offset: i32) -> Vec<u8> {
    let mut obj =
        Object::new(BinaryFormat::Elf, Architecture::Bpf, Endianness::Little);

    let text_id =
        obj.add_section(vec![], b".text".to_vec(), SectionKind::Text);
    let unlikely_id = obj.add_section(
        vec![],
        b".text.unlikely.".to_vec(),
        SectionKind::Text,
    );

    // .text: call instruction + exit
    let text_data: Vec<u8> = call_insn(target_offset)
        .iter()
        .chain(exit_insn().iter())
        .copied()
        .collect();
    obj.set_section_data(text_id, text_data, 8);

    // .text.unlikely.: cold function bytes
    obj.set_section_data(unlikely_id, cold_fn_bytes().to_vec(), 8);

    // entrypoint symbol in .text
    obj.add_symbol(Symbol {
        name: b"entrypoint".to_vec(),
        value: 0,
        size: 16,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text_id),
        flags: SymbolFlags::None,
    });

    // Named STT_FUNC symbol for the cold function in .text.unlikely.
    obj.add_symbol(Symbol {
        name: cold_fn_name.as_bytes().to_vec(),
        value: target_offset as u64,
        size: cold_fn_bytes().len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(unlikely_id),
        flags: SymbolFlags::None,
    });

    // STT_SECTION symbol for .text.unlikely. -- this is what the
    // relocation entry references, not the named function symbol.
    let sec_sym = obj.section_symbol(unlikely_id);

    // R_BPF_64_32 relocation at offset 0 in .text (the call instruction)
    // targeting the STT_SECTION symbol for .text.unlikely.
    obj.add_relocation(
        text_id,
        Relocation {
            offset: 0,
            symbol: sec_sym,
            addend: 0,
            flags: RelocationFlags::Elf { r_type: object::elf::R_BPF_64_32 },
        },
    )
    .unwrap();

    let mut out = Vec::new();
    obj.emit(&mut out).unwrap();
    out
}

// The direct bug: call relocation targets STT_SECTION symbol for
// .text.unlikely. with addend=0. The named cold function sits at
// address 0 within .text.unlikely. Before the fix, symbol.name()
// returned "" and the assembler panicked with "Identifier '' should
// have been resolved earlier". After the fix, the secondary lookup
// finds the named symbol and resolves the call target correctly.
#[test]
fn call_reloc_sttsection_resolves_named_target() {
    let elf = build_call_reloc_elf("cold_target", 0);
    let result = link_program(&elf);
    assert!(
        result.is_ok(),
        "parse_bytecode panicked or errored: {:?}",
        result.err()
    );
}

// Non-zero addend: the cold function is not at the base of
// .text.unlikely. but at some offset within it. The secondary lookup
// must use the addend as the address, not hardcode 0.
// A fix that only handles addend=0 fails this test.
#[test]
fn call_reloc_sttsection_nonzero_addend() {
    let elf = build_call_reloc_elf("cold_target_offset", 8);
    let result = link_program(&elf);
    assert!(
        result.is_ok(),
        "parse_bytecode failed for non-zero addend: {:?}",
        result.err()
    );
}

// Named symbol call: relocation targets a named STT_FUNC symbol
// directly rather than a STT_SECTION symbol. The fix must not break
// this path -- the named symbol branch must still fire correctly.
#[test]
fn call_reloc_named_symbol_unaffected() {
    let mut obj =
        Object::new(BinaryFormat::Elf, Architecture::Bpf, Endianness::Little);

    let text_id =
        obj.add_section(vec![], b".text".to_vec(), SectionKind::Text);
    let text_data: Vec<u8> =
        call_insn(0).iter().chain(exit_insn().iter()).copied().collect();
    obj.set_section_data(text_id, text_data, 8);

    obj.add_symbol(Symbol {
        name: b"entrypoint".to_vec(),
        value: 0,
        size: 16,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text_id),
        flags: SymbolFlags::None,
    });

    // Named call target directly -- no STT_SECTION indirection.
    let named_sym = obj.add_symbol(Symbol {
        name: b"named_callee".to_vec(),
        value: 0,
        size: 8,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text_id),
        flags: SymbolFlags::None,
    });

    obj.add_relocation(
        text_id,
        Relocation {
            offset: 0,
            symbol: named_sym,
            addend: 0,
            flags: RelocationFlags::Elf { r_type: object::elf::R_BPF_64_32 },
        },
    )
    .unwrap();

    let mut out = Vec::new();
    obj.emit(&mut out).unwrap();

    let result = link_program(&out);
    assert!(
        result.is_ok(),
        "named symbol call relocation broke: {:?}",
        result.err()
    );
}

// No named symbol at the addend address: .text.unlikely. has a
// STT_SECTION symbol but no named STT_FUNC at the target address.
// The fix must leave the raw integer immediate in place rather than
// panicking or producing an empty label. This is the 0.1.8 fallback
// behavior for unresolvable call targets.
#[test]
fn call_reloc_sttsection_no_named_symbol_leaves_raw_imm() {
    let mut obj =
        Object::new(BinaryFormat::Elf, Architecture::Bpf, Endianness::Little);

    let text_id =
        obj.add_section(vec![], b".text".to_vec(), SectionKind::Text);
    let unlikely_id = obj.add_section(
        vec![],
        b".text.unlikely.".to_vec(),
        SectionKind::Text,
    );

    let text_data: Vec<u8> =
        call_insn(0).iter().chain(exit_insn().iter()).copied().collect();
    obj.set_section_data(text_id, text_data, 8);
    obj.set_section_data(unlikely_id, cold_fn_bytes().to_vec(), 8);

    obj.add_symbol(Symbol {
        name: b"entrypoint".to_vec(),
        value: 0,
        size: 16,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text_id),
        flags: SymbolFlags::None,
    });

    // Deliberately no named STT_FUNC symbol in .text.unlikely.
    // Only the STT_SECTION symbol exists.
    let sec_sym = obj.section_symbol(unlikely_id);

    obj.add_relocation(
        text_id,
        Relocation {
            offset: 0,
            symbol: sec_sym,
            addend: 0,
            flags: RelocationFlags::Elf { r_type: object::elf::R_BPF_64_32 },
        },
    )
    .unwrap();

    let mut out = Vec::new();
    obj.emit(&mut out).unwrap();

    // Must not panic -- raw integer fallback should keep parse_bytecode
    // returning Ok even when no named symbol is found at the target.
    let result = link_program(&out);
    assert!(
        result.is_ok(),
        "panicked when no named symbol found: {:?}",
        result.err()
    );
}
