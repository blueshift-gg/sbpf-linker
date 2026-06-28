// assembly-output: ptx-linker
// revisions: live_self_recursive dead_self_recursive live_mutual_recursive
// compile-flags: --crate-type bin -C opt-level=3

// Tests that DFE correctly handles recursive functions:
//
// live_self_recursive  — a self-recursive function reachable from entrypoint
//                        must survive DFE and its self-call must remain in
//                        the linked binary.
//
// dead_self_recursive  — a self-recursive function never called from entrypoint
//                        must be eliminated by DFE even though it has a
//                        self-call edge in the CFG.
//
// live_mutual_recursive — two mutually recursive functions (ping <-> pong)
//                         reachable from entrypoint must both survive DFE and
//                         their cross-calls must remain in the linked binary.

#![no_std]
#![no_main]

// aux-build: loop-panic-handler.rs
extern crate loop_panic_handler;

// ── live_self_recursive ──────────────────────────────────────────────────────

// factorial(n) = n * factorial(n-1). The volatile read on the recursive result
// breaks the tail-call pattern so LLVM emits a real recursive call instruction.
#[cfg(live_self_recursive)]
#[unsafe(no_mangle)]
#[inline(never)]
fn factorial(n: u64) -> u64 {
    if n <= 1 {
        return 1;
    }
    let sub = factorial(n - 1);
    (unsafe { core::ptr::read_volatile(&sub as *const u64) }) * n
}

#[cfg(live_self_recursive)]
#[unsafe(no_mangle)]
pub fn entrypoint(_input: *mut u8) -> u64 {
    factorial(5)
}

// ── dead_self_recursive ──────────────────────────────────────────────────────

// dead_recursive is never called from entrypoint. DFE must remove it even
// though it has a self-call edge in the CFG.
#[cfg(dead_self_recursive)]
#[unsafe(no_mangle)]
#[inline(never)]
fn dead_recursive(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    dead_recursive(n - 1)
}

#[cfg(dead_self_recursive)]
#[unsafe(no_mangle)]
pub fn entrypoint(_input: *mut u8) -> u64 {
    42
}

// ── live_mutual_recursive ────────────────────────────────────────────────────

// ping and pong call each other. The volatile reads prevent tail-call
// optimisation so both real call instructions are preserved.
#[cfg(live_mutual_recursive)]
#[unsafe(no_mangle)]
#[inline(never)]
fn ping(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let sub = pong(n - 1);
    (unsafe { core::ptr::read_volatile(&sub as *const u64) }) + 1
}

#[cfg(live_mutual_recursive)]
#[unsafe(no_mangle)]
#[inline(never)]
fn pong(n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let sub = ping(n - 1);
    (unsafe { core::ptr::read_volatile(&sub as *const u64) }) + 1
}

#[cfg(live_mutual_recursive)]
#[unsafe(no_mangle)]
pub fn entrypoint(_input: *mut u8) -> u64 {
    ping(4)
}

// ── CHECK patterns ───────────────────────────────────────────────────────────

// factorial is reachable from entrypoint so DFE must keep it. The self-call
// inside factorial's body must appear after the function label.
// CHECK,live_self_recursive: label factorial
// CHECK,live_self_recursive: call factorial

// dead_recursive is unreachable — DFE must eliminate it entirely.
// CHECK,dead_self_recursive: label entrypoint
// CHECK,dead_self_recursive-NOT: label dead_recursive

// Both ping and pong are reachable. Each function's cross-call must survive.
// entrypoint comes first, then ping, then pong — matching the linked binary order.
// CHECK,live_mutual_recursive: label entrypoint
// CHECK,live_mutual_recursive: call ping
// CHECK,live_mutual_recursive: label ping
// CHECK,live_mutual_recursive: call pong
// CHECK,live_mutual_recursive: label pong
// CHECK,live_mutual_recursive: call ping
