use either::Either;
use sbpf_assembler::CompileError;
use sbpf_assembler::ast::AST;
use sbpf_assembler::astnode::ASTNode;
use sbpf_common::{
    instruction::Instruction,
    opcode::{LOAD_MEMORY_OPS, Opcode, STORE_IMM_OPS, STORE_REG_OPS},
};
use std::ops::Range;

const R11: u8 = 11;
const R10: u8 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionRange {
    pub name: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackArgOverlap {
    pub function: String,
    pub local: Range<i32>,
    pub argument: Range<i32>,
}

fn memory_width(opcode: Opcode) -> Option<i32> {
    match opcode {
        Opcode::Ldxb | Opcode::Stb | Opcode::Stxb => Some(1),
        Opcode::Ldxh | Opcode::Sth | Opcode::Stxh => Some(2),
        Opcode::Ldxw | Opcode::Stw | Opcode::Stxw => Some(4),
        Opcode::Ldxdw | Opcode::Stdw | Opcode::Stxdw => Some(8),
        _ => None,
    }
}

pub fn diagnose_stack_arg_overlaps(
    ast: &AST,
    stack_frame_size: i32,
    functions: &[FunctionRange],
) -> Vec<StackArgOverlap> {
    fn ranges_overlap(left: &Range<i32>, right: &Range<i32>) -> bool {
        left.start < right.end && right.start < left.end
    }

    fn memory_access(
        instruction: &Instruction,
    ) -> Option<(u8, i16, i32, bool)> {
        let Some(Either::Right(offset)) = instruction.off else {
            return None;
        };
        let width = memory_width(instruction.opcode)?;
        let (register, is_load) =
            if LOAD_MEMORY_OPS.contains(&instruction.opcode) {
                (instruction.src.as_ref()?.n, true)
            } else if STORE_IMM_OPS.contains(&instruction.opcode)
                || STORE_REG_OPS.contains(&instruction.opcode)
            {
                (instruction.dst.as_ref()?.n, false)
            } else {
                return None;
            };

        Some((register, offset, width, is_load))
    }

    let mut overlaps = Vec::new();

    for function in functions {
        let instructions = ast.nodes.iter().filter_map(|node| match node {
            ASTNode::Instruction { instruction, offset }
                if *offset >= function.start && *offset < function.end =>
            {
                Some(instruction)
            }
            _ => None,
        });

        let mut locals = Vec::new();
        let mut arguments = Vec::new();
        for (register, offset, width, is_load) in
            instructions.filter_map(memory_access)
        {
            if register == R10 && offset < 0 {
                let start = i32::from(offset);
                locals.push(start..start + width);
            } else if register == R11 && offset > 0 && is_load {
                let start = i32::from(offset) - stack_frame_size;
                arguments.push(start..start + width);
            }
        }

        for local in &locals {
            for argument in &arguments {
                if ranges_overlap(local, argument) {
                    overlaps.push(StackArgOverlap {
                        function: function.name.clone(),
                        local: local.clone(),
                        argument: argument.clone(),
                    });
                }
            }
        }
    }

    overlaps
}

pub fn rewrite_r11_stack_args(
    ast: &mut AST,
    stack_frame_size: i32,
) -> Result<(), Vec<CompileError>> {
    let mut errors = Vec::new();

    for node in ast.nodes.iter_mut() {
        let ASTNode::Instruction { instruction, offset } = node else {
            continue;
        };

        let uses_r11 = instruction.src.as_ref().is_some_and(|r| r.n == R11)
            || instruction.dst.as_ref().is_some_and(|r| r.n == R11);
        if !uses_r11 {
            continue;
        }

        let is_load = LOAD_MEMORY_OPS.contains(&instruction.opcode);
        let is_store = STORE_IMM_OPS.contains(&instruction.opcode)
            || STORE_REG_OPS.contains(&instruction.opcode);
        assert!(
            is_load || is_store,
            "r11 must only be used by memory load/store instructions"
        );

        let Some(Either::Right(off)) = instruction.off else {
            unreachable!(
                "memory load/store instructions always have an offset"
            );
        };

        if is_load {
            assert_eq!(
                instruction.src.as_ref().map(|r| r.n),
                Some(R11),
                "r11 must be the base register of a memory load"
            );
            assert!(
                off > 0,
                "an incoming r11 load must have a positive offset"
            );

            let Some(new_off) = i32::from(off)
                .checked_sub(stack_frame_size)
                .and_then(|offset| i16::try_from(offset).ok())
            else {
                errors.push(CompileError::BytecodeError {
                    error: format!(
                        "cannot rewrite r11 load at byte offset {offset:#x}: {off} - {stack_frame_size} does not fit in a BPF instruction offset"
                    ),
                    span: instruction.span.clone(),
                    custom_label: None,
                });
                continue;
            };
            instruction.off = Some(Either::Right(new_off));
            instruction
                .src
                .as_mut()
                .expect("a memory load always has a source register")
                .n = R10;
        } else {
            assert_eq!(
                instruction.dst.as_ref().map(|r| r.n),
                Some(R11),
                "r11 must be the base register of a memory store"
            );
            assert!(
                off < 0,
                "an outgoing r11 store must have a negative offset"
            );

            let Some(new_off) = off.checked_neg() else {
                errors.push(CompileError::BytecodeError {
                    error: format!(
                        "cannot rewrite r11 store at byte offset {offset:#x}: negating offset {off} does not fit in a BPF instruction offset"
                    ),
                    span: instruction.span.clone(),
                    custom_label: None,
                });
                continue;
            };
            instruction.off = Some(Either::Right(new_off));
            instruction
                .dst
                .as_mut()
                .expect("a memory store always has a destination register")
                .n = R10;
        }
    }

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}
