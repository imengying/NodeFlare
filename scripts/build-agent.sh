#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$script_dir/ensure-rust.sh"

case "$(uname -m)" in
  x86_64|amd64) arch="x86_64" ;;
  aarch64|arm64) arch="aarch64" ;;
  *) echo "Unsupported agent build architecture" >&2; exit 1 ;;
esac

agent_target_dir=${CARGO_TARGET_DIR:-target/agent}
CARGO_TARGET_DIR="$agent_target_dir" cargo build --manifest-path agent/Cargo.toml --release --locked
install -Dm755 "$agent_target_dir/release/cf-monitor-agent" "frontend/public/agent-linux-$arch"
