#!/bin/sh
set -eu

bun run --cwd frontend build

if build_key=$(sh scripts/build-key.sh frontend 2>/dev/null); then
  printf '%s\n' "$build_key" > frontend/dist/.nodeflare-revision
fi
