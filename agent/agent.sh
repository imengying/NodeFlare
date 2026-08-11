#!/bin/sh
set -eu

SERVICE_NAME="nodeflare-agent"
INSTALL_DIR="/opt/nodeflare"
AGENT_FILE="$INSTALL_DIR/agent"
CONFIG_FILE="$INSTALL_DIR/agent.env"
SERVICE_FILE="/etc/systemd/system/$SERVICE_NAME.service"
OPENRC_FILE="/etc/init.d/$SERVICE_NAME"

usage() {
  echo "Usage: agent.sh install --server-id ID --token TOKEN --url WORKER_URL [--interval 60] [--collect-interval 5]"
  echo "       agent.sh uninstall"
  echo "       agent.sh status"
}

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

install_agent() {
  [ "$(id -u)" -eq 0 ] || { echo "Run install as root" >&2; exit 1; }
  command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
  command -v sha256sum >/dev/null || { echo "sha256sum is required" >&2; exit 1; }
  server_id=$(arg --server-id "$@") || { usage; exit 1; }
  token=$(arg --token "$@") || { usage; exit 1; }
  worker_url=$(arg --url "$@") || { usage; exit 1; }
  interval=$(arg --interval "$@" || printf '60')
  collect_interval=$(arg --collect-interval "$@" || printf '5')
  worker_url=${worker_url%/}
  safe_value "$server_id" && safe_value "$token" && safe_value "$worker_url" || { echo "Invalid install argument" >&2; exit 1; }
  case "$worker_url" in http://?*|https://?*) ;; *) echo "Worker URL must use HTTP or HTTPS" >&2; exit 1 ;; esac
  case "$interval" in ''|*[!0-9]*) echo "Invalid interval" >&2; exit 1 ;; esac
  [ "$interval" -ge 15 ] && [ "$interval" -le 3600 ] || { echo "Interval must be between 15 and 3600 seconds" >&2; exit 1; }
  case "$collect_interval" in ''|*[!0-9]*) echo "Invalid collect interval" >&2; exit 1 ;; esac
  [ "$collect_interval" -ge 2 ] && [ "$collect_interval" -le 60 ] && [ "$collect_interval" -le "$interval" ] || { echo "Collect interval must be between 2 and 60 seconds and no greater than report interval" >&2; exit 1; }
  case "$(uname -m)" in x86_64|amd64) arch="x86_64" ;; aarch64|arm64) arch="aarch64" ;; *) echo "Unsupported architecture" >&2; exit 1 ;; esac

  mkdir -p "$INSTALL_DIR"
  temporary="$INSTALL_DIR/.agent.$$.download"
  trap 'rm -f "$temporary"' EXIT HUP INT TERM
  artifact="agent-linux-$arch"
  release_base="https://github.com/imengying/NodeFlare/releases/latest/download"
  curl --fail --location --silent --show-error --max-time 120 \
    "$release_base/$artifact" \
    -o "$temporary"
  checksums=$(curl --fail --location --silent --show-error --max-time 30 "$release_base/SHA256SUMS")
  expected=$(printf '%s\n' "$checksums" | awk -v name="$artifact" '$2 == name { print $1; exit }')
  actual=$(sha256sum "$temporary" | awk '{ print $1 }')
  [ -n "$expected" ] && [ "$actual" = "$expected" ] || { echo "Agent checksum verification failed" >&2; exit 1; }
  chmod 755 "$temporary"
  "$temporary" version >/dev/null
  systemctl stop "$SERVICE_NAME" 2>/dev/null || true
  rc-service "$SERVICE_NAME" stop 2>/dev/null || true
  mv "$temporary" "$AGENT_FILE"
  trap - EXIT HUP INT TERM
  {
    printf 'SERVER_ID=%s\n' "$server_id"
    printf 'AGENT_TOKEN=%s\n' "$token"
    printf 'WORKER_URL=%s\n' "$worker_url"
    printf 'REPORT_INTERVAL=%s\n' "$interval"
    printf 'COLLECT_INTERVAL=%s\n' "$collect_interval"
    printf 'NETWORK_INTERFACE=\n'
  } > "$CONFIG_FILE"
  chmod 600 "$CONFIG_FILE"

  if command -v systemctl >/dev/null 2>&1; then
    printf '%s\n' \
    '[Unit]' \
    'Description=NodeFlare Rust Agent' \
    'After=network-online.target' \
    'Wants=network-online.target' \
    '' \
    '[Service]' \
    'Type=simple' \
    "EnvironmentFile=$CONFIG_FILE" \
    "ExecStart=$AGENT_FILE run" \
    'Restart=always' \
    'RestartSec=10' \
    'NoNewPrivileges=true' \
    'PrivateTmp=true' \
    'ProtectHome=true' \
    'ProtectSystem=strict' \
    "ReadWritePaths=$INSTALL_DIR" \
    '' \
    '[Install]' \
      'WantedBy=multi-user.target' > "$SERVICE_FILE"
    systemctl daemon-reload
    systemctl enable --now "$SERVICE_NAME"
  elif command -v rc-service >/dev/null 2>&1; then
    printf '%s\n' \
      '#!/sbin/openrc-run' \
      "name=\"$SERVICE_NAME\"" \
      "command=\"$AGENT_FILE\"" \
      'command_args="run"' \
      "command_user=\"root\"" \
      "supervisor=\"supervise-daemon\"" \
      "respawn_delay=10" \
      ". \"$CONFIG_FILE\"" \
      'export SERVER_ID AGENT_TOKEN WORKER_URL REPORT_INTERVAL COLLECT_INTERVAL NETWORK_INTERFACE' \
      'depend() { need net; }' > "$OPENRC_FILE"
    chmod 755 "$OPENRC_FILE"
    rc-update add "$SERVICE_NAME" default
    rc-service "$SERVICE_NAME" restart
  else
    echo "systemd or OpenRC is required" >&2
    exit 1
  fi
  echo "NodeFlare Rust agent installed."
}

status_agent() {
  if command -v systemctl >/dev/null 2>&1; then
    if systemctl status "$SERVICE_NAME" --no-pager; then return 0; else return $?; fi
  fi
  if command -v rc-service >/dev/null 2>&1; then
    rc-service "$SERVICE_NAME" status
    return $?
  fi
  echo "NodeFlare agent service is not installed." >&2
  return 1
}

uninstall_agent() {
  [ "$(id -u)" -eq 0 ] || { echo "Run uninstall as root" >&2; exit 1; }
  systemctl disable --now "$SERVICE_NAME" 2>/dev/null || true
  rc-service "$SERVICE_NAME" stop 2>/dev/null || true
  rc-update del "$SERVICE_NAME" default 2>/dev/null || true
  rm -f "$SERVICE_FILE"
  rm -f "$OPENRC_FILE"
  systemctl daemon-reload 2>/dev/null || true
  rm -rf "$INSTALL_DIR"
  echo "NodeFlare agent removed."
}

case "${1:-}" in
  install) shift; install_agent "$@" ;;
  uninstall) uninstall_agent ;;
  status) status_agent ;;
  *) usage; exit 1 ;;
esac
