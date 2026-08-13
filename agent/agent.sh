#!/bin/sh
set -eu

SERVICE_NAME="nodeflare"
INSTALL_DIR="/opt/nodeflare"
AGENT_FILE="$INSTALL_DIR/agent"
SERVICE_FILE="/etc/systemd/system/$SERVICE_NAME.service"
OPENRC_FILE="/etc/init.d/$SERVICE_NAME"

usage() {
  echo "Usage: agent.sh -e ENDPOINT -t TOKEN [-i 60]"
  echo "       agent.sh --uninstall"
  echo "       agent.sh --status"
}

safe_value() {
  case "$1" in *[!A-Za-z0-9_./:@-]*|'') return 1 ;; esac
}

detect_init_system() {
  if [ -f /etc/alpine-release ] && command -v rc-service >/dev/null 2>&1; then
    printf 'openrc'
  elif [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    printf 'systemd'
  elif command -v rc-service >/dev/null 2>&1 && [ -d /etc/init.d ]; then
    printf 'openrc'
  else
    printf 'unknown'
  fi
}

ensure_curl() {
  command -v curl >/dev/null 2>&1 && return
  echo "curl not found; installing it..."
  if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install -y curl
  elif command -v dnf >/dev/null 2>&1; then
    dnf install -y curl
  elif command -v yum >/dev/null 2>&1; then
    yum install -y curl
  elif command -v apk >/dev/null 2>&1; then
    apk add --no-cache curl
  else
    echo "curl is required and no supported package manager was found" >&2
    exit 1
  fi
  command -v curl >/dev/null 2>&1 || { echo "Failed to install curl" >&2; exit 1; }
}

explain_exec_failure() {
  target="$1"
  if [ ! -f "$target" ]; then
    echo "Downloaded Agent disappeared before validation: $target" >&2
  elif [ ! -x "$target" ]; then
    echo "Downloaded Agent is not executable: $target" >&2
  elif [ "$(dd if="$target" bs=4 count=1 2>/dev/null)" != "$(printf '\177ELF')" ]; then
    echo "Downloaded asset is not a Linux ELF executable" >&2
  else
    echo "Downloaded Agent could not run on this system." >&2
    echo "Use a release built with the current static Linux targets, then retry installation." >&2
  fi
}

install_agent() {
  [ "$(id -u)" -eq 0 ] || { echo "Run install as root" >&2; exit 1; }
  ensure_curl
  command -v sha256sum >/dev/null || { echo "sha256sum is required" >&2; exit 1; }
  token=""
  endpoint=""
  interval=60
  interval_set=false
  while [ "$#" -gt 0 ]; do
    option="$1"
    case "$option" in
      -t|-e|-i)
        [ "$#" -ge 2 ] || { echo "Missing value for $option" >&2; usage; exit 1; }
        value="$2"
        shift 2
        ;;
      *) echo "Unknown argument: $option" >&2; usage; exit 1 ;;
    esac
    case "$option" in
      -t) [ -z "$token" ] || { echo "Duplicate argument: $option" >&2; exit 1; }; token="$value" ;;
      -e) [ -z "$endpoint" ] || { echo "Duplicate argument: $option" >&2; exit 1; }; endpoint="$value" ;;
      -i) [ "$interval_set" = false ] || { echo "Duplicate argument: $option" >&2; exit 1; }; interval="$value"; interval_set=true ;;
    esac
  done
  [ -n "$token" ] && [ -n "$endpoint" ] || { usage; exit 1; }
  endpoint=${endpoint%/}
  [ ${#token} -le 512 ] && [ ${#endpoint} -le 2048 ] || { echo "Install argument is too long" >&2; exit 1; }
  safe_value "$token" && safe_value "$endpoint" || { echo "Invalid install argument" >&2; exit 1; }
  case "$endpoint" in
    https://?*|http://localhost|http://localhost/*|http://localhost:*|http://127.0.0.1|http://127.0.0.1/*|http://127.0.0.1:*) ;;
    *) echo "Worker URL must use HTTPS; HTTP is only allowed for loopback development" >&2; exit 1 ;;
  esac
  case "$endpoint" in *@*) echo "Endpoint must not contain user information" >&2; exit 1 ;; esac
  case "$interval" in ''|*[!0-9]*) echo "Invalid interval" >&2; exit 1 ;; esac
  [ "$interval" -ge 15 ] && [ "$interval" -le 3600 ] || { echo "Interval must be between 15 and 3600 seconds" >&2; exit 1; }
  case "$(uname -m)" in x86_64|amd64) arch="x86_64" ;; aarch64|arm64) arch="aarch64" ;; *) echo "Unsupported architecture" >&2; exit 1 ;; esac
  init_system=$(detect_init_system)
  [ "$init_system" != "unknown" ] || { echo "A running systemd or OpenRC installation is required" >&2; exit 1; }

  mkdir -p "$INSTALL_DIR"
  temporary="$INSTALL_DIR/.agent.$$.download"
  trap 'rm -f "$temporary"' EXIT HUP INT TERM
  artifact="agent-linux-$arch"
  release_api="https://api.github.com/repos/imengying/NodeFlare/releases/latest"
  release_json=$(curl --fail --location --silent --show-error --max-time 30 \
    -H 'Accept: application/vnd.github+json' \
    -H 'User-Agent: nodeflare-installer' \
    "$release_api")
  release_tag=$(printf '%s\n' "$release_json" | tr ',' '\n' | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
  printf '%s\n' "$release_tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || {
    echo "GitHub latest release has an invalid tag: ${release_tag:-missing}" >&2
    exit 1
  }
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
  release_base="https://github.com/imengying/NodeFlare/releases/download/$release_tag"
  curl --fail --location --silent --show-error --max-time 120 \
    "$release_base/$artifact" \
    -o "$temporary"
  actual=$(sha256sum "$temporary" | awk '{ print $1 }')
  [ -n "$expected" ] && [ "$actual" = "$expected" ] || { echo "Agent checksum verification failed" >&2; exit 1; }
  chmod 755 "$temporary"
  if ! installed_version=$("$temporary" --version); then
    explain_exec_failure "$temporary"
    exit 1
  fi
  installed_version=${installed_version##* }
  printf '%s\n' "$installed_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || {
    echo "Downloaded Agent returned an invalid version: $installed_version" >&2
    exit 1
  }
  [ "$installed_version" = "${release_tag#v}" ] || {
    echo "Release $release_tag contains Agent version $installed_version; refusing to install it" >&2
    exit 1
  }
  case "$init_system" in
    systemd) systemctl stop "$SERVICE_NAME" 2>/dev/null || true ;;
    openrc) rc-service "$SERVICE_NAME" stop 2>/dev/null || true ;;
  esac
  mv "$temporary" "$AGENT_FILE"
  trap - EXIT HUP INT TERM
  if [ "$init_system" = "systemd" ]; then
    printf '%s\n' \
    '[Unit]' \
    'Description=NodeFlare Rust Agent' \
    'After=network-online.target' \
    'Wants=network-online.target' \
    '' \
    '[Service]' \
    'Type=simple' \
    "ExecStart=$AGENT_FILE -e $endpoint -t $token -i $interval" \
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
    systemctl is-active --quiet "$SERVICE_NAME" || {
      systemctl status "$SERVICE_NAME" --no-pager >&2 || true
      echo "NodeFlare Agent service failed to start" >&2
      exit 1
    }
  elif [ "$init_system" = "openrc" ]; then
    printf '%s\n' \
      '#!/sbin/openrc-run' \
      "name=\"$SERVICE_NAME\"" \
      "command=\"$AGENT_FILE\"" \
      "command_args=\"-e $endpoint -t $token -i $interval\"" \
      "command_user=\"root\"" \
      "supervisor=\"supervise-daemon\"" \
      "respawn_delay=10" \
      'depend() { need net; }' > "$OPENRC_FILE"
    chmod 755 "$OPENRC_FILE"
    rc-update add "$SERVICE_NAME" default
    rc-service "$SERVICE_NAME" restart
    rc-service "$SERVICE_NAME" status >/dev/null || {
      rc-service "$SERVICE_NAME" status >&2 || true
      echo "NodeFlare Agent service failed to start" >&2
      exit 1
    }
  fi
  echo "NodeFlare Agent $installed_version installed with $init_system."
}

status_agent() {
  init_system=$(detect_init_system)
  if [ "$init_system" = "systemd" ]; then
    if systemctl status "$SERVICE_NAME" --no-pager; then return 0; else return $?; fi
  fi
  if [ "$init_system" = "openrc" ]; then
    rc-service "$SERVICE_NAME" status
    return $?
  fi
  echo "NodeFlare agent service is not installed." >&2
  return 1
}

uninstall_agent() {
  [ "$(id -u)" -eq 0 ] || { echo "Run uninstall as root" >&2; exit 1; }
  init_system=$(detect_init_system)
  case "$init_system" in
    systemd) systemctl disable --now "$SERVICE_NAME" 2>/dev/null || true ;;
    openrc)
      rc-service "$SERVICE_NAME" stop 2>/dev/null || true
      rc-update del "$SERVICE_NAME" default 2>/dev/null || true
      ;;
  esac
  rm -f "$SERVICE_FILE"
  rm -f "$OPENRC_FILE"
  [ "$init_system" != "systemd" ] || systemctl daemon-reload 2>/dev/null || true
  rm -rf "$INSTALL_DIR"
  echo "NodeFlare agent removed."
}

case "${1:-}" in
  --uninstall) [ "$#" -eq 1 ] || { usage; exit 1; }; uninstall_agent ;;
  --status) [ "$#" -eq 1 ] || { usage; exit 1; }; status_agent ;;
  -h|--help|'') usage; exit 0 ;;
  *) install_agent "$@" ;;
esac
