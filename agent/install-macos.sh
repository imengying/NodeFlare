#!/bin/sh
set -eu

LABEL="com.cfmonitor.agent"
INSTALL_DIR="/usr/local/libexec/cf-monitor"
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
  echo "CF Monitor Agent removed."
  exit 0
fi

[ "${1:-}" = "install" ] || { echo "Usage: install-macos.sh install --server-id ID --token TOKEN --url URL" >&2; exit 1; }
shift
[ "$(id -u)" -eq 0 ] || { echo "Run as root" >&2; exit 1; }
server_id=$(arg --server-id "$@") || exit 1
token=$(arg --token "$@") || exit 1
worker_url=$(arg --url "$@") || exit 1
interval=$(arg --interval "$@" || printf '60')
collect_interval=$(arg --collect-interval "$@" || printf '5')
worker_url=${worker_url%/}
safe_value "$server_id" && safe_value "$token" && safe_value "$worker_url" || { echo "Invalid argument" >&2; exit 1; }

mkdir -p "$INSTALL_DIR"
curl -fL "https://github.com/imengying/CF-Monitor/releases/latest/download/agent-macos-aarch64" -o "$AGENT_FILE.download"
chmod 755 "$AGENT_FILE.download"
"$AGENT_FILE.download" version >/dev/null
mv "$AGENT_FILE.download" "$AGENT_FILE"

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
<key>StandardOutPath</key><string>/var/log/cf-monitor-agent.log</string>
<key>StandardErrorPath</key><string>/var/log/cf-monitor-agent.log</string>
</dict></plist>
EOF
chmod 600 "$PLIST_FILE"
launchctl bootout system "$PLIST_FILE" 2>/dev/null || true
launchctl bootstrap system "$PLIST_FILE"
echo "CF Monitor Agent installed and started."
