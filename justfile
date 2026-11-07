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

# Regenerate the gate parity corpus by running the JavaScript oracle.
# Needs a sibling open-auri checkout, or AURI_OPEN_AURI_DIR pointing at one.
# The JSON it writes is gitignored like every other oracle.
corpus:
  node tools/gen-gate-verdicts.mjs

# Byte-exact parity against the open-auri oracles (AGPL, not vendored).
# Set AURI_FIXTURES_DIR to <open-auri>/test/fixtures and AURI_GATE_VERDICTS to
# a generated gate-verdicts.json if they are not in the default places.
parity:
  cargo test -p auri-codec --test parity -- --nocapture
  cargo test -p auri-gate --test parity -- --nocapture
