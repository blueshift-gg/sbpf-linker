// assembly-output: ptx-linker
// ignore-sbpf-arch: v0
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// The signed div/mod fixup needs scratch registers (div: 3, mod: 2). Each
// function below keeps a different number of values live across the division,
// so the fixup must spill 0-3 of them around its expansion. Where LLVM has
// already spilled to the stack, the fixup's slots must sit below LLVM's.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

use core::ptr::read_volatile;

/// Defines `$name`, which reads one value per `$v`, then performs a signed
/// `$method` and folds every value into the result afterwards. Each `$v` is
/// therefore live across the division, so the list length sets the register
/// pressure at the division site.
macro_rules! signed_op_with_live_values {
    ($name:ident, $ty:ty, $method:ident, [$($v:ident),*]) => {
        #[inline(never)]
        #[unsafe(no_mangle)]
        pub fn $name(input: *const $ty) -> u64 {
            let mut i = 0;
            $(let $v = unsafe { read_volatile(input.add(i)) }; i += 1;)*
            let a = unsafe { read_volatile(input.add(i)) };
            let b = unsafe { read_volatile(input.add(i + 1)) };
            let mut acc = a.$method(b).unwrap_or(0);
            $(acc = (acc ^ $v).wrapping_mul(3);)*
            acc as u64
        }
    };
}

signed_op_with_live_values!(div64_live5, i64, checked_div, [v0, v1, v2, v3, v4]);
signed_op_with_live_values!(div64_live6, i64, checked_div, [v0, v1, v2, v3, v4, v5]);
signed_op_with_live_values!(div64_live7, i64, checked_div, [v0, v1, v2, v3, v4, v5, v6]);
signed_op_with_live_values!(div64_live8, i64, checked_div, [v0, v1, v2, v3, v4, v5, v6, v7]);
signed_op_with_live_values!(div64_live9, i64, checked_div, [v0, v1, v2, v3, v4, v5, v6, v7, v8]);
signed_op_with_live_values!(rem64_live7, i64, checked_rem, [v0, v1, v2, v3, v4, v5, v6]);
signed_op_with_live_values!(rem64_live8, i64, checked_rem, [v0, v1, v2, v3, v4, v5, v6, v7]);
signed_op_with_live_values!(div32_live8, i32, checked_div, [v0, v1, v2, v3, v4, v5, v6, v7]);

#[unsafe(no_mangle)]
pub fn entrypoint(input: *mut u8) -> u64 {
    div64_live5(input.cast())
        ^ div64_live6(input.cast())
        ^ div64_live7(input.cast())
        ^ div64_live8(input.cast())
        ^ div64_live9(input.cast())
        ^ rem64_live7(input.cast())
        ^ rem64_live8(input.cast())
        ^ div32_live8(input.cast())
}

// div32, all 3 scratch registers spilled.
// CHECK-LABEL: label div32_live8
// CHECK:       stxdw [r10-0x8], [[A0:r[0-9]]]
// CHECK-NEXT:  stxdw [r10-0x10], [[A1:r[0-9]]]
// CHECK-NEXT:  stxdw [r10-0x18], [[A2:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       div64
// CHECK:       ldxdw [[A0]], [r10-0x8]
// CHECK-NEXT:  ldxdw [[A1]], [r10-0x10]
// CHECK-NEXT:  ldxdw [[A2]], [r10-0x18]
// CHECK:       exit

// div64, enough free registers: no spills.
// CHECK-LABEL: label div64_live5
// CHECK-NOT:   stxdw
// CHECK:       div64
// CHECK-NOT:   ldxdw r{{[0-9]}}, [r10
// CHECK:       exit

// div64, 1 spill.
// CHECK-LABEL: label div64_live6
// CHECK:       stxdw [r10-0x8], [[B0:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       div64
// CHECK:       ldxdw [[B0]], [r10-0x8]
// CHECK-NOT:   ldxdw r{{[0-9]}}, [r10
// CHECK:       exit

// div64, 2 spills.
// CHECK-LABEL: label div64_live7
// CHECK:       stxdw [r10-0x8], [[C0:r[0-9]]]
// CHECK-NEXT:  stxdw [r10-0x10], [[C1:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       div64
// CHECK:       ldxdw [[C0]], [r10-0x8]
// CHECK-NEXT:  ldxdw [[C1]], [r10-0x10]
// CHECK-NOT:   ldxdw r{{[0-9]}}, [r10
// CHECK:       exit

// div64, LLVM spills one value itself; the fixup's 2 slots go below it.
// CHECK-LABEL: label div64_live8
// CHECK:       stxdw [r10-0x8], r{{[0-9]}}
// CHECK:       stxdw [r10-0x10], [[D0:r[0-9]]]
// CHECK-NEXT:  stxdw [r10-0x18], [[D1:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       div64
// CHECK:       ldxdw [[D0]], [r10-0x10]
// CHECK-NEXT:  ldxdw [[D1]], [r10-0x18]
// CHECK:       ldxdw r{{[0-9]}}, [r10-0x8]
// CHECK:       exit

// div64, LLVM spills two values itself; the fixup's 2 slots go below them.
// CHECK-LABEL: label div64_live9
// CHECK:       stxdw [r10-0x10], r{{[0-9]}}
// CHECK:       stxdw [r10-0x8], r{{[0-9]}}
// CHECK:       stxdw [r10-0x18], [[E0:r[0-9]]]
// CHECK-NEXT:  stxdw [r10-0x20], [[E1:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       div64
// CHECK:       ldxdw [[E0]], [r10-0x18]
// CHECK-NEXT:  ldxdw [[E1]], [r10-0x20]
// CHECK:       exit

// CHECK-LABEL: label entrypoint

// mod64, 1 of 2 scratch registers spilled.
// CHECK-LABEL: label rem64_live7
// CHECK:       stxdw [r10-0x8], [[F0:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       mod64
// CHECK:       ldxdw [[F0]], [r10-0x8]
// CHECK-NOT:   ldxdw r{{[0-9]}}, [r10
// CHECK:       exit

// mod64, LLVM spills one value itself; the fixup's slot goes below it.
// CHECK-LABEL: label rem64_live8
// CHECK:       stxdw [r10-0x8], r{{[0-9]}}
// CHECK:       stxdw [r10-0x10], [[G0:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       mod64
// CHECK:       ldxdw [[G0]], [r10-0x10]
// CHECK:       exit
