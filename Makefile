.PHONY: all build build-release build-gallery test test-harness test-unit lint fmt fmt-check clippy clean
	
all: build

build:
	cargo build

build-release:
	cargo build --release

gallery:
	cargo install-with-gallery 

test: test-harness

test-harness:
	cargo test --test tests compile_test -- --nocapture

test-unit:
	cargo test --lib

lint: fmt-check clippy

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

ci: lint test 

clean:
	cargo clean
