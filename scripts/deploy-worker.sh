#!/bin/sh
set -eu

# Some Cloudflare deployment flows provision bindings before this script runs.
# A first deployment without that pre-provisioning needs one upload to create D1.
if ! bunx wrangler d1 info DB --json >/dev/null 2>&1; then
  echo "D1 binding is not provisioned; creating it through Wrangler deploy..."
  bunx wrangler deploy
fi

bunx wrangler d1 migrations apply DB --remote
exec bunx wrangler deploy
