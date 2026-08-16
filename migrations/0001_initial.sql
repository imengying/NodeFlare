PRAGMA foreign_keys = ON;

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
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
  collect_interval INTEGER NOT NULL DEFAULT 5,
  rx_correction INTEGER NOT NULL DEFAULT 0,
  tx_correction INTEGER NOT NULL DEFAULT 0,
  agent_mirror TEXT NOT NULL DEFAULT '',
  offline_notify_disabled INTEGER NOT NULL DEFAULT 0,
  auto_update INTEGER NOT NULL DEFAULT 1,
  token TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE latest_metrics (
  server_id TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
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
  uptime INTEGER NOT NULL DEFAULT 0,
  processes INTEGER NOT NULL DEFAULT 0,
  tcp_connections INTEGER NOT NULL DEFAULT 0,
  udp_connections INTEGER NOT NULL DEFAULT 0,
  cpu_cores INTEGER NOT NULL DEFAULT 0,
  cpu_model TEXT NOT NULL DEFAULT '',
  os TEXT NOT NULL DEFAULT '',
  kernel TEXT NOT NULL DEFAULT '',
  arch TEXT NOT NULL DEFAULT '',
  virtualization TEXT NOT NULL DEFAULT '',
  gpu_usage REAL NOT NULL DEFAULT 0,
  gpu_model TEXT NOT NULL DEFAULT '',
  agent_version TEXT NOT NULL DEFAULT '',
  disk_read_bps REAL NOT NULL DEFAULT 0,
  disk_write_bps REAL NOT NULL DEFAULT 0,
  disk_read_iops REAL NOT NULL DEFAULT 0,
  disk_write_iops REAL NOT NULL DEFAULT 0,
  disk_await_ms REAL NOT NULL DEFAULT 0,
  disk_utilization REAL NOT NULL DEFAULT 0,
  disk_info TEXT NOT NULL DEFAULT '[]',
  gpu_info TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE traffic_cycles (
  server_id TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
  cycle_key INTEGER NOT NULL,
  reset_day INTEGER NOT NULL DEFAULT 1,
  timestamp INTEGER NOT NULL,
  raw_rx INTEGER NOT NULL DEFAULT 0,
  raw_tx INTEGER NOT NULL DEFAULT 0,
  used_rx INTEGER NOT NULL DEFAULT 0,
  used_tx INTEGER NOT NULL DEFAULT 0
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
  PRIMARY KEY(server_id, timestamp)
);

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
  PRIMARY KEY(server_id, timestamp)
);

CREATE INDEX idx_history_server_time
  ON metric_history(server_id, timestamp DESC);
CREATE INDEX idx_history_hourly_server_time
  ON metric_history_hourly(server_id, timestamp DESC);
CREATE INDEX idx_servers_sort
  ON servers(sort_order, created_at);

INSERT INTO settings(key, value, updated_at) VALUES
  ('site_description', '轻量、实时的服务器运行状态', unixepoch()),
  ('site_announcement', '', unixepoch()),
  ('favicon_url', '', unixepoch()),
  ('locale', 'zh-CN', unixepoch()),
  ('public_dashboard', 'true', unixepoch()),
  ('history_cache_version', '0', unixepoch()),
  ('default_theme', 'system', unixepoch()),
  ('active_theme_id', 'builtin-nodeflare-glass', unixepoch()),
  ('background_url', '', unixepoch()),
  ('theme_options', '{}', unixepoch()),
  ('show_search', 'true', unixepoch()),
  ('show_groups', 'true', unixepoch()),
  ('show_stats', 'true', unixepoch()),
  ('show_assets', 'true', unixepoch()),
  ('show_traffic', 'true', unixepoch()),
  ('show_speed', 'true', unixepoch()),
  ('show_price', 'true', unixepoch()),
  ('show_expiry', 'true', unixepoch()),
  ('show_latency', 'true', unixepoch()),
  ('show_uptime', 'true', unixepoch()),
  ('admin_username', '', unixepoch()),
  ('admin_password_hash', '', unixepoch()),
  ('password_client_salt', lower(hex(randomblob(16))), unixepoch()),
  ('turnstile_enabled', 'false', unixepoch()),
  ('turnstile_login_enabled', 'true', unixepoch()),
  ('turnstile_site_key', '', unixepoch()),
  ('turnstile_secret_key', '', unixepoch()),
  ('notification_enabled', 'false', unixepoch()),
  ('notification_endpoint', '', unixepoch()),
  ('notification_target', '', unixepoch()),
  ('offline_alert_minutes', '5', unixepoch()),
  ('expiry_alert_days', '7', unixepoch()),
  ('cloudflare_account_id', '', unixepoch()),
  ('cloudflare_api_token', '', unixepoch());

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
  PRIMARY KEY(task_id, server_id)
);

CREATE TABLE latency_latest (
  task_id TEXT NOT NULL REFERENCES latency_tasks(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  timestamp INTEGER NOT NULL,
  latency_ms REAL NOT NULL,
  packet_loss REAL NOT NULL,
  PRIMARY KEY(task_id, server_id)
);

CREATE TABLE latency_history (
  task_id TEXT NOT NULL REFERENCES latency_tasks(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  timestamp INTEGER NOT NULL,
  latency_ms REAL NOT NULL,
  packet_loss REAL NOT NULL,
  PRIMARY KEY(task_id, server_id, timestamp)
);

CREATE INDEX idx_latency_task_servers_server
  ON latency_task_servers(server_id, task_id);
CREATE INDEX idx_latency_history_server_time
  ON latency_history(server_id, timestamp DESC);
CREATE INDEX idx_latency_history_task_time
  ON latency_history(task_id, timestamp DESC);

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
);

CREATE TABLE alert_states (
  rule_id TEXT NOT NULL REFERENCES alert_rules(id) ON DELETE CASCADE,
  server_id TEXT NOT NULL REFERENCES servers(id) ON DELETE CASCADE,
  active INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(rule_id, server_id)
);
