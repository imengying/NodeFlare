#!/bin/sh
set -eu

MONITOR_BASE_URL=${MONITOR_BASE_URL:-http://127.0.0.1:8787}
MONITOR_TURNSTILE_TOKEN=${MONITOR_TURNSTILE_TOKEN:-XXXX.DUMMY.TOKEN.XXXX}
: "${MONITOR_ADMIN_USERNAME:?Set MONITOR_ADMIN_USERNAME before running the smoke test}"
: "${MONITOR_ADMIN_PASSWORD:?Set MONITOR_ADMIN_PASSWORD before running the smoke test}"

monitor_curl() {
  case "$MONITOR_BASE_URL" in
    http://127.0.0.1:*|http://localhost:*) curl --noproxy '*' "$@" ;;
    *) curl "$@" ;;
  esac
}

request() {
  monitor_curl --fail --location --silent --show-error "$@"
}

bootstrap_json=$(request "$MONITOR_BASE_URL/api/bootstrap")
printf '%s' "$bootstrap_json" | jq -e '.access == "ok" and (.servers | type == "array")' >/dev/null
config_json=$(printf '%s' "$bootstrap_json" | jq -c '.config')
printf '%s' "$config_json" | jq -e '.site_name | length > 0' >/dev/null
password_client_salt=$(printf '%s' "$config_json" | jq -er '.password_client_salt | select(length > 0)')
password_derived=$(NODEFLARE_PASSWORD="$MONITOR_ADMIN_PASSWORD" NODEFLARE_SALT="$password_client_salt" bun -e '
const encoder = new TextEncoder();
const key = await crypto.subtle.importKey("raw", encoder.encode(process.env.NODEFLARE_PASSWORD), "PBKDF2", false, ["deriveBits"]);
const bits = await crypto.subtle.deriveBits({ name: "PBKDF2", hash: "SHA-256", iterations: 600000, salt: encoder.encode(`nodeflare:${process.env.NODEFLARE_SALT}`) }, key, 256);
console.log(Array.from(new Uint8Array(bits), byte => byte.toString(16).padStart(2, "0")).join(""));
')
cross_origin_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H 'Origin: https://not-nodeflare.invalid' "$MONITOR_BASE_URL/api/bootstrap")
[ "$cross_origin_status" = "403" ]

admin_html=$(request "$MONITOR_BASE_URL/admin")
case "$admin_html" in
  *"/admin-assets/admin.js"*) ;;
  *)
    echo "Embedded admin script is missing" >&2
    exit 1
    ;;
esac
case "$admin_html" in
  *"/admin-assets/admin.css"*) ;;
  *)
    echo "Embedded admin stylesheet is missing" >&2
    exit 1
    ;;
esac
request "$MONITOR_BASE_URL/admin-assets/admin.js" | grep -q '管理面板'
request "$MONITOR_BASE_URL/admin-assets/admin.css" | grep -q 'admin-shell'

login_json=$(request -H 'Content-Type: application/json' \
  --data "$(jq -nc --arg username "$MONITOR_ADMIN_USERNAME" --arg password "$MONITOR_ADMIN_PASSWORD" --arg password_derived "$password_derived" --arg turnstile_token "$MONITOR_TURNSTILE_TOKEN" '{username:$username,password:$password,password_derived:$password_derived,turnstile_token:$turnstile_token}')" \
  "$MONITOR_BASE_URL/api/admin/login")
admin_token=$(printf '%s' "$login_json" | jq -er '.token')

# Login protection defaults to enabled, but without a complete Turnstile pair it
# remains inactive so the first admin settings save must still work.
settings_payload=$(request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/settings" | \
  jq -c '.site_description = "Smoke settings" | del(.admin_password_configured)')
request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data "$settings_payload" \
  "$MONITOR_BASE_URL/api/admin/settings" | jq -e '.settings.site_description == "Smoke settings"' >/dev/null

request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/bootstrap" | \
  jq -e '.config.site_description == "Smoke settings" and .exchange_rates.base == "CNY" and .exchange_rates.rates.CNY == 1 and .exchange_rates.rates.USD > 0 and .exchange_rates.rates.CAD > 0 and (.exchange_rates | has("cny") | not)' >/dev/null

request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/themes" | \
  jq -e '.themes | any(.builtin == true and .id == "builtin-nodeflare-glass" and .name == "NodeFlare Glass" and .active == true)' >/dev/null

server_input='{"name":"Smoke Test Node","region":"JP","group_name":"Test","tags":"smoke","hidden":false,"expires_at":1893456000,"traffic_limit":107374182400,"traffic_limit_type":"max","price":9.9,"billing_cycle":30,"currency":"USD","auto_renewal":true,"network_interface":"","reset_day":1,"report_interval":60,"collect_interval":5,"rx_correction":0,"tx_correction":0,"agent_mirror":"https://mirror.example.com","offline_notify_disabled":false,"auto_update":true}'

server_id=
latency_task_id=
alert_rule_id=
stress_report_file=
cleanup() {
  if [ -n "$stress_report_file" ]; then
    rm -f "$stress_report_file"
  fi
  if [ -n "$alert_rule_id" ]; then
    monitor_curl --silent --show-error -H "Authorization: Bearer $admin_token" \
      -X DELETE "$MONITOR_BASE_URL/api/admin/alert-rules/$alert_rule_id" >/dev/null || true
  fi
  if [ -n "$latency_task_id" ]; then
    monitor_curl --silent --show-error -H "Authorization: Bearer $admin_token" \
      -X DELETE "$MONITOR_BASE_URL/api/admin/latency-tasks/$latency_task_id" >/dev/null || true
  fi
  if [ -n "$server_id" ]; then
    monitor_curl --silent --show-error -H "Authorization: Bearer $admin_token" \
      -X DELETE "$MONITOR_BASE_URL/api/admin/servers/$server_id" >/dev/null || true
  fi
}
trap cleanup EXIT

latency_task_json=$(request -H "Authorization: Bearer $admin_token" \
  -H 'Content-Type: application/json' \
  --data '{"name":"Smoke TCP","task_type":"tcp","target":"example.com","port":443,"interval_seconds":60,"default_enabled":true,"server_ids":[]}' \
  "$MONITOR_BASE_URL/api/admin/latency-tasks")
latency_task_id=$(printf '%s' "$latency_task_json" | jq -er '.id')

server_json=$(request -H "Authorization: Bearer $admin_token" \
  -H 'Content-Type: application/json' \
  --data "$server_input" \
  "$MONITOR_BASE_URL/api/admin/servers")
server_id=$(printf '%s' "$server_json" | jq -er '.id')
agent_token=$(printf '%s' "$server_json" | jq -er '.agent_token')
token_without_admin_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id/token")
[ "$token_without_admin_status" = "401" ]
request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id/token" | \
  jq -e --arg token "$agent_token" '.agent_token == $token' >/dev/null
agent_version=$(sh scripts/resolve-version.sh)

sample=$(jq -nc --arg agent_version "$agent_version" '{
  timestamp:(now|floor),cpu:18.5,load1:0.42,load5:0.36,load15:0.31,
  mem_used:2147483648,mem_total:4294967296,swap_used:0,swap_total:0,disk_used:21474836480,disk_total:53687091200,
  net_in:4096,net_out:2048,net_rx_total:1073741824,net_tx_total:536870912,
  uptime:86400,processes:90,tcp_connections:18,udp_connections:4,cpu_cores:2,
  cpu_model:"Smoke CPU",os:"Debian 12",kernel:"6.1",arch:"x86_64",virtualization:"kvm",
  gpu_usage:32.5,gpu_model:"NVIDIA T4",disk_read_bps:4194304,disk_write_bps:2097152,
  disk_read_iops:120,disk_write_iops:48,disk_await_ms:1.4,disk_utilization:8.2,
  disks:[{name:"/dev/vda1",mount_point:"/",used:21474836480,total:53687091200,read_bps:4194304,write_bps:2097152,read_iops:120,write_iops:48,await_ms:1.4,utilization:8.2}],
  gpus:[{model:"NVIDIA T4",usage:32.5,memory_used:1073741824,memory_total:17179869184}],
  agent_version:$agent_version,latency_results:[]
}')
report=$(jq -nc --argjson sample "$sample" '{samples:[$sample]}')
invalid_token_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H 'Authorization: Bearer invalid-agent-token' -H 'Content-Type: application/json' \
  --data "$report" "$MONITOR_BASE_URL/api/agent/report")
[ "$invalid_token_status" = "401" ]
report_response=$(request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  -H 'CF-Connecting-IP: 8.8.8.8' \
  --data "$report" "$MONITOR_BASE_URL/api/agent/report")
printf '%s' "$report_response" | jq -e \
  --arg task_id "$latency_task_id" \
  '.collect_interval == 5 and .agent_mirror == "https://mirror.example.com" and .auto_update == true and (.latency_tasks | any(.id == $task_id and .task_type == "tcp" and .target == "example.com" and .port == 443))' >/dev/null
request -H "Authorization: Bearer $agent_token" \
  "$MONITOR_BASE_URL/api/agent/config" | jq -e \
  --arg task_id "$latency_task_id" \
  '.collect_interval == 5 and .agent_mirror == "https://mirror.example.com" and (.latency_tasks | any(.id == $task_id))' >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/admin/servers" | \
  jq -e --arg id "$server_id" '.servers | any(.id == $id and .last_ip == "8.8.8.8")' >/dev/null

latency_report=$(printf '%s' "$report" | jq --arg task_id "$latency_task_id" '.samples[0].timestamp=(now|floor) | .samples[0].latency_results=[{task_id:$task_id,timestamp:(now|floor),latency_ms:28.4,packet_loss:25}]')
request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$latency_report" "$MONITOR_BASE_URL/api/agent/report" | jq -e '.collect_interval == 5' >/dev/null

# A whole Agent batch must collapse into one traffic-cycle write while preserving
# counter resets. Replaying the same batch must not count any bytes twice.
traffic_report=$(printf '%s' "$report" | jq '.samples = [
  (.samples[0] | .timestamp += 2 | .net_rx_total=2147483648 | .net_tx_total=1073741824),
  (.samples[0] | .timestamp += 3 | .net_rx_total=268435456 | .net_tx_total=134217728),
  (.samples[0] | .timestamp += 4 | .net_rx_total=536870912 | .net_tx_total=268435456)
]')
request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$traffic_report" "$MONITOR_BASE_URL/api/agent/report" | jq -e '.collect_interval == 5' >/dev/null
request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$traffic_report" "$MONITOR_BASE_URL/api/agent/report" | jq -e '.collect_interval == 5' >/dev/null

request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/bootstrap" | jq -e --arg id "$server_id" --arg task_id "$latency_task_id" '.servers | any(.id == $id and .cpu == 18.5 and .gpu_usage == 32.5 and .disk_await_ms == 1.4 and (.gpus | length) == 1 and (.disks | length) == 1 and .disk_used == 21474836480 and .traffic_limit == 107374182400 and .net_rx_total == 2684354560 and .net_tx_total == 1342177280 and .price == 9.9 and (has("last_ip") | not) and (.latency | any(.task_id == $task_id and .latency_ms == 28.4 and .packet_loss == 25)))' >/dev/null

# Changing the reset day starts a new traffic cycle. Only growth after the first
# sample in that cycle counts toward usage.
request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data "$(printf '%s' "$server_input" | jq '.reset_day=2')" \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id" >/dev/null
cycle_report=$(printf '%s' "$report" | jq '.samples = [
  (.samples[0] | .timestamp += 5 | .net_rx_total=805306368 | .net_tx_total=536870912),
  (.samples[0] | .timestamp += 6 | .net_rx_total=1073741824 | .net_tx_total=805306368)
]')
request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$cycle_report" "$MONITOR_BASE_URL/api/agent/report" | jq -e '.collect_interval == 5' >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/bootstrap" | \
  jq -e --arg id "$server_id" '.servers | any(.id == $id and .net_rx_total == 268435456 and .net_tx_total == 268435456)' >/dev/null

# Exercise the protocol maximum so batch aggregation and request-size limits
# stay covered when an Agent flushes a full pending queue.
stress_report_file=$(mktemp "${TMPDIR:-/tmp}/nodeflare-stress.XXXXXX")
printf '%s' "$report" | jq '(now | floor) as $start | .samples = [
  range(0; 720) as $index |
  (.samples[0] |
    .timestamp=($start + $index + 10) |
    .net_rx_total=(1073741824 + (($index + 1) * 4096)) |
    .net_tx_total=(805306368 + (($index + 1) * 2048)))
]' >"$stress_report_file"
request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data-binary "@$stress_report_file" "$MONITOR_BASE_URL/api/agent/report" | jq -e '.collect_interval == 5' >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/history/$server_id?hours=1" | jq -e '.points | length >= 1 and any(.gpu_usage == 32.5)' >/dev/null
history_cache_header=$(monitor_curl --silent --dump-header - --output /dev/null \
  -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/history/$server_id?hours=1" | \
  tr -d '\r' | awk -F ': ' 'tolower($1) == "x-cache" { print $2 }' | tail -1)
[ "$history_cache_header" = "HIT" ]
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/latency/$server_id?hours=1" | jq -e --arg task_id "$latency_task_id" '(.tasks | any(.id == $task_id)) and (.points | any(.task_id == $task_id and .latency_ms == 28.4))' >/dev/null

# An Agent may have already measured a task when the administrator removes its
# assignment. The stale result must not block delivery of the new task list.
request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data '{"name":"Smoke TCP","task_type":"tcp","target":"example.com","port":443,"interval_seconds":60,"default_enabled":true,"server_ids":[]}' \
  "$MONITOR_BASE_URL/api/admin/latency-tasks/$latency_task_id" >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/latency/$server_id?hours=1" | jq -e --arg task_id "$latency_task_id" '(.tasks | all(.id != $task_id)) and (.points | all(.task_id != $task_id))' >/dev/null
stale_latency_response=$(request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$latency_report" "$MONITOR_BASE_URL/api/agent/report")
printf '%s' "$stale_latency_response" | jq -e --arg task_id "$latency_task_id" \
  '.latency_tasks | all(.id != $task_id)' >/dev/null

alert_rule_json=$(request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' \
  --data "$(jq -nc --arg server_id "$server_id" '{name:"Smoke CPU",metric:"cpu",threshold:80,duration_minutes:5,aggregation:"average",enabled:true,server_ids:[$server_id]}')" \
  "$MONITOR_BASE_URL/api/admin/alert-rules")
alert_rule_id=$(printf '%s' "$alert_rule_json" | jq -er '.id')
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/admin/alert-rules" | jq -e --arg id "$alert_rule_id" --arg server_id "$server_id" '.rules | any(.id == $id and .metric == "cpu" and .enabled == true and (.server_ids | index($server_id)))' >/dev/null
request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data "$(printf '%s' "$server_input" | jq '.hidden=true')" \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id" >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/bootstrap" | \
  jq -e --arg id "$server_id" '.servers | all(.id != $id)' >/dev/null
hidden_history_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/history/$server_id?hours=1")
[ "$hidden_history_status" = "404" ]

echo "Smoke test passed"
