#!/bin/sh
set -eu

if [ "${CF_MONITOR_ADMIN_READY:-0}" != "1" ]; then
  bun run build:agent
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

exec "$worker_build" --release --no-panic-recovery
