#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
NODEFLARE_VERSION=$(sh "$script_dir/resolve-version.sh")
VITE_NODEFLARE_VERSION=$NODEFLARE_VERSION
export NODEFLARE_VERSION VITE_NODEFLARE_VERSION
cd "$root_dir"

current_build_key=$(sh scripts/build-key.sh worker 2>/dev/null || true)
if [ "${NODEFLARE_WORKER_READY:-0}" = "1" ] && [ -n "$current_build_key" ]; then
  built_build_key=$(sed -n '1p' build/.nodeflare-revision 2>/dev/null || true)
  if [ "$built_build_key" = "$current_build_key" ] \
    && [ -f build/index.js ] \
    && [ -f build/worker/index.wasm ]; then
    echo "Worker build output is ready; skipping rebuild."
    exit 0
  fi
fi

. "$script_dir/ensure-rust.sh"

if [ "${NODEFLARE_ADMIN_READY:-0}" != "1" ]; then
  bun run build:frontend
fi

if command -v worker-build >/dev/null 2>&1; then
  worker_build="$(command -v worker-build)"
elif [ -x "${CARGO_HOME:-$HOME/.cargo}/bin/worker-build" ]; then
  worker_build="${CARGO_HOME:-$HOME/.cargo}/bin/worker-build"
else
  cargo install -q "worker-build@^0.8"
  worker_build="${CARGO_HOME:-$HOME/.cargo}/bin/worker-build"
fi

CUSTOM_SHIM="$script_dir/worker-shim.js" "$worker_build" --release --no-panic-recovery

if [ -n "$current_build_key" ]; then
  printf '%s\n' "$current_build_key" > build/.nodeflare-revision
fi
