<h1 align="center">
  SBPF Linker
</h1>
<p align="center">
  An upstream BPF linker to relink upstream BPF binaries into an SBPF V0 compatible binary format.
</p>

### Install

```sh
cargo install sbpf-linker
```

### Generate a Program

```sh
cargo generate --git https://github.com/blueshift-gg/solana-upstream-bpf-template
```

### Building LLVM from source

The xtask command will clone the `upstream-gallery-21` branch from the Blueshift LLVM fork.

```sh
cargo task build-llvm --src-dir ./llvm-project --build-dir ./llvm-build --install-prefix ./llvm-install
```

```sh
LLVM_PREFIX=./llvm-install cargo +nightly install --path .
```
