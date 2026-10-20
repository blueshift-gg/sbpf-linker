// assembly-output: ptx-linker
// ignore-sbpf-arch: v0
// compile-flags: --crate-type bin -C opt-level=3 -C panic=abort

// Several signed div/mod operations per function. The fixup computes register
// liveness once and then expands every division, so each expansion may only
// clobber registers that are dead at its own site, across straight-line code,
// branches and loop back edges. Every function runs under enough register
// pressure that at least one expansion has to spill.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

use core::ptr::read_volatile;

fn rd(input: *const i64, i: usize) -> i64 {
    unsafe { read_volatile(input.add(i)) }
}

/// Two independent divisions in one block, six values live across both.
#[inline(never)]
#[unsafe(no_mangle)]
pub fn two_divs(input: *const i64) -> u64 {
    let (v0, v1, v2, v3, v4, v5) = (rd(input, 0), rd(input, 1), rd(input, 2), rd(input, 3), rd(input, 4), rd(input, 5));
    let q1 = rd(input, 6).checked_div(rd(input, 7)).unwrap_or(0);
    let q2 = rd(input, 8).checked_div(rd(input, 9)).unwrap_or(0);
    ((((((q1 ^ v0).wrapping_mul(3) ^ v1).wrapping_mul(3) ^ v2).wrapping_mul(3) ^ v3).wrapping_mul(3) ^ v4).wrapping_mul(3) ^ v5 ^ q2) as u64
}

/// The first quotient is the second dividend.
#[inline(never)]
#[unsafe(no_mangle)]
pub fn chained_divs(input: *const i64) -> u64 {
    let (v0, v1, v2, v3, v4) = (rd(input, 0), rd(input, 1), rd(input, 2), rd(input, 3), rd(input, 4));
    let (a, b, c) = (rd(input, 5), rd(input, 6), rd(input, 7));
    let q1 = a.checked_div(b).unwrap_or(0);
    let q2 = q1.checked_rem(c).unwrap_or(0);
    (((((q2 ^ v0).wrapping_mul(3) ^ v1).wrapping_mul(3) ^ v2).wrapping_mul(3) ^ v3).wrapping_mul(3) ^ v4 ^ q1) as u64
}

/// A division inside a loop; loop-carried and loop-invariant values stay live
/// around the back edge.
#[inline(never)]
#[unsafe(no_mangle)]
pub fn div_in_loop(input: *const i64) -> u64 {
    let n = rd(input, 0) & 7;
    let (k0, k1, k2, k3, k4) = (rd(input, 1), rd(input, 2), rd(input, 3), rd(input, 4), rd(input, 5));
    let mut acc = rd(input, 6);
    let mut x = rd(input, 7);
    for _ in 0..n {
        acc = acc.checked_div(x).unwrap_or(1) ^ k0;
        x = x.wrapping_mul(k1) ^ k2;
        acc = acc.wrapping_add(k3) ^ k4;
    }
    (acc ^ x) as u64
}

/// `j0`-`j2` are read only before the division inside the loop, so they are
/// live at the division solely through the back edge.
#[inline(never)]
#[unsafe(no_mangle)]
pub fn div_live_across_back_edge(input: *const i64) -> u64 {
    let n = rd(input, 0) & 7;
    let (j0, j1, j2) = (rd(input, 1), rd(input, 2), rd(input, 3));
    let (k0, k1, k2) = (rd(input, 4), rd(input, 5), rd(input, 6));
    let mut acc = rd(input, 7);
    let x = rd(input, 8);
    for _ in 0..n {
        acc = ((acc ^ j0).wrapping_mul(j1) ^ j2).checked_div(x).unwrap_or(1);
        acc = (acc ^ k0).wrapping_mul(k1) ^ k2;
    }
    acc as u64
}

/// Different signed operations on each side of a branch, with different values
/// live in each arm.
#[inline(never)]
#[unsafe(no_mangle)]
pub fn divs_in_branches(input: *const i64) -> u64 {
    let (v0, v1, v2, v3, v4) = (rd(input, 0), rd(input, 1), rd(input, 2), rd(input, 3), rd(input, 4));
    let (a, b, c, d) = (rd(input, 5), rd(input, 6), rd(input, 7), rd(input, 8));
    let r = if rd(input, 9) & 1 == 0 {
        (a.checked_div(b).unwrap_or(0) ^ v0).wrapping_mul(v1)
    } else {
        (c.checked_rem(d).unwrap_or(0) ^ v2).wrapping_mul(v3)
    };
    (r ^ v4) as u64
}

/// `w` is defined between the two divisions (its load address depends on the
/// first quotient), so it is live across the second division only.
#[inline(never)]
#[unsafe(no_mangle)]
pub fn value_defined_between(input: *const i64) -> u64 {
    let (v0, v1, v2, v3, v4) = (rd(input, 0), rd(input, 1), rd(input, 2), rd(input, 3), rd(input, 4));
    let (a, b, c) = (rd(input, 5), rd(input, 6), rd(input, 7));
    let q1 = a.checked_div(b).unwrap_or(0);
    let w = rd(input, 8 + (q1 & 1) as usize);
    let q2 = c.checked_div(w ^ v0).unwrap_or(0);
    ((((q1 ^ v1).wrapping_mul(3) ^ v2).wrapping_mul(3) ^ v3).wrapping_mul(3) ^ v4 ^ q2 ^ w) as u64
}

#[unsafe(no_mangle)]
pub fn entrypoint(input: *mut u8) -> u64 {
    two_divs(input.cast())
        ^ chained_divs(input.cast())
        ^ div_in_loop(input.cast())
        ^ div_live_across_back_edge(input.cast())
        ^ divs_in_branches(input.cast())
        ^ value_defined_between(input.cast())
}

// LLVM emits the rem first (no spill), then the div (2 spills).
// CHECK-LABEL: label chained_divs
// CHECK-NOT:   stxdw
// CHECK:       mod64
// CHECK:       stxdw [r10-0x8], [[A0:r[0-9]]]
// CHECK-NEXT:  stxdw [r10-0x10], [[A1:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       div64
// CHECK:       ldxdw [[A0]], [r10-0x8]
// CHECK-NEXT:  ldxdw [[A1]], [r10-0x10]
// CHECK:       exit

// The spill is below LLVM's own slot and restored before the back edge.
// CHECK-LABEL: label div_in_loop
// CHECK:       stxdw [r10-0x8], r{{[0-9]}}
// CHECK:       stxdw [r10-0x10], [[B0:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       div64
// CHECK:       ldxdw [[B0]], [r10-0x10]
// CHECK:       j{{[a-z]+}} r{{[0-9]}}, r{{[0-9]}}, -0x
// CHECK:       exit

// With `j0`-`j2` live through the back edge no register is free, so all 3
// scratch registers are spilled, below LLVM's own slots.
// CHECK-LABEL: label div_live_across_back_edge
// CHECK:       stxdw [r10-0x18], [[E0:r[0-9]]]
// CHECK-NEXT:  stxdw [r10-0x20], [[E1:r[0-9]]]
// CHECK-NEXT:  stxdw [r10-0x28], [[E2:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       div64
// CHECK:       ldxdw [[E0]], [r10-0x18]
// CHECK-NEXT:  ldxdw [[E1]], [r10-0x20]
// CHECK-NEXT:  ldxdw [[E2]], [r10-0x28]
// CHECK:       j{{[a-z]+}} r{{[0-9]}}, r{{[0-9]}}, -0x

// Each arm has registers to spare, so neither expansion spills.
// CHECK-LABEL: label divs_in_branches
// CHECK-NOT:   stxdw
// CHECK:       div64
// CHECK-NOT:   stxdw
// CHECK:       mod64
// CHECK-NOT:   stxdw
// CHECK:       exit

// CHECK-LABEL: label entrypoint

// Two expansions in one block, each restoring exactly what it spilled.
// CHECK-LABEL: label two_divs
// CHECK:       stxdw [r10-0x8], [[C0:r[0-9]]]
// CHECK-NEXT:  stxdw [r10-0x10], [[C1:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       div64
// CHECK:       ldxdw [[C0]], [r10-0x8]
// CHECK-NEXT:  ldxdw [[C1]], [r10-0x10]
// CHECK:       stxdw [r10-[[OFF:0x[0-9a-f]+]]], [[C2:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       div64
// CHECK:       ldxdw [[C2]], [r10-[[OFF]]]
// CHECK:       exit

// `w` is not yet defined at the first division, so only the first spills.
// CHECK-LABEL: label value_defined_between
// CHECK:       stxdw [r10-0x8], r{{[0-9]}}
// CHECK:       stxdw [r10-0x10], [[D0:r[0-9]]]
// CHECK-NEXT:  stxdw [r10-0x18], [[D1:r[0-9]]]
// CHECK-NEXT:  mov64
// CHECK:       div64
// CHECK:       ldxdw [[D0]], [r10-0x10]
// CHECK-NEXT:  ldxdw [[D1]], [r10-0x18]
// CHECK-NOT:   stxdw
// CHECK:       div64
// CHECK:       exit
