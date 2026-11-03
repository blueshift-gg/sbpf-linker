<h1 align="center">
  SBPF Linker
</h1>
<p align="center">
  An upstream BPF linker to relink upstream BPF binaries into an SBPF V0/V3 compatible binary format.
</p>
<p align="center">
  <a href="https://codecov.io/gh/blueshift-gg/sbpf-linker">
    <img src="https://codecov.io/gh/blueshift-gg/sbpf-linker/graph/badge.svg" alt="codecov">
  </a>
</p>

## Installation

### cargo binstall (recommended for solana devs)

The recommended installation method is
[`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall):

```sh
cargo binstall sbpf-linker
```

### cargo install (sbpf-linker developers only)

> [!WARNING]
> Building from source is intended for sbpf-linker development only. It depends on the Rust toolchain's LLVM shared library at runtime, which breaks when the toolchain's LLVM version or layout changes. Use `cargo binstall` to install sbpf-linker for building Solana programs.

A source install that selects the LLVM shared library at runtime (make sure the linker is invoked from rustc nightly with minimum nightly-2026-08-05)

```sh
cargo install sbpf-linker
```

### Generate a Program

```sh
cargo generate --git https://github.com/blueshift-gg/solana-upstream-bpf-template
```

```sh
cargo +nightly build-bpf
```

## Compiler builtins

Pass `solana-compiler-builtins` as a regular linker input before Rust's `libcompiler_builtins.rlib`. Rust's implementations use `weak` linkage, so the Solana implementations take precedence. With the crate's `weak-linkage` feature, the Solana implementations use `weak_odr` linkage. When both define the same symbol, LLVM retains the first definition encountered, giving the Solana implementations precedence, and program-defined implementations still override both.

The linker emits a warning if `solana-compiler-builtins` is missing from the inputs. With LTO, rustc merges dependencies before linking, so the linker recognizes the crate from thin-LTO object names or from the symbols the fat-LTO object defines.

When building a Solana program with Cargo, add `solana-compiler-builtins` as a
direct dependency from the program's project directory:

```sh
cargo add solana-compiler-builtins
```

Declaring the dependency alone does not guarantee that rustc will include it in
the linker inputs. Reference the crate from `src/lib.rs` to ensure it is linked:

```rust
use solana_compiler_builtins as _;
```
