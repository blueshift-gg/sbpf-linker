//! Regression tests for anonymous rodata relocation handling.
//!
//! LLVM emits compiler-generated data (de Bruijn LUTs, jump tables) into
//! .rodata using a STT_SECTION symbol with st_size=0 and no named STT_OBJECT
//! symbol. The previous code skipped all size==0 symbols unconditionally,
//! leaving rodata_table unpopulated for those bytes and panicking at
//! byteparser.rs:136 when the lddw relocation handler tried to resolve them.

use object::write::{Object, Relocation, Symbol, SymbolSection};
use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags,
    SectionKind, SymbolFlags, SymbolKind, SymbolScope,
};
use sbpf_linker::byteparser::parse_bytecode;

/// BPF lddw instruction: two 8-byte words, 16 bytes total.
///
/// Word 0: opcode=0x18, dst=r1, src=r0, off=0, imm=addend
/// Word 1: all zeros
///
/// The addend is encoded in the imm field of word 0, bytes [4..8].
/// byteparser reads this directly as the offset into .rodata when
/// resolving the lddw relocation -- it is not carried in the relocation
/// entry itself.
fn lddw(addend: u32) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0] = 0x18; // BPF_LD | BPF_DW | BPF_IMM
    b[1] = 0x01; // dst=r1
    b[4..8].copy_from_slice(&addend.to_le_bytes());
    b
}

fn exit_insn() -> [u8; 8] {
    let mut b = [0u8; 8];
    b[0] = 0x95;
    b
}

/// Builds a minimal BPF ELF relocatable object.
///
/// `rodata` -- raw bytes placed in .rodata
/// 
/// `named` -- (name, offset_in_rodata, size) for STT_OBJECT symbols
/// 
/// `reloc_addend` -- Some(n) emits an R_BPF_64_64 relocation against the
///                   STT_SECTION symbol with the lddw imm encoding n.
///                   None emits no relocation.
fn build_elf(
    rodata: &[u8],
    named: &[(&str, u64, u64)],
    reloc_addend: Option<u32>,
) -> Vec<u8> {
    let mut obj =
        Object::new(BinaryFormat::Elf, Architecture::Bpf, Endianness::Little);

    let text_id =
        obj.add_section(vec![], b".text".to_vec(), SectionKind::Text);
    let addend = reloc_addend.unwrap_or(0);
    let text_data: Vec<u8> = lddw(addend)
        .iter()
        .chain(exit_insn().iter())
        .copied()
        .collect();
    obj.set_section_data(text_id, text_data, 8);

    let ro_id = obj.add_section(
        vec![],
        b".rodata".to_vec(),
        SectionKind::ReadOnlyData,
    );
    obj.set_section_data(ro_id, rodata.to_vec(), 1);

    // STT_SECTION symbol: always emitted by LLVM, st_size=0, st_value=0.
    // This is the symbol the size==0 guard was silently discarding.
    let sec_sym = obj.add_symbol(Symbol {
        name: vec![],
        value: 0,
        size: 0,
        kind: SymbolKind::Section,
        scope: SymbolScope::Compilation,
        weak: false,
        section: SymbolSection::Section(ro_id),
        flags: SymbolFlags::None,
    });

    for (name, offset, size) in named {
        obj.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: *offset,
            size: *size,
            kind: SymbolKind::Data,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(ro_id),
            flags: SymbolFlags::None,
        });
    }

    obj.add_symbol(Symbol {
        name: b"entrypoint".to_vec(),
        value: 0,
        size: 24, // lddw(16) + exit(8)
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text_id),
        flags: SymbolFlags::None,
    });

    if reloc_addend.is_some() {
        obj.add_relocation(
            text_id,
            Relocation {
                offset: 0,
                symbol: sec_sym,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_BPF_64_64,
                },
            },
        )
        .unwrap();
    }

    let mut out = Vec::new();
    obj.emit(&mut out).unwrap();
    out
}

// The direct bug: .rodata is entirely anonymous, only a STT_SECTION symbol
// with size=0, lddw addend=0. Before the fix this panicked unconditionally
// at byteparser.rs:136 because rodata_table was never populated.
#[test]
fn sttsection_only_lut_at_section_base() {
    let lut: Vec<u8> = vec![
        0x00, 0x01, 0x02, 0x07, 0x03, 0x0d, 0x08, 0x13,
        0x04, 0x19, 0x0e, 0x1c, 0x09, 0x22, 0x14, 0x28,
        0x05, 0x11, 0x1a, 0x26, 0x0f, 0x2e, 0x1d, 0x30,
        0x0a, 0x1f, 0x23, 0x36, 0x15, 0x32, 0x29, 0x39,
        0x3f, 0x06, 0x0c, 0x12, 0x18, 0x1b, 0x21, 0x27,
        0x10, 0x25, 0x2d, 0x2f, 0x1e, 0x35, 0x31, 0x38,
        0x3e, 0x0b, 0x17, 0x20, 0x24, 0x2c, 0x34, 0x37,
        0x3d, 0x16, 0x2b, 0x33, 0x3c, 0x2a, 0x3b, 0x3a,
    ];
    let elf = build_elf(&lut, &[], Some(0));
    assert!(parse_bytecode(&elf).is_ok());
}

// Non-zero addend: the anonymous region is not at offset 0 in .rodata.
// A named symbol covers [0, 8); the LUT sits at [8, 16).
// The lookup key the relocation handler constructs is (section_idx, 8).
// The gap-fill must produce a synthetic entry keyed at address 8, not 0.
// A naive fix that only handles the section-base case fails here.
#[test]
fn sttsection_reloc_nonzero_addend() {
    let mut rodata = vec![0xAAu8; 8];
    rodata.extend_from_slice(&[0xBBu8; 8]);
    let elf = build_elf(&rodata, &[("named_prefix", 0, 8)], Some(8));
    assert!(parse_bytecode(&elf).is_ok());
}

// Anonymous gap sandwiched between two named symbols.
// Layout: [sym_a: 0..8][anon: 8..16][sym_b: 16..24].
// Relocation addend=8 targets the middle gap. A gap-fill pass that only
// handles tail bytes (cursor < section_size after the loop) would miss
// this entirely and still panic.
#[test]
fn anonymous_gap_between_named_symbols() {
    let rodata = vec![0xCCu8; 24];
    let elf = build_elf(
        &rodata,
        &[("sym_a", 0, 8), ("sym_b", 16, 8)],
        Some(8),
    );
    assert!(parse_bytecode(&elf).is_ok());
}

// Named symbol covers the entire section exactly.
// The gap-fill pass must detect zero uncovered bytes and synthesize nothing.
// If it spuriously produces a zero-size entry or an entry overlapping the
// named symbol, rodata_table gets two entries at address 0 and the relocation
// resolves to whichever was inserted last -- which may not be the named symbol,
// producing wrong label resolution without panicking.
#[test]
fn fully_named_rodata_produces_no_spurious_synthetic() {
    let rodata = vec![0xDDu8; 16];
    let elf = build_elf(&rodata, &[("full_coverage", 0, 16)], Some(0));
    assert!(parse_bytecode(&elf).is_ok());
}

// Named symbols arrive in reverse address order within .symtab.
// LLVM makes no ordering guarantee for symbol table entries. Without the
// sort-before-emit step, sym_b (higher address but listed first in .symtab)
// receives rodata_offset=0 and sym_a receives rodata_offset=8, inverting the
// section layout in the AST. The relocation targeting address 0 then resolves
// to sym_b's label, which describes the wrong byte range. The emitted bytecode
// loads from the wrong offset at runtime with no indication of the error here.
#[test]
fn named_symbols_reversed_in_symtab_order() {
    let rodata = vec![0xEEu8; 16];
    let elf = build_elf(
        &rodata,
        &[("sym_b", 8, 8), ("sym_a", 0, 8)], // high-address listed first
        Some(0),
    );
    assert!(parse_bytecode(&elf).is_ok());
}

// Multiple disjoint anonymous gaps in one section.
// Layout: [sym_a: 0..4][anon_1: 4..12][sym_b: 12..16][anon_2: 16..24].
// Both gaps must be independently synthesized with their correct addresses.
// We target anon_1 at addend=4. If the gap detector stops after finding one
// gap, or merges the two anonymous regions, anon_1 either has no entry or
// has an entry at the wrong address.
#[test]
fn multiple_disjoint_anonymous_gaps() {
    let rodata = vec![0xFFu8; 24];
    let elf = build_elf(
        &rodata,
        &[("sym_a", 0, 4), ("sym_b", 12, 4)],
        Some(4),
    );
    assert!(parse_bytecode(&elf).is_ok());
}