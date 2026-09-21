use object::write::{Object, Relocation, SectionId, Symbol, SymbolSection};
use object::{
    Architecture, BinaryFormat, Endianness, RelocationEncoding,
    RelocationFlags, RelocationKind, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
};
use sbpf_assembler::{OptimizationConfig, SbpfArch, astnode::ASTNode};
use sbpf_linker::{
    ProgramOptions, SbpfLinkerError, byteparser::parse_bytecode,
};

const STACK_FRAME_SIZE: i32 = 4096;
const EXIT_INSTRUCTION: [u8; 8] = [0x95, 0, 0, 0, 0, 0, 0, 0];

#[test]
fn rodata_relocation_rejects_undefined_target() {
    let (mut object, rodata) = create_object_with_rodata(8);
    let undefined = object.add_symbol(Symbol {
        name: b"undefined".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Data,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    add_rodata_relocation(&mut object, rodata, 0, undefined, 0);

    assert_rodata_relocation_error(
        object,
        0,
        "relocation target has no section",
    );
}

#[test]
fn rodata_relocation_rejects_target_outside_rodata_and_text() {
    let (mut object, rodata) = create_object_with_rodata(8);
    let data = object.add_section(
        Vec::new(),
        b".data.test".to_vec(),
        SectionKind::Data,
    );
    object.append_section_data(data, &[0; 8], 8);
    let target = object.add_symbol(Symbol {
        name: b"data_target".to_vec(),
        value: 0,
        size: 8,
        kind: SymbolKind::Data,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(data),
        flags: SymbolFlags::None,
    });
    add_rodata_relocation(&mut object, rodata, 0, target, 0);

    assert_rodata_relocation_error(
        object,
        0,
        "relocation target is not rodata or text",
    );
}

#[test]
fn rodata_relocation_rejects_location_out_of_bounds() {
    let (mut object, rodata) = create_object_with_rodata(8);
    let rodata_section = object.section_symbol(rodata);
    add_rodata_relocation(&mut object, rodata, 8, rodata_section, 0);

    assert_rodata_relocation_error(
        object,
        8,
        "relocation location out of bounds",
    );
}

#[test]
fn rodata_relocation_handles_interior_rodata_target() {
    let (mut object, rodata) = create_object_with_rodata(24);
    object.add_symbol(Symbol {
        name: b"data".to_vec(),
        value: 0,
        size: 24,
        kind: SymbolKind::Data,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(rodata),
        flags: SymbolFlags::None,
    });
    let rodata_section = object.section_symbol(rodata);
    for offset in [0, 8] {
        add_rodata_relocation(&mut object, rodata, offset, rodata_section, 16);
    }

    let program =
        parse_object(object).expect("valid rodata relocations should parse");
    let interior_labels = program
        .data_section
        .get_nodes()
        .iter()
        .filter(|node| {
            matches!(
                node,
                ASTNode::ROData { rodata, offset }
                    if rodata.name == ".rodata.__at__0x10" && *offset == 16
            )
        })
        .count();
    assert_eq!(interior_labels, 1);
}

#[test]
fn rodata_relocation_handles_interior_text_target() {
    let (mut object, rodata) = create_object_with_rodata(8);
    let text =
        object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(text, &EXIT_INSTRUCTION.repeat(2), 8);
    object.add_symbol(Symbol {
        name: b"entrypoint".to_vec(),
        value: 0,
        size: EXIT_INSTRUCTION.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let text_section = object.section_symbol(text);
    add_rodata_relocation(&mut object, rodata, 0, text_section, 8);

    let program =
        parse_object(object).expect("valid text relocation should parse");
    assert!(program.code_section.get_nodes().iter().any(|node| {
        matches!(
            node,
            ASTNode::Label { label, offset }
                if label.name == ".text.__at__0x8" && *offset == 8
        )
    }));
}

fn create_object_with_rodata(size: usize) -> (Object<'static>, SectionId) {
    let mut object =
        Object::new(BinaryFormat::Elf, Architecture::Sbf, Endianness::Little);
    let rodata = object.add_section(
        Vec::new(),
        b".rodata".to_vec(),
        SectionKind::ReadOnlyData,
    );
    object.append_section_data(rodata, &vec![0; size], 8);
    (object, rodata)
}

fn add_rodata_relocation(
    object: &mut Object<'_>,
    rodata: SectionId,
    offset: u64,
    symbol: object::write::SymbolId,
    addend: i64,
) {
    object
        .add_relocation(
            rodata,
            Relocation {
                offset,
                symbol,
                addend,
                flags: RelocationFlags::Generic {
                    kind: RelocationKind::Absolute,
                    encoding: RelocationEncoding::Generic,
                    size: 64,
                },
            },
        )
        .expect("failed to add rodata relocation");
}

fn parse_object(
    object: Object<'_>,
) -> Result<sbpf_assembler::ProgramLayout, SbpfLinkerError> {
    let bytes = object.write().expect("failed to write object");
    parse_bytecode(
        &bytes,
        ProgramOptions::new(
            OptimizationConfig::enabled(),
            SbpfArch::V0,
            STACK_FRAME_SIZE,
        ),
    )
}

fn assert_rodata_relocation_error(
    object: Object<'_>,
    expected_address: u64,
    expected_detail: &str,
) {
    let error = match parse_object(object) {
        Ok(_) => panic!("expected rodata relocation to fail"),
        Err(error) => error,
    };
    match error {
        SbpfLinkerError::RodataRelocationError {
            section,
            address,
            detail,
        } => {
            assert_eq!(section, ".rodata");
            assert_eq!(address, expected_address);
            assert_eq!(detail, expected_detail);
        }
        _ => panic!("expected RodataRelocationError"),
    }
}
