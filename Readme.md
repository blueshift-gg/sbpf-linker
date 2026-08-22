<h1 align="center">
  SBPF Linker
</h1>
<p align="center">
  An upstream BPF linker to relink upstream BPF binaries into an SBPF V0/V3 compatible binary format.
</p>

## Installation

### cargo binstall (recommended for solana devs)

The recommended installation method is
[`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall):

```sh
cargo binstall sbpf-linker
```

### cargo install (requires toolchain that ships a shared LLVM library)

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
