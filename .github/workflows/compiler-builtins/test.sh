#!/usr/bin/env bash
set -euo pipefail

fixture_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "${fixture_dir}/../../.." && pwd)
linker=${SBPF_LINKER:-${repo_root}/target/debug/sbpf-linker}
target_dir=${CARGO_TARGET_DIR:-${repo_root}/target/compiler-builtins}
program=${target_dir}/bpfel-unknown-none/release/liblibcall_program.so

test -x "${linker}"

export CARGO_TARGET_DIR="${target_dir}"
export RUSTFLAGS="-Clinker=${linker} -Clink-arg=--llvm-args=--bpf-stack-size=4096 -Clink-arg=--deploy=false -Clink-arg=--export=__muldf3"

cargo +nightly build \
    --release \
    --offline \
    --locked \
    --target bpfel-unknown-none \
    -Z build-std=core,alloc \
    --manifest-path "${fixture_dir}/Cargo.toml" \
    --package libcall-program

test -s "${program}"
grep -aFq 'compiler-builtins fixture linked' "${program}"
