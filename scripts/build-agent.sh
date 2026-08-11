#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
NODEFLARE_NATIVE_RUST_ONLY=1
export NODEFLARE_NATIVE_RUST_ONLY
. "$script_dir/ensure-rust.sh"

agent_target_dir=${CARGO_TARGET_DIR:-target/agent}
CARGO_TARGET_DIR="$agent_target_dir" cargo build --manifest-path agent/Cargo.toml --release --locked
