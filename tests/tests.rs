#![expect(unused_crate_dependencies, reason = "used in test harness")]

use std::{
    collections::{BTreeMap, HashMap},
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use either::Either;
use object::{File, Object as _, ObjectSection as _, ObjectSymbol as _};
use sbpf_assembler::{
    OptimizationConfig, SbpfArch,
    astnode::{ASTNode, ROData},
    header::ProgramHeader,
    parser::Token,
};
use sbpf_common::{
    inst_param::Number,
    instruction::{AsmFormat, Instruction},
    opcode::Opcode,
};
use sbpf_linker::{ProgramOptions, byteparser::parse_bytecode};

const NO_TESTS_FILTER: &str = "__no_tests_match_this_sbpf_arch__";

const DEFAULT_STACK_FRAME_SIZE: i32 = 4096;

trait TestArch {
    const ARCH: SbpfArch;

    fn decode_instruction(
        data: &[u8],
    ) -> Result<Instruction, sbpf_common::errors::SBPFError>;

    fn arch_arg() -> String {
        format!("v{}", Self::ARCH.e_flags())
    }

    fn dump(src: &Path, dst: &Path)
    where
        Self: Sized,
    {
        sbpf_dump::<Self>(src, dst);
    }
}

struct SbpfV0;

impl TestArch for SbpfV0 {
    const ARCH: SbpfArch = SbpfArch::V0;

    fn decode_instruction(
        data: &[u8],
    ) -> Result<Instruction, sbpf_common::errors::SBPFError> {
        Instruction::from_bytes(data)
    }
}

struct SbpfV3;

impl TestArch for SbpfV3 {
    const ARCH: SbpfArch = SbpfArch::V3;

    fn decode_instruction(
        data: &[u8],
    ) -> Result<Instruction, sbpf_common::errors::SBPFError> {
        Instruction::from_bytes_sbpf_v3(data)
    }
}

fn rustc_cmd() -> Command {
    Command::new(
        env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc")),
    )
}

fn find_binary(binary_re_str: &str) -> PathBuf {
    let binary_re = regex::Regex::new(binary_re_str).unwrap();
    let mut binary = which::which_re(binary_re).expect(binary_re_str);
    binary.next().unwrap_or_else(|| panic!("could not find {binary_re_str}"))
}

fn run_mode<A, F>(target: &str, mode: &str, sysroot: &Path, cfg: Option<F>)
where
    A: TestArch,
    F: Fn(&mut compiletest_rs::Config),
{
    let arch_arg = A::arch_arg();
    let cpu = match A::ARCH {
        SbpfArch::V0 => "v2",
        SbpfArch::V3 => "v4",
    };
    let target_rustcflags = format!(
        "-C linker={} -C target-cpu={} -C target-feature=+allows-misaligned-mem-access -C link-arg=--arch={} -C link-arg=--llvm-args=--bpf-stack-size=4096 --sysroot {}",
        env!("CARGO_BIN_EXE_sbpf-linker"),
        cpu,
        arch_arg,
        sysroot.display()
    );

    let llvm_filecheck = Some(find_binary(r"^FileCheck(-\d+)?$"));

    let mode = mode.parse().expect("invalid compiletest mode");
    let mut config = compiletest_rs::Config {
        target: target.to_owned(),
        target_rustcflags: Some(target_rustcflags),
        llvm_filecheck,
        mode,
        src_base: PathBuf::from(format!("tests/{mode}")),
        ..Default::default()
    };
    config.link_deps();

    if let Some(cfg) = cfg {
        cfg(&mut config);
    }

    config.filters = test_filters_for_arch::<A>(&config.src_base)
        .expect("failed to filter tests by sBPF arch");

    compiletest_rs::run_tests(&config);
}

fn sbpf_dump<A: TestArch>(src: &Path, dst: &Path) {
    let dump = render_emitted_program::<A>(src).unwrap_or_else(|err| {
        panic!("failed to render {}: {err}", src.display())
    });
    fs::write(dst, dump).unwrap_or_else(|err| {
        panic!("failed to write {}: {err}", dst.display())
    });
}

fn test_filters_for_arch<A: TestArch>(
    src_base: &Path,
) -> io::Result<Vec<String>> {
    let suite_name =
        src_base.file_name().unwrap_or_default().to_string_lossy();
    let arch_arg = A::arch_arg();
    let mut filters = Vec::new();
    collect_test_filters_for_arch(
        src_base,
        src_base,
        &suite_name,
        &mut filters,
        &arch_arg,
    )?;
    filters.sort();
    if filters.is_empty() {
        filters.push(NO_TESTS_FILTER.to_owned());
    }
    Ok(filters)
}

fn collect_test_filters_for_arch(
    src_base: &Path,
    dir: &Path,
    suite_name: &str,
    filters: &mut Vec<String>,
    arch_arg: &str,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            // Compiletest builds auxiliary crates only when a fixture requests them.
            if entry.file_name() != "auxiliary" {
                collect_test_filters_for_arch(
                    src_base, &path, suite_name, filters, arch_arg,
                )?;
            }
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && !ignored_for_arch(&path, arch_arg)?
        {
            let relative_path = path.strip_prefix(src_base).unwrap_or(&path);
            filters.push(format!("{suite_name}/{}", relative_path.display()));
        }
    }
    Ok(())
}

fn ignored_for_arch(path: &Path, arch_arg: &str) -> io::Result<bool> {
    let contents = fs::read_to_string(path)?;
    Ok(contents.lines().any(|line| {
        line.trim_start()
            .strip_prefix("//")
            .map(str::trim_start)
            .and_then(|line| line.strip_prefix("ignore-sbpf-arch:"))
            .is_some_and(|ignored_arches| {
                ignored_arches
                    .split([',', ' ', '\t'])
                    .any(|ignored_arch| ignored_arch.trim() == arch_arg)
            })
    }))
}

#[test]
fn compile_test() {
    // Assembly fixtures live in `tests/assembly`. Each file is a tiny Rust
    // crate with compiletest directives at the top and inline `CHECK:` lines
    // at the bottom. Use `// ignore-sbpf-arch: v0` or `v3` to skip a fixture
    // for one linker arch. Run just this harness with:
    //
    // `cargo test --test tests compile_test -- --nocapture`
    //
    // or run the whole suite with `cargo test`.
    let target = "bpfel-unknown-none";
    let root_dir = env::var_os("CARGO_MANIFEST_DIR")
        .expect("could not determine the root directory of the project");
    let root_dir = Path::new(&root_dir);
    let bpf_sysroot = if let Some(bpf_sysroot) =
        env::var_os("BPFEL_SYSROOT_DIR")
    {
        PathBuf::from(bpf_sysroot)
    } else {
        let rustc_src = rustc_build_sysroot::rustc_sysroot_src(rustc_cmd())
            .expect("could not determine sysroot source directory");
        let directory = root_dir.join("target/sysroot");
        let mut cargo = Command::new(
            env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")),
        );
        cargo.env("RUSTC_BOOTSTRAP", "1");
        match rustc_build_sysroot::SysrootBuilder::new(&directory, target)
            .cargo(cargo)
            .build_mode(rustc_build_sysroot::BuildMode::Build)
            .sysroot_config(rustc_build_sysroot::SysrootConfig::NoStd)
            .build_from_source(&rustc_src)
            .expect("failed to build sysroot")
        {
            rustc_build_sysroot::SysrootStatus::AlreadyCached => {}
            rustc_build_sysroot::SysrootStatus::SysrootBuilt => {}
        }
        directory
    };

    run_mode::<SbpfV0, _>(
        target,
        "assembly",
        &bpf_sysroot,
        Some(|cfg: &mut compiletest_rs::Config| {
            cfg.llvm_filecheck_preprocess = Some(SbpfV0::dump);
        }),
    );
    run_mode::<SbpfV3, _>(
        target,
        "assembly",
        &bpf_sysroot,
        Some(|cfg: &mut compiletest_rs::Config| {
            cfg.llvm_filecheck_preprocess = Some(SbpfV3::dump);
        }),
    );
}

// TODO: add below query methods to sbpf and update below to use them
fn render_emitted_program<A: TestArch>(path: &Path) -> anyhow::Result<String> {
    let bytes = fs::read(path)?;
    let syscall_labels = collect_syscall_labels::<A>(&bytes)?;
    let parse_result = parse_bytecode(
        &bytes,
        ProgramOptions::new(
            OptimizationConfig::enabled(),
            A::ARCH,
            DEFAULT_STACK_FRAME_SIZE,
        ),
    )?;
    let ph_count = if parse_result.prog_is_static { 1u64 } else { 3u64 };
    let rodata_base =
        parse_result.code_section.get_size() + 64 + ph_count * 56;
    let rodata_len = parse_result.data_section.get_size();

    let mut out = Vec::new();
    let rodata_nodes = parse_result.data_section.get_nodes();
    let mut rodata_labels = HashMap::new();
    let mut code_labels = HashMap::new();
    out.push(format!("rodata-count: {}", rodata_nodes.len()));

    for node in rodata_nodes {
        if let ASTNode::ROData { rodata, offset } = node {
            let label = format!("data_{offset:04x}");
            rodata_labels.insert(*offset, label.clone());
            out.push(format!("rodata-label[{offset}]: {label}"));
            out.push(format!("rodata[{offset}]: {}", render_rodata(rodata)?));
        }
    }

    let code_nodes = parse_result.code_section.get_nodes();
    for node in code_nodes {
        if let ASTNode::Label { label, offset } = node {
            code_labels.insert(*offset as i64, label.name.clone());
        }
    }

    out.extend(render_rodata_relocations::<A>(
        &bytes,
        rodata_nodes,
        rodata_base,
        rodata_len,
        parse_result.code_section.get_size(),
        &code_labels,
    )?);

    for node in code_nodes {
        match node {
            ASTNode::Label { label, offset } => {
                out.push(format!("{offset:04x}: label {}", label.name));
            }
            ASTNode::Instruction { instruction, offset } => {
                for asm in render_instruction::<A>(
                    instruction,
                    *offset,
                    rodata_base,
                    rodata_len,
                    &rodata_labels,
                    &code_labels,
                    &syscall_labels,
                )? {
                    out.push(format!("{offset:04x}: {asm}"));
                }
            }
            _ => {}
        }
    }

    Ok(out.join("\n"))
}

fn render_instruction<A: TestArch>(
    instruction: &Instruction,
    offset: u64,
    rodata_base: u64,
    rodata_len: u64,
    rodata_labels: &HashMap<u64, String>,
    code_labels: &HashMap<i64, String>,
    syscall_labels: &HashMap<u64, String>,
) -> anyhow::Result<Vec<String>> {
    if instruction.opcode == Opcode::Call
        && let Some(label) = syscall_labels.get(&offset)
    {
        return Ok(vec![format!("call {label}")]);
    }

    if instruction.opcode == Opcode::Call
        && let Some(Either::Right(Number::Int(value) | Number::Addr(value))) =
            &instruction.imm
    {
        let target = offset as i64 + 8 + value * 8;
        if let Some(label) = code_labels.get(&target) {
            return Ok(vec![
                instruction.to_asm(AsmFormat::Default)?,
                format!("call {label}"),
            ]);
        }
    }

    if instruction.opcode == Opcode::Lddw
        && let Some(Either::Right(number)) = &instruction.imm
        && let Number::Int(value) | Number::Addr(value) = number
        && let Some(offset) =
            rodata_offset_for_lddw::<A>(*value as u64, rodata_base, rodata_len)
    {
        let dst = instruction.dst.as_ref().ok_or_else(|| {
            anyhow::anyhow!("lddw is missing a destination register")
        })?;
        let mut rendered = vec![format!("lddw r{}, rodata[{offset}]", dst.n)];
        if let Some(label) = rodata_labels.get(&offset) {
            rendered.push(format!("lddw r{}, {}", dst.n, label));
        }
        return Ok(rendered);
    }

    Ok(vec![instruction.to_asm(AsmFormat::Default)?])
}

fn rodata_offset_for_lddw<A: TestArch>(
    value: u64,
    rodata_base: u64,
    rodata_len: u64,
) -> Option<u64> {
    let rodata_vaddr =
        ProgramHeader::new_load(rodata_base, rodata_len, false, A::ARCH)
            .p_vaddr;
    (value >= rodata_vaddr && value < rodata_vaddr + rodata_len)
        .then_some(value - rodata_vaddr)
}

fn collect_syscall_labels<A: TestArch>(
    bytes: &[u8],
) -> anyhow::Result<HashMap<u64, String>> {
    let obj = File::parse(bytes)?;
    let Some(text) = obj.section_by_name(".text") else {
        return Ok(HashMap::new());
    };
    let data = text.data()?;

    let mut labels = HashMap::new();
    let mut offset = 0usize;
    while offset < data.len() {
        let instruction =
            A::decode_instruction(&data[offset..]).map_err(|err| {
                anyhow::anyhow!("failed to decode .text at {offset:#x}: {err}")
            })?;
        if instruction.opcode == Opcode::Call
            && let Some(Either::Left(identifier)) = instruction.imm
        {
            labels.insert(offset as u64, identifier);
        }
        offset += if instruction.opcode == Opcode::Lddw { 16 } else { 8 };
    }

    Ok(labels)
}

fn render_rodata(rodata: &ROData) -> anyhow::Result<String> {
    match (&rodata.args[0], &rodata.args[1]) {
        (Token::Directive(directive, _), Token::VectorLiteral(values, _)) => {
            let bytes =
                values.iter().map(ToString::to_string).collect::<Vec<_>>();
            Ok(format!("{directive} {}", bytes.join(", ")))
        }
        (Token::Directive(directive, _), Token::StringLiteral(value, _)) => {
            Ok(format!("{directive} {:?}", value))
        }
        _ => Err(anyhow::anyhow!(
            "unsupported rodata node layout for {}",
            rodata.name
        )),
    }
}

fn render_rodata_relocations<A: TestArch>(
    bytes: &[u8],
    rodata_nodes: &[ASTNode],
    rodata_base: u64,
    rodata_len: u64,
    text_len: u64,
    code_labels: &HashMap<i64, String>,
) -> anyhow::Result<Vec<String>> {
    let obj = File::parse(bytes)?;
    let rodata_vaddr =
        ProgramHeader::new_load(rodata_base, rodata_len, false, A::ARCH)
            .p_vaddr;
    let mut relocation_lines = BTreeMap::new();
    for section in obj.sections().filter(|section| {
        section.name().is_ok_and(|name| {
            name.starts_with(".rodata") || name.starts_with(".data.rel.ro")
        })
    }) {
        for (input_offset, _) in section.relocations() {
            let mut relocation = None;
            for node in rodata_nodes {
                let ASTNode::ROData { rodata, offset } = node else {
                    continue;
                };
                let symbol = obj
                    .symbols()
                    .find(|symbol| {
                        symbol.name().is_ok_and(|name| name == rodata.name)
                            && symbol.section_index().is_some()
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "no symbol found for rodata: {}",
                            rodata.name
                        )
                    })?;
                let section_index = symbol.section_index().unwrap().0;
                let address = symbol.address();
                if section_index != section.index().0 {
                    continue;
                }

                let Some(offset_in_node) = input_offset.checked_sub(address)
                else {
                    continue;
                };

                let node_bytes = match rodata.args.get(1) {
                    Some(Token::VectorLiteral(node_bytes, _)) => {
                        node_bytes.as_slice()
                    }
                    _ => {
                        return Err(anyhow::anyhow!(
                            "rodata {} is not a byte vector",
                            rodata.name
                        ));
                    }
                };
                if offset_in_node < node_bytes.len() as u64 {
                    relocation = Some((
                        rodata,
                        *offset + offset_in_node,
                        offset_in_node,
                    ));
                    break;
                }
            }

            let (relocation_rodata, output_offset, offset_in_node) =
                relocation.ok_or_else(|| {
                    anyhow::anyhow!(
                        "invalid rodata relocation: {input_offset:#x}",
                    )
                })?;

            let offset_in_node = usize::try_from(offset_in_node)?;
            let relocation_bytes = match relocation_rodata.args.get(1) {
                Some(Token::VectorLiteral(node_bytes, _)) => node_bytes
                    .get(offset_in_node..offset_in_node + 8)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "relocation in rodata {} is out of bounds",
                            relocation_rodata.name
                        )
                    })?,
                _ => {
                    return Err(anyhow::anyhow!(
                        "rodata {} is not a byte vector",
                        relocation_rodata.name
                    ));
                }
            };
            let mut encoded_target = [0u8; 8];
            for (byte, value) in
                encoded_target.iter_mut().zip(relocation_bytes)
            {
                let Number::Int(value) = value else {
                    return Err(anyhow::anyhow!(
                        "relocation in rodata {} contains non-integer byte",
                        relocation_rodata.name
                    ));
                };
                *byte = u8::try_from(*value).map_err(|_| {
                    anyhow::anyhow!(
                        "relocation in rodata {} contains invalid byte {value}",
                        relocation_rodata.name
                    )
                })?;
            }

            let encoded_target = u64::from_le_bytes(encoded_target);
            let target_vaddr = if A::ARCH.is_v3() {
                encoded_target
            } else {
                encoded_target >> 32
            };

            let text_vaddr = ProgramHeader::new_load(
                rodata_base - text_len,
                text_len,
                true,
                A::ARCH,
            )
            .p_vaddr;
            let target = if let Some(text_off) = target_vaddr
                .checked_sub(text_vaddr)
                .filter(|address| *address < text_len)
            {
                match code_labels.get(&(text_off as i64)) {
                    Some(name) => format!("text[{text_off}] ({name})"),
                    None => format!("text[{text_off}]"),
                }
            } else if let Some(target_address_out) = target_vaddr
                .checked_sub(rodata_vaddr)
                .filter(|address| *address < rodata_len)
            {
                let target_name = rodata_nodes.iter().find_map(|node| {
                    let ASTNode::ROData { rodata, offset } = node else {
                        return None;
                    };
                    if *offset == target_address_out {
                        return Some(rodata.name.as_str());
                    }
                    None
                });
                match target_name {
                    Some(name) => {
                        format!("rodata[{target_address_out}] ({name})")
                    }
                    None => format!("rodata[{target_address_out}]"),
                }
            } else {
                return Err(anyhow::anyhow!(
                    "relocation in rodata {} targets an address outside text and rodata",
                    relocation_rodata.name
                ));
            };
            relocation_lines.insert(
                output_offset,
                format!("rodata-relocation[{output_offset}] -> {target}"),
            );
        }
    }
    Ok(relocation_lines.into_values().collect())
}
