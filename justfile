#!/usr/bin/env -S just --justfile

test:
  cargo test --workspace

test-release:
  cargo test --workspace --release

lint:
  cargo clippy --workspace --all-targets -- -D warnings

fmt:
  cargo +nightly fmt --all

fmt-check:
  cargo +nightly fmt --all -- --check

build:
  cargo build --release --workspace

# Byte-exact parity against the open-auri oracles (AGPL, not vendored).
# Set AURI_FIXTURES_DIR to point at <open-auri>/test/fixtures if the checkout
# is not a sibling of this repository.
parity:
  cargo test -p auri-codec --test parity -- --nocapture
