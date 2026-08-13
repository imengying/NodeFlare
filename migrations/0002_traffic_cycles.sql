-- Current schema: persist per-node traffic counters across monthly resets and agent restarts.
CREATE TABLE IF NOT EXISTS traffic_cycles (
  server_id TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
  cycle_key INTEGER NOT NULL,
  reset_day INTEGER NOT NULL DEFAULT 1,
  timestamp INTEGER NOT NULL,
  raw_rx INTEGER NOT NULL DEFAULT 0,
  raw_tx INTEGER NOT NULL DEFAULT 0,
  used_rx INTEGER NOT NULL DEFAULT 0,
  used_tx INTEGER NOT NULL DEFAULT 0
);
