PRAGMA foreign_keys = ON;

CREATE TABLE settings (
  id INTEGER PRIMARY KEY CHECK(id = 1),
  value TEXT NOT NULL CHECK(json_valid(value)),
  updated_at INTEGER NOT NULL
);

CREATE TABLE servers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  region TEXT NOT NULL DEFAULT '',
  group_name TEXT NOT NULL DEFAULT '默认',
  tags TEXT NOT NULL DEFAULT '',
  hidden INTEGER NOT NULL DEFAULT 0,
  sort_order INTEGER NOT NULL DEFAULT 0,
  expires_at INTEGER,
  traffic_limit INTEGER NOT NULL DEFAULT 0,
  traffic_limit_type TEXT NOT NULL DEFAULT 'sum',
  price REAL NOT NULL DEFAULT 0,
  billing_cycle INTEGER NOT NULL DEFAULT 30,
  currency TEXT NOT NULL DEFAULT 'CNY',
  auto_renewal INTEGER NOT NULL DEFAULT 0,
  last_ip TEXT NOT NULL DEFAULT '',
  network_interface TEXT NOT NULL DEFAULT '',
  reset_day INTEGER NOT NULL DEFAULT 1,
  report_interval INTEGER NOT NULL DEFAULT 60,
  collect_interval INTEGER NOT NULL DEFAULT 1,
  rx_correction INTEGER NOT NULL DEFAULT 0,
  tx_correction INTEGER NOT NULL DEFAULT 0,
  agent_mirror TEXT NOT NULL DEFAULT '',
  offline_notify_disabled INTEGER NOT NULL DEFAULT 0,
  auto_update INTEGER NOT NULL DEFAULT 1,
  token TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE metric_history (
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  timestamp INTEGER NOT NULL,
  cpu REAL NOT NULL DEFAULT 0,
  load1 REAL NOT NULL DEFAULT 0,
  load5 REAL NOT NULL DEFAULT 0,
  load15 REAL NOT NULL DEFAULT 0,
  mem_used INTEGER NOT NULL DEFAULT 0,
  mem_total INTEGER NOT NULL DEFAULT 0,
  swap_used INTEGER NOT NULL DEFAULT 0,
  swap_total INTEGER NOT NULL DEFAULT 0,
  disk_used INTEGER NOT NULL DEFAULT 0,
  disk_total INTEGER NOT NULL DEFAULT 0,
  net_in REAL NOT NULL DEFAULT 0,
  net_out REAL NOT NULL DEFAULT 0,
  net_rx_total INTEGER NOT NULL DEFAULT 0,
  net_tx_total INTEGER NOT NULL DEFAULT 0,
  processes INTEGER NOT NULL DEFAULT 0,
  tcp_connections INTEGER NOT NULL DEFAULT 0,
  udp_connections INTEGER NOT NULL DEFAULT 0,
  gpu_usage REAL NOT NULL DEFAULT 0,
  disk_read_bps REAL NOT NULL DEFAULT 0,
  disk_write_bps REAL NOT NULL DEFAULT 0,
  disk_read_iops REAL NOT NULL DEFAULT 0,
  disk_write_iops REAL NOT NULL DEFAULT 0,
  disk_await_ms REAL NOT NULL DEFAULT 0,
  disk_utilization REAL NOT NULL DEFAULT 0,
  latest_timestamp INTEGER NOT NULL,
  latest_json TEXT NOT NULL CHECK(json_valid(latest_json)),
  latency_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(latency_json)),
  PRIMARY KEY(server_id, timestamp)
) WITHOUT ROWID;

CREATE TABLE metric_history_hourly (
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  timestamp INTEGER NOT NULL,
  cpu REAL NOT NULL DEFAULT 0,
  load1 REAL NOT NULL DEFAULT 0,
  load5 REAL NOT NULL DEFAULT 0,
  load15 REAL NOT NULL DEFAULT 0,
  mem_used INTEGER NOT NULL DEFAULT 0,
  mem_total INTEGER NOT NULL DEFAULT 0,
  swap_used INTEGER NOT NULL DEFAULT 0,
  swap_total INTEGER NOT NULL DEFAULT 0,
  disk_used INTEGER NOT NULL DEFAULT 0,
  disk_total INTEGER NOT NULL DEFAULT 0,
  net_in REAL NOT NULL DEFAULT 0,
  net_out REAL NOT NULL DEFAULT 0,
  net_rx_total INTEGER NOT NULL DEFAULT 0,
  net_tx_total INTEGER NOT NULL DEFAULT 0,
  processes INTEGER NOT NULL DEFAULT 0,
  tcp_connections INTEGER NOT NULL DEFAULT 0,
  udp_connections INTEGER NOT NULL DEFAULT 0,
  gpu_usage REAL NOT NULL DEFAULT 0,
  disk_read_bps REAL NOT NULL DEFAULT 0,
  disk_write_bps REAL NOT NULL DEFAULT 0,
  disk_read_iops REAL NOT NULL DEFAULT 0,
  disk_write_iops REAL NOT NULL DEFAULT 0,
  disk_await_ms REAL NOT NULL DEFAULT 0,
  disk_utilization REAL NOT NULL DEFAULT 0,
  latest_timestamp INTEGER NOT NULL,
  latest_json TEXT NOT NULL CHECK(json_valid(latest_json)),
  latency_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(latency_json)),
  PRIMARY KEY(server_id, timestamp)
) WITHOUT ROWID;

CREATE INDEX idx_servers_sort
  ON servers(sort_order, created_at);

INSERT INTO settings(id, value, updated_at) VALUES (
  1,
  json_patch(
    '{
      "site_description": "轻量、实时的服务器运行状态",
      "site_announcement": "",
      "favicon_url": "",
      "locale": "zh-CN",
      "public_dashboard": "true",
      "history_cache_version": "0",
      "default_theme": "system",
      "active_theme_id": "builtin-nodeflare-glass",
      "background_url": "",
      "theme_options": "{}",
      "show_search": "true",
      "show_groups": "true",
      "show_stats": "true",
      "show_assets": "true",
      "show_traffic": "true",
      "show_speed": "true",
      "show_price": "true",
      "show_expiry": "true",
      "show_latency": "true",
      "show_uptime": "true",
      "admin_username": "",
      "admin_password_hash": "",
      "turnstile_enabled": "false",
      "turnstile_login_enabled": "true",
      "turnstile_site_key": "",
      "turnstile_secret_key": "",
      "notification_enabled": "false",
      "notification_endpoint": "",
      "notification_target": "",
      "offline_alert_minutes": "5",
      "expiry_alert_days": "7",
      "cloudflare_account_id": "",
      "cloudflare_api_token": ""
    }',
    json_object('password_client_salt', lower(hex(randomblob(16))))
  ),
  unixepoch()
);

CREATE TABLE exchange_rate_snapshots (
  base_currency TEXT PRIMARY KEY,
  rates_json TEXT NOT NULL,
  source TEXT NOT NULL,
  rate_date TEXT NOT NULL,
  fetched_at INTEGER NOT NULL,
  attempted_at INTEGER NOT NULL
);

CREATE TABLE themes (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  url TEXT NOT NULL UNIQUE,
  version TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL
);

CREATE INDEX idx_themes_created
  ON themes(created_at DESC);

INSERT INTO exchange_rate_snapshots (
  base_currency, rates_json, source, rate_date, fetched_at, attempted_at
) VALUES (
  'CNY',
  '{"CNY":1,"USD":0.14799,"CAD":0.2086,"HKD":1.1594,"EUR":0.1275,"GBP":0.11027,"JPY":23.707,"RUB":11.560694,"CHF":0.120661,"INR":14.248668,"VND":3875.968992,"THB":4.97107}',
  'default', '', 0, 0
);

CREATE TABLE latency_tasks (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  task_type TEXT NOT NULL CHECK(task_type IN ('tcp', 'icmp')),
  target TEXT NOT NULL,
  port INTEGER CHECK(port IS NULL OR port BETWEEN 1 AND 65535),
  interval_seconds INTEGER NOT NULL CHECK(interval_seconds BETWEEN 30 AND 3600),
  default_enabled INTEGER NOT NULL DEFAULT 0,
  sort_order INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK((task_type = 'tcp' AND port IS NOT NULL) OR (task_type = 'icmp' AND port IS NULL))
);

CREATE TABLE latency_task_servers (
  task_id TEXT NOT NULL REFERENCES latency_tasks(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  assigned_at INTEGER NOT NULL,
  PRIMARY KEY(task_id, server_id)
) WITHOUT ROWID;

CREATE INDEX idx_latency_task_servers_server
  ON latency_task_servers(server_id, task_id);

CREATE TABLE alert_rules (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  metric TEXT NOT NULL CHECK(metric IN ('cpu', 'memory', 'disk', 'net_in', 'net_out')),
  threshold REAL NOT NULL CHECK(threshold > 0),
  duration_minutes INTEGER NOT NULL CHECK(duration_minutes BETWEEN 1 AND 1440),
  aggregation TEXT NOT NULL CHECK(aggregation IN ('average', 'continuous')),
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE alert_rule_servers (
  rule_id TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  PRIMARY KEY(rule_id, server_id)
) WITHOUT ROWID;

CREATE TABLE alert_states (
  rule_id TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  active INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(rule_id, server_id)
) WITHOUT ROWID;

PRAGMA optimize;
