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

request "$MONITOR_BASE_URL/api/config" | jq -e '.site_name | length > 0' >/dev/null

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
  --data "$(jq -nc --arg username "$MONITOR_ADMIN_USERNAME" --arg password "$MONITOR_ADMIN_PASSWORD" --arg turnstile_token "$MONITOR_TURNSTILE_TOKEN" '{username:$username,password:$password,turnstile_token:$turnstile_token}')" \
  "$MONITOR_BASE_URL/api/admin/login")
admin_token=$(printf '%s' "$login_json" | jq -er '.token')

request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/exchange-rates" | \
  jq -e '.base == "CNY" and .rates.CNY == 1 and .rates.USD > 0 and .rates.CAD > 0 and .cny.USD == .rates.USD' >/dev/null

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
  --data '{"name":"Smoke Test Node","region":"JP","group_name":"Test","tags":"smoke","note":"private","public_remark":"public","hidden":false,"expires_at":1893456000,"traffic_limit":107374182400,"traffic_limit_type":"max","price":9.9,"billing_cycle":30,"currency":"USD","auto_renewal":true}' \
  "$MONITOR_BASE_URL/api/admin/servers")
server_id=$(printf '%s' "$server_json" | jq -er '.id')
agent_token=$(printf '%s' "$server_json" | jq -er '.agent_token')
agent_version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' agent/Cargo.toml | head -1)

sample=$(jq -nc --arg agent_version "$agent_version" '{
  timestamp:(now|floor),cpu:18.5,load1:0.42,load5:0.36,load15:0.31,
  mem_used:2147483648,mem_total:4294967296,disk_used:21474836480,disk_total:53687091200,
  net_in:4096,net_out:2048,net_rx_total:1073741824,net_tx_total:536870912,
  uptime:86400,processes:90,tcp_connections:18,udp_connections:4,cpu_cores:2,
  cpu_model:"Smoke CPU",os:"Debian 12",kernel:"6.1",arch:"x86_64",virtualization:"kvm",
  gpu_usage:32.5,gpu_model:"NVIDIA T4",disk_read_bps:4194304,disk_write_bps:2097152,
  disk_read_iops:120,disk_write_iops:48,disk_await_ms:1.4,disk_utilization:8.2,
  disks:[{name:"/dev/vda1",mount_point:"/",used:21474836480,total:53687091200,read_bps:4194304,write_bps:2097152,read_iops:120,write_iops:48,await_ms:1.4,utilization:8.2}],
  gpus:[{model:"NVIDIA T4",usage:32.5,memory_used:1073741824,memory_total:17179869184}],
  agent_version:$agent_version,message:"smoke"
}')
report=$(jq -nc --arg server_id "$server_id" --argjson sample "$sample" '{server_id:$server_id,samples:[$sample]}')
report_response=$(request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$report" "$MONITOR_BASE_URL/api/agent/report")
printf '%s' "$report_response" | jq -e \
  --arg task_id "$latency_task_id" \
  --arg agent_version "$agent_version" \
  '.success == true and .config.collect_interval == 5 and .config.latest_agent_version == $agent_version and (.config.latency_tasks | any(.id == $task_id and .task_type == "tcp" and .target == "example.com:443"))' >/dev/null

latency_report=$(printf '%s' "$report" | jq --arg task_id "$latency_task_id" '.samples[0].timestamp=(now|floor) | .samples[0].latency_results=[{task_id:$task_id,timestamp:(now|floor),latency_ms:28.4,packet_loss:25}]')
request -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$latency_report" "$MONITOR_BASE_URL/api/agent/report" | jq -e '.success == true' >/dev/null

request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/servers/$server_id" | jq -e --arg task_id "$latency_task_id" '.cpu == 18.5 and .gpu_usage == 32.5 and .disk_await_ms == 1.4 and (.gpus | length) == 1 and (.disks | length) == 1 and .price == 9.9 and .note == "" and .public_remark == "public" and (.latency | any(.task_id == $task_id and .latency_ms == 28.4 and .packet_loss == 25))' >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/history/$server_id?hours=1" | jq -e '.points | length >= 1 and any(.gpu_usage == 32.5)' >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/latency/$server_id?hours=1" | jq -e --arg task_id "$latency_task_id" '(.tasks | any(.id == $task_id)) and (.points | any(.task_id == $task_id and .latency_ms == 28.4))' >/dev/null
alert_rule_json=$(request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' \
  --data "$(jq -nc --arg server_id "$server_id" '{name:"Smoke CPU",metric:"cpu",threshold:80,duration_minutes:5,aggregation:"average",enabled:true,server_ids:[$server_id]}')" \
  "$MONITOR_BASE_URL/api/admin/alert-rules")
alert_rule_id=$(printf '%s' "$alert_rule_json" | jq -er '.id')
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/admin/alert-rules" | jq -e --arg id "$alert_rule_id" --arg server_id "$server_id" '.rules | any(.id == $id and .metric == "cpu" and (.server_ids | index($server_id)))' >/dev/null
rotated_json=$(request -H "Authorization: Bearer $admin_token" -X POST \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id/token")
rotated_token=$(printf '%s' "$rotated_json" | jq -er '.agent_token')

old_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H "Authorization: Bearer $agent_token" -H 'Content-Type: application/json' \
  --data "$report" "$MONITOR_BASE_URL/api/agent/report")
[ "$old_status" = "401" ]
request -H "Authorization: Bearer $rotated_token" -H 'Content-Type: application/json' \
  --data "$latency_report" "$MONITOR_BASE_URL/api/agent/report" | jq -e '.success == true' >/dev/null

request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data '{"name":"Smoke Test Node","region":"JP","group_name":"Test","tags":"smoke","note":"private","public_remark":"public","hidden":true,"expires_at":1893456000,"traffic_limit":107374182400,"traffic_limit_type":"max","price":9.9,"billing_cycle":30,"currency":"USD","auto_renewal":true}' \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id" | jq -e '.success == true' >/dev/null
hidden_detail_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/servers/$server_id")
hidden_history_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/history/$server_id?hours=1")
[ "$hidden_detail_status" = "404" ]
[ "$hidden_history_status" = "404" ]

echo "Smoke test passed"
