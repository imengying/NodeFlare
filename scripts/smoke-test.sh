#!/bin/sh
set -eu

MONITOR_BASE_URL=${MONITOR_BASE_URL:-http://127.0.0.1:8787}
MONITOR_ADMIN_USERNAME=${MONITOR_ADMIN_USERNAME:-admin}
MONITOR_TURNSTILE_TOKEN=${MONITOR_TURNSTILE_TOKEN:-XXXX.DUMMY.TOKEN.XXXX}
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

config_json=$(request "$MONITOR_BASE_URL/api/config")
printf '%s' "$config_json" | jq -e '.site_name | length > 0' >/dev/null
password_client_salt=$(printf '%s' "$config_json" | jq -er '.password_client_salt | select(length > 0)')
password_derived=$(NODEFLARE_PASSWORD="$MONITOR_ADMIN_PASSWORD" NODEFLARE_SALT="$password_client_salt" bun -e '
const encoder = new TextEncoder();
const key = await crypto.subtle.importKey("raw", encoder.encode(process.env.NODEFLARE_PASSWORD), "PBKDF2", false, ["deriveBits"]);
const bits = await crypto.subtle.deriveBits({ name: "PBKDF2", hash: "SHA-256", iterations: 600000, salt: encoder.encode(`nodeflare:${process.env.NODEFLARE_SALT}`) }, key, 256);
console.log(Array.from(new Uint8Array(bits), byte => byte.toString(16).padStart(2, "0")).join(""));
')
cross_origin_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H 'Origin: https://not-nodeflare.invalid' "$MONITOR_BASE_URL/api/config")
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
request "$MONITOR_BASE_URL/admin-assets/admin.js" >/dev/null
request "$MONITOR_BASE_URL/admin-assets/admin.css" >/dev/null

login_json=$(request -H 'Content-Type: application/json' \
  --data "$(jq -nc --arg username "$MONITOR_ADMIN_USERNAME" --arg password "$MONITOR_ADMIN_PASSWORD" --arg password_derived "$password_derived" --arg turnstile_token "$MONITOR_TURNSTILE_TOKEN" '{username:$username,password:$password,password_derived:$password_derived,turnstile_token:$turnstile_token}')" \
  "$MONITOR_BASE_URL/api/admin/login")
admin_token=$(printf '%s' "$login_json" | jq -er '.token')

# Login protection defaults to enabled, but without a complete Turnstile pair it
# remains inactive so the first admin settings save must still work.
request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data '{"site_description":"Smoke settings"}' \
  "$MONITOR_BASE_URL/api/admin/settings" | jq -e '.settings.site_description == "Smoke settings"' >/dev/null

request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/exchange-rates" | \
  jq -e '.base == "CNY" and .rates.CNY == 1 and .rates.USD > 0 and .rates.CAD > 0 and (has("cny") | not)' >/dev/null

request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/themes" | \
  jq -e '.themes | any(.builtin == true and .id == "builtin-komari-glass" and .active == true)' >/dev/null

server_input='{"name":"Smoke Test Node","region":"JP","group_name":"Test","tags":"smoke","note":"private","hidden":false,"expires_at":1893456000,"traffic_limit":107374182400,"traffic_limit_type":"max","price":9.9,"billing_cycle":30,"currency":"USD","auto_renewal":true,"public_remark":"public","network_interface":"","reset_day":1,"report_interval":60,"collect_interval":5,"rx_correction":0,"tx_correction":0,"offline_notify_disabled":false,"auto_update":true}'

server_id=
latency_task_id=
alert_rule_id=
cleanup() {
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
  --data '{"name":"Smoke TCP","task_type":"tcp","target":"example.com:443","interval_seconds":60,"default_enabled":true,"server_ids":[]}' \
  "$MONITOR_BASE_URL/api/admin/latency-tasks")
latency_task_id=$(printf '%s' "$latency_task_json" | jq -er '.id')

server_json=$(request -H "Authorization: Bearer $admin_token" \
  -H 'Content-Type: application/json' \
  --data "$server_input" \
  "$MONITOR_BASE_URL/api/admin/servers")
server_id=$(printf '%s' "$server_json" | jq -er '.id')
agent_token=$(printf '%s' "$server_json" | jq -er '.agent_token')
agent_version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' agent/Cargo.toml | head -1)

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
report=$(jq -nc --arg server_id "$server_id" --argjson sample "$sample" '{server_id:$server_id,samples:[$sample]}')
report_response=$(request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$report" "$MONITOR_BASE_URL/api/agent/report")
printf '%s' "$report_response" | jq -e \
  --arg task_id "$latency_task_id" \
  '.collect_interval == 5 and .auto_update == true and (.latency_tasks | any(.id == $task_id and .task_type == "tcp" and .target == "example.com:443"))' >/dev/null

latency_report=$(printf '%s' "$report" | jq --arg task_id "$latency_task_id" '.samples[0].timestamp=(now|floor) | .samples[0].latency_results=[{task_id:$task_id,timestamp:(now|floor),latency_ms:28.4,packet_loss:25}]')
request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$latency_report" "$MONITOR_BASE_URL/api/agent/report" | jq -e '.collect_interval == 5' >/dev/null

# Traffic counters may reset when a host reboots. Cycle usage must retain the
# bytes already observed before the reset and continue from the new counter.
traffic_report=$(printf '%s' "$report" | jq '.samples[0].timestamp += 2 | .samples[0].net_rx_total=2147483648 | .samples[0].net_tx_total=1073741824')
request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$traffic_report" "$MONITOR_BASE_URL/api/agent/report" | jq -e '.collect_interval == 5' >/dev/null
reset_report=$(printf '%s' "$report" | jq '.samples[0].timestamp += 3 | .samples[0].net_rx_total=268435456 | .samples[0].net_tx_total=134217728')
request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$reset_report" "$MONITOR_BASE_URL/api/agent/report" | jq -e '.collect_interval == 5' >/dev/null

request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/servers" | jq -e --arg id "$server_id" --arg task_id "$latency_task_id" '.servers | any(.id == $id and .cpu == 18.5 and .gpu_usage == 32.5 and .disk_await_ms == 1.4 and (.gpus | length) == 1 and (.disks | length) == 1 and .disk_used == 21474836480 and .traffic_limit == 107374182400 and .net_rx_total == 2415919104 and .net_tx_total == 1207959552 and .price == 9.9 and (has("note") | not) and .public_remark == "public" and (.latency | any(.task_id == $task_id and .latency_ms == 28.4 and .packet_loss == 25)))' >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/history/$server_id?hours=1" | jq -e '.points | length >= 1 and any(.gpu_usage == 32.5)' >/dev/null
history_cache_header=$(monitor_curl --silent --dump-header - --output /dev/null \
  -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/history/$server_id?hours=1" | \
  tr -d '\r' | awk -F ': ' 'tolower($1) == "x-cache" { print $2 }' | tail -1)
[ "$history_cache_header" = "HIT" ]
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/latency/$server_id?hours=1" | jq -e --arg task_id "$latency_task_id" '(.tasks | any(.id == $task_id)) and (.points | any(.task_id == $task_id and .latency_ms == 28.4))' >/dev/null

# An Agent may have already measured a task when the administrator removes its
# assignment. The stale result must not block delivery of the new task list.
request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data '{"name":"Smoke TCP","task_type":"tcp","target":"example.com:443","interval_seconds":60,"default_enabled":true,"server_ids":[]}' \
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
rotated_json=$(request -H "Authorization: Bearer $admin_token" -X POST \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id/token")
rotated_token=$(printf '%s' "$rotated_json" | jq -er '.agent_token')

revoked_token_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$report" "$MONITOR_BASE_URL/api/agent/report")
[ "$revoked_token_status" = "401" ]
request -H "Authorization: Bearer $rotated_token" -H 'Content-Type: application/json' \
  --data "$latency_report" "$MONITOR_BASE_URL/api/agent/report" | jq -e '.collect_interval == 5' >/dev/null

request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data "$(printf '%s' "$server_input" | jq '.hidden=true')" \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id" >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/servers" | \
  jq -e --arg id "$server_id" '.servers | all(.id != $id)' >/dev/null
hidden_history_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/history/$server_id?hours=1")
[ "$hidden_history_status" = "404" ]

echo "Smoke test passed"
