#!/bin/sh
set -eu

LABEL="com.nodeflare.agent"
INSTALL_DIR="/usr/local/libexec/nodeflare"
AGENT_FILE="$INSTALL_DIR/agent"
PLIST_FILE="/Library/LaunchDaemons/$LABEL.plist"

safe_value() {
  case "$1" in *[!A-Za-z0-9_./:@-]*|'') return 1 ;; esac
}

if [ "${1:-}" = "--uninstall" ]; then
  [ "$#" -eq 1 ] || { echo "--uninstall does not accept additional arguments" >&2; exit 1; }
  [ "$(id -u)" -eq 0 ] || { echo "Run as root" >&2; exit 1; }
  launchctl bootout system "$PLIST_FILE" 2>/dev/null || true
  rm -f "$PLIST_FILE" "$AGENT_FILE"
  rmdir "$INSTALL_DIR" 2>/dev/null || true
  echo "NodeFlare Agent removed."
  exit 0
fi

if [ "${1:-}" = "--status" ]; then
  [ "$#" -eq 1 ] || { echo "--status does not accept additional arguments" >&2; exit 1; }
  launchctl print "system/$LABEL"
  exit $?
fi

[ "${1:-}" != "-h" ] && [ "${1:-}" != "--help" ] && [ "$#" -gt 0 ] || {
  echo "Usage: install-macos.sh -e ENDPOINT -t TOKEN -s SERVER_ID [-i 60]"
  echo "       install-macos.sh --uninstall|--status"
  exit 0
}
[ "$(id -u)" -eq 0 ] || { echo "Run as root" >&2; exit 1; }
[ "$(uname -m)" = "arm64" ] || { echo "Only Apple Silicon (arm64) is supported" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl is required" >&2; exit 1; }
command -v shasum >/dev/null || { echo "shasum is required" >&2; exit 1; }
server_id=""
token=""
endpoint=""
interval=60
interval_set=false
while [ "$#" -gt 0 ]; do
  option="$1"
  case "$option" in
    -s|-t|-e|-i)
      [ "$#" -ge 2 ] || { echo "Missing value for $option" >&2; exit 1; }
      value="$2"
      shift 2
      ;;
    *) echo "Unknown argument: $option" >&2; exit 1 ;;
  esac
  case "$option" in
    -s) [ -z "$server_id" ] || { echo "Duplicate argument: $option" >&2; exit 1; }; server_id="$value" ;;
    -t) [ -z "$token" ] || { echo "Duplicate argument: $option" >&2; exit 1; }; token="$value" ;;
    -e) [ -z "$endpoint" ] || { echo "Duplicate argument: $option" >&2; exit 1; }; endpoint="$value" ;;
    -i) [ "$interval_set" = false ] || { echo "Duplicate argument: $option" >&2; exit 1; }; interval="$value"; interval_set=true ;;
  esac
done
[ -n "$server_id" ] && [ -n "$token" ] && [ -n "$endpoint" ] || { echo "Missing required argument" >&2; exit 1; }
endpoint=${endpoint%/}
[ ${#server_id} -le 160 ] && [ ${#token} -le 512 ] && [ ${#endpoint} -le 2048 ] || { echo "Install argument is too long" >&2; exit 1; }
safe_value "$server_id" && safe_value "$token" && safe_value "$endpoint" || { echo "Invalid argument" >&2; exit 1; }
case "$endpoint" in
  https://?*|http://localhost|http://localhost/*|http://localhost:*|http://127.0.0.1|http://127.0.0.1/*|http://127.0.0.1:*) ;;
  *) echo "Worker URL must use HTTPS; HTTP is only allowed for loopback development" >&2; exit 1 ;;
esac
case "$endpoint" in *@*) echo "Endpoint must not contain user information" >&2; exit 1 ;; esac
case "$interval" in ''|*[!0-9]*) echo "Invalid interval" >&2; exit 1 ;; esac
[ "$interval" -ge 15 ] && [ "$interval" -le 3600 ] || { echo "Interval must be between 15 and 3600 seconds" >&2; exit 1; }

mkdir -p "$INSTALL_DIR"
temporary="$INSTALL_DIR/.agent.$$.download"
trap 'rm -f "$temporary"' EXIT HUP INT TERM
artifact="agent-macos-aarch64"
release_api="https://api.github.com/repos/imengying/NodeFlare/releases/latest"
release_json=$(curl --fail --location --silent --show-error --max-time 30 \
  -H 'Accept: application/vnd.github+json' \
  -H 'User-Agent: nodeflare-installer' \
  "$release_api")
release_tag=$(printf '%s\n' "$release_json" | tr ',' '\n' | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
printf '%s\n' "$release_tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || { echo "GitHub latest release has an invalid tag" >&2; exit 1; }
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
actual=$(shasum -a 256 "$temporary" | awk '{ print $1 }')
[ -n "$expected" ] && [ "$actual" = "$expected" ] || { echo "Agent checksum verification failed" >&2; exit 1; }
chmod 755 "$temporary"
installed_version=$("$temporary" --version) || { echo "Downloaded Agent cannot run on this macOS system" >&2; exit 1; }
installed_version=${installed_version##* }
[ "$installed_version" = "${release_tag#v}" ] || { echo "Release $release_tag contains Agent version $installed_version" >&2; exit 1; }
launchctl bootout system "$PLIST_FILE" 2>/dev/null || true
mv "$temporary" "$AGENT_FILE"
trap - EXIT HUP INT TERM
cat > "$PLIST_FILE" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>$LABEL</string>
<key>ProgramArguments</key><array><string>$AGENT_FILE</string><string>-e</string><string>$endpoint</string><string>-t</string><string>$token</string><string>-s</string><string>$server_id</string><string>-i</string><string>$interval</string></array>
<key>KeepAlive</key><true/><key>RunAtLoad</key><true/>
<key>StandardOutPath</key><string>/var/log/nodeflare-agent.log</string>
<key>StandardErrorPath</key><string>/var/log/nodeflare-agent.log</string>
</dict></plist>
EOF
chmod 600 "$PLIST_FILE"
launchctl bootstrap system "$PLIST_FILE"
started=false
for _ in 1 2 3 4 5 6 7 8 9 10; do
  sleep 1
  if launchctl print "system/$LABEL" 2>/dev/null | grep -q 'state = running'; then
    started=true
    break
  fi
done
[ "$started" = true ] || { echo "NodeFlare Agent failed to start; inspect /var/log/nodeflare-agent.log" >&2; exit 1; }
echo "NodeFlare Agent $installed_version installed and started."
