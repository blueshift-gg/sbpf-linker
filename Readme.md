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

### Upstream Gallery: Early Feature Gate

Integrates the latest LLVM commits from [`upstream-gallery-21`](https://github.com/blueshift-gg/llvm-project/tree/upstream-gallery-21) to experiment with upcoming changes during the upstreaming process. The xtask command clones this branch from the Blueshift LLVM fork and builds sbpf-linker with static LLVM linking.

```sh
cargo xtask
```

### Generate a Program

```sh
cargo generate --git https://github.com/blueshift-gg/solana-upstream-bpf-template
```

```sh
cargo +nightly build-bpf
```
