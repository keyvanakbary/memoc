#!/bin/sh
# Runs inside the native Rust Alpine builder. /cache persists between runs.
set -eu
export CARGO_HOME=/cache/cargo CARGO_TARGET_DIR=/cache/target CARGO_INCREMENTAL=0
cargo test --locked
cargo build --locked --release
cp "$CARGO_TARGET_DIR/release/memoc" /out/memoc
# Only third-party artifacts belong in the shared debug and release cache.
cargo clean --locked -p memoc
