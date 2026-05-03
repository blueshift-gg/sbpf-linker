#![expect(unused_crate_dependencies, reason = "used in test harness")]

use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use either::Either;
use object::{File, Object as _, ObjectSection as _};
use sbpf_assembler::{
    astnode::{ASTNode, ROData},
    parser::Token,
};
use sbpf_common::{
    inst_param::Number,
    instruction::{AsmFormat, Instruction},
    opcode::Opcode,
};
use sbpf_linker::byteparser::parse_bytecode;

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

fn run_mode<F>(target: &str, mode: &str, sysroot: &Path, cfg: Option<F>)
where
    F: Fn(&mut compiletest_rs::Config),
{
    let target_rustcflags = format!(
        "-C linker={} --sysroot {}",
        env!("CARGO_BIN_EXE_sbpf-linker"),
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

    compiletest_rs::run_tests(&config);
}

fn sbpf_dump(src: &Path, dst: &Path) {
    let dump = render_emitted_program(src).unwrap_or_else(|err| {
        panic!("failed to render {}: {err}", src.display())
    });
    fs::write(dst, dump).unwrap_or_else(|err| {
        panic!("failed to write {}: {err}", dst.display())
    });
}

#[test]
fn compile_test() {
    // Assembly fixtures live in `tests/assembly`. Each file is a tiny Rust
    // crate with compiletest directives at the top and inline `CHECK:` lines
    // at the bottom. Run just this harness with:
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

    run_mode(
        target,
        "assembly",
        &bpf_sysroot,
        Some(|cfg: &mut compiletest_rs::Config| {
            cfg.llvm_filecheck_preprocess = Some(sbpf_dump);
        }),
    );
}

// TODO: add below query methods to sbpf and update below to use them
fn render_emitted_program(path: &Path) -> anyhow::Result<String> {
    let bytes = fs::read(path)?;
    let syscall_labels = collect_syscall_labels(&bytes)?;
    let parse_result = parse_bytecode(&bytes)?;
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

    for node in parse_result.code_section.get_nodes() {
        match node {
            ASTNode::Label { label, offset } => {
                code_labels.insert(*offset as i64, label.name.clone());
                out.push(format!("{offset:04x}: label {}", label.name));
            }
            ASTNode::Instruction { instruction, offset } => {
                for asm in render_instruction(
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

fn render_instruction(
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
        && (*value as u64) >= rodata_base
        && (*value as u64) < rodata_base + rodata_len
    {
        let offset = (*value as u64) - rodata_base;
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

fn collect_syscall_labels(
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
            Instruction::from_bytes(&data[offset..]).map_err(|err| {
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
