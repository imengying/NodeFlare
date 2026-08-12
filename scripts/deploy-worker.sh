#!/bin/sh
set -eu

runtime_secrets_file=""

cleanup() {
  if [ -n "$runtime_secrets_file" ]; then
    rm -f "$runtime_secrets_file"
  fi
}

trap cleanup EXIT HUP INT TERM

frontend_ready() {
  build_key=$(sh scripts/build-key.sh frontend 2>/dev/null || true)
  built_build_key=$(sed -n '1p' frontend/dist/.nodeflare-revision 2>/dev/null || true)
  [ -n "$build_key" ] \
    && [ "$built_build_key" = "$build_key" ] \
    && [ -f frontend/dist/index.html ] \
    && [ -f frontend/admin-dist/admin.js ]
}

worker_ready() {
  build_key=$(sh scripts/build-key.sh worker 2>/dev/null || true)
  built_build_key=$(sed -n '1p' build/.nodeflare-revision 2>/dev/null || true)
  [ -n "$build_key" ] \
    && [ "$built_build_key" = "$build_key" ] \
    && [ -f build/index.js ] \
    && [ -f build/worker/index.wasm ]
}

prepare_runtime_secrets() {
  runtime_secrets_file=$(mktemp)
  RUNTIME_SECRETS_FILE="$runtime_secrets_file" bun -e '
    const names = ["ADMIN_PASSWORD", "TURNSTILE_SECRET_KEY", "CF_USAGE_API_TOKEN"];
    const secrets = Object.fromEntries(
      names.flatMap((name) => process.env[name] ? [[name, process.env[name]]] : []),
    );
    if (Object.keys(secrets).length > 0) {
      await Bun.write(process.env.RUNTIME_SECRETS_FILE, JSON.stringify(secrets));
    }
  '

  if [ ! -s "$runtime_secrets_file" ]; then
    rm -f "$runtime_secrets_file"
    runtime_secrets_file=""
  fi
}

deploy_worker() {
  set -- bunx wrangler deploy
  [ -z "${ADMIN_USERNAME:-}" ] || set -- "$@" --var "ADMIN_USERNAME:${ADMIN_USERNAME}"
  [ -z "${SITE_NAME:-}" ] || set -- "$@" --var "SITE_NAME:${SITE_NAME}"
  [ -z "${TURNSTILE_SITE_KEY:-}" ] || set -- "$@" --var "TURNSTILE_SITE_KEY:${TURNSTILE_SITE_KEY}"
  [ -z "${OFFLINE_THRESHOLD_SECONDS:-}" ] || set -- "$@" --var "OFFLINE_THRESHOLD_SECONDS:${OFFLINE_THRESHOLD_SECONDS}"
  [ -z "${HISTORY_RETENTION_DAYS:-}" ] || set -- "$@" --var "HISTORY_RETENTION_DAYS:${HISTORY_RETENTION_DAYS}"
  [ -z "${CF_USAGE_ACCOUNT_ID:-}" ] || set -- "$@" --var "CF_USAGE_ACCOUNT_ID:${CF_USAGE_ACCOUNT_ID}"
  [ -z "$runtime_secrets_file" ] || set -- "$@" --secrets-file "$runtime_secrets_file"
  if worker_ready; then
    NODEFLARE_ADMIN_READY=1 NODEFLARE_WORKER_READY=1 "$@"
  else
    NODEFLARE_ADMIN_READY=1 "$@"
  fi
}

if ! frontend_ready; then
  bun run build:frontend
fi

prepare_runtime_secrets

# Workers Builds provisions this binding before running the deploy command. A
# direct first deployment does not, so provision the Worker first when the
# default database is not present. The first branch uploads only once.
if bunx wrangler d1 info nodeflare --json >/dev/null 2>&1; then
  bunx wrangler d1 migrations apply DB --remote
  deploy_worker
else
  deploy_worker
  bunx wrangler d1 migrations apply DB --remote
fi
