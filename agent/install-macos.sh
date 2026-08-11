#!/bin/sh
set -eu

LABEL="com.nodeflare.agent"
INSTALL_DIR="/usr/local/libexec/nodeflare"
AGENT_FILE="$INSTALL_DIR/agent"
PLIST_FILE="/Library/LaunchDaemons/$LABEL.plist"

arg() {
  key="$1"; shift
  while [ "$#" -gt 1 ]; do
    [ "$1" = "$key" ] && { printf '%s' "$2"; return 0; }
    shift
  done
  return 1
}

safe_value() {
  case "$1" in *[!A-Za-z0-9_./:@-]*|'') return 1 ;; esac
}

if [ "${1:-}" = "uninstall" ]; then
  [ "$(id -u)" -eq 0 ] || { echo "Run as root" >&2; exit 1; }
  launchctl bootout system "$PLIST_FILE" 2>/dev/null || true
  rm -f "$PLIST_FILE" "$AGENT_FILE"
  rmdir "$INSTALL_DIR" 2>/dev/null || true
  echo "NodeFlare Agent removed."
  exit 0
fi

if [ "${1:-}" = "status" ]; then
  launchctl print "system/$LABEL"
  exit $?
fi

[ "${1:-}" = "install" ] || { echo "Usage: install-macos.sh <install|uninstall|status> --server-id ID --token TOKEN --url URL" >&2; exit 1; }
shift
[ "$(id -u)" -eq 0 ] || { echo "Run as root" >&2; exit 1; }
[ "$(uname -m)" = "arm64" ] || { echo "Only Apple Silicon (arm64) is supported" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
command -v shasum >/dev/null || { echo "shasum is required" >&2; exit 1; }
server_id=$(arg --server-id "$@") || exit 1
token=$(arg --token "$@") || exit 1
worker_url=$(arg --url "$@") || exit 1
interval=$(arg --interval "$@" || printf '60')
collect_interval=$(arg --collect-interval "$@" || printf '5')
worker_url=${worker_url%/}
safe_value "$server_id" && safe_value "$token" && safe_value "$worker_url" || { echo "Invalid argument" >&2; exit 1; }
case "$worker_url" in
  https://?*|http://localhost|http://localhost/*|http://localhost:*|http://127.0.0.1|http://127.0.0.1/*|http://127.0.0.1:*) ;;
  *) echo "Worker URL must use HTTPS; HTTP is only allowed for loopback development" >&2; exit 1 ;;
esac
case "$worker_url" in *@*) echo "Worker URL must not contain user information" >&2; exit 1 ;; esac
case "$interval" in ''|*[!0-9]*) echo "Invalid interval" >&2; exit 1 ;; esac
[ "$interval" -ge 15 ] && [ "$interval" -le 3600 ] || { echo "Interval must be between 15 and 3600 seconds" >&2; exit 1; }
case "$collect_interval" in ''|*[!0-9]*) echo "Invalid collect interval" >&2; exit 1 ;; esac
[ "$collect_interval" -ge 2 ] && [ "$collect_interval" -le 60 ] && [ "$collect_interval" -le "$interval" ] || { echo "Collect interval must be between 2 and 60 seconds and no greater than report interval" >&2; exit 1; }

mkdir -p "$INSTALL_DIR"
temporary="$INSTALL_DIR/.agent.$$.download"
trap 'rm -f "$temporary"' EXIT HUP INT TERM
artifact="agent-macos-aarch64"
release_api="https://api.github.com/repos/imengying/NodeFlare/releases/latest"
release_json=$(curl --fail --location --silent --show-error --max-time 30 \
  -H 'Accept: application/vnd.github+json' \
  -H 'User-Agent: nodeflare-installer' \
  "$release_api")
expected=$(printf '%s\n' "$release_json" | tr '{' '\n' | awk -v name="$artifact" '
  {
    compact = $0
    gsub(/[[:space:]]/, "", compact)
    if (index(compact, "\"name\":\"" name "\"") > 0) selected = 1
    else if (index(compact, "\"name\":") > 0) selected = 0
  }
  selected {
    marker = "\"digest\":\"sha256:"
    position = index(compact, marker)
    if (position == 0) next
    digest = substr(compact, position + length(marker), 64)
    if (length(digest) == 64 && digest !~ /[^0-9a-fA-F]/) {
      print tolower(digest)
      exit
    }
  }
')
[ -n "$expected" ] || { echo "GitHub release does not contain a SHA-256 digest for $artifact" >&2; exit 1; }
release_base="https://github.com/imengying/NodeFlare/releases/latest/download"
curl --fail --location --silent --show-error --max-time 120 \
  "$release_base/$artifact" \
  -o "$temporary"
actual=$(shasum -a 256 "$temporary" | awk '{ print $1 }')
[ -n "$expected" ] && [ "$actual" = "$expected" ] || { echo "Agent checksum verification failed" >&2; exit 1; }
chmod 755 "$temporary"
"$temporary" version >/dev/null
launchctl bootout system "$PLIST_FILE" 2>/dev/null || true
mv "$temporary" "$AGENT_FILE"
trap - EXIT HUP INT TERM

cat > "$PLIST_FILE" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>$LABEL</string>
<key>ProgramArguments</key><array><string>$AGENT_FILE</string><string>run</string></array>
<key>EnvironmentVariables</key><dict>
<key>SERVER_ID</key><string>$server_id</string>
<key>AGENT_TOKEN</key><string>$token</string>
<key>WORKER_URL</key><string>$worker_url</string>
<key>REPORT_INTERVAL</key><string>$interval</string>
<key>COLLECT_INTERVAL</key><string>$collect_interval</string>
</dict>
<key>KeepAlive</key><true/><key>RunAtLoad</key><true/>
<key>StandardOutPath</key><string>/var/log/nodeflare-agent.log</string>
<key>StandardErrorPath</key><string>/var/log/nodeflare-agent.log</string>
</dict></plist>
EOF
chmod 600 "$PLIST_FILE"
launchctl bootstrap system "$PLIST_FILE"
echo "NodeFlare Agent installed and started."
