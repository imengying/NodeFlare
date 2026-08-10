use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use worker::{wasm_bindgen::JsValue, D1Database, Date, Result};

use crate::models::{
    AgentConfigView, AgentReport, AlertRuleInput, AlertRuleView, HistoryPoint, ServerInput,
    ServerView, SettingsInput, TokenHashRow,
};

const SERVER_SELECT: &str = r#"
SELECT
  s.id, s.name, s.region, s.group_name, s.tags, s.note, s.hidden,
  s.sort_order, s.expires_at, s.traffic_limit, s.traffic_limit_type,
  s.price, s.billing_cycle, s.currency, s.auto_renewal, s.public_remark,
  s.network_interface, s.reset_day, s.report_interval, s.collect_interval,
  s.rx_correction, s.tx_correction, s.offline_notify_disabled, s.auto_update,
  s.created_at, s.updated_at,
  m.timestamp, m.cpu, m.load1, m.load5, m.load15, m.mem_used, m.mem_total,
  m.swap_used, m.swap_total, m.disk_used, m.disk_total, m.net_in, m.net_out,
  CASE WHEN m.net_rx_total IS NULL THEN NULL ELSE m.net_rx_total + s.rx_correction END AS net_rx_total,
  CASE WHEN m.net_tx_total IS NULL THEN NULL ELSE m.net_tx_total + s.tx_correction END AS net_tx_total,
  m.uptime, m.processes, m.tcp_connections,
  m.udp_connections, m.cpu_cores, m.cpu_model, m.os, m.kernel, m.arch,
  m.virtualization, m.ipv4, m.ipv6, m.gpu_usage, m.gpu_model, m.agent_version,
  m.disk_read_bps, m.disk_write_bps, m.disk_read_iops, m.disk_write_iops,
  m.disk_await_ms, m.disk_utilization, m.disk_info, m.gpu_info,
  m.message
FROM servers s
LEFT JOIN latest_metrics m ON m.server_id = s.id
"#;

#[derive(Debug, serde::Deserialize)]
struct SettingRow {
    key: String,
    value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingsView {
    pub site_name: String,
    pub site_description: String,
    pub site_announcement: String,
    pub favicon_url: String,
    pub locale: String,
    pub public_dashboard: bool,
    pub offline_threshold_seconds: i64,
    pub history_retention_days: i64,
    pub default_theme: String,
    pub background_url: String,
    pub theme_url: String,
    pub theme_options: serde_json::Value,
    pub show_search: bool,
    pub show_groups: bool,
    pub show_stats: bool,
    pub show_assets: bool,
    pub show_traffic: bool,
    pub show_speed: bool,
    pub show_price: bool,
    pub show_expiry: bool,
    pub show_latency: bool,
    pub show_uptime: bool,
    pub admin_username: String,
    pub admin_password_configured: bool,
    #[serde(skip_serializing)]
    pub admin_password_hash: String,
    pub turnstile_enabled: bool,
    pub turnstile_login_enabled: bool,
    pub turnstile_site_key: String,
    pub turnstile_secret_key: String,
    pub notification_enabled: bool,
    pub notification_endpoint: String,
    pub notification_target: String,
    pub offline_alert_minutes: i64,
    pub expiry_alert_days: i64,
    pub cloudflare_account_id: String,
    pub cloudflare_api_token: String,
    pub cors_allowed_origins: String,
    pub csp_asset_origins: String,
    pub federation_sites: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DatabaseStats {
    pub server_count: i64,
    pub online_count: i64,
    pub history_rows: i64,
    pub oldest_history: Option<i64>,
    pub newest_history: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExchangeRateSnapshot {
    pub base_currency: String,
    pub rates_json: String,
    pub source: String,
    pub rate_date: String,
    pub fetched_at: i64,
    pub attempted_at: i64,
}

#[derive(Debug, Deserialize)]
struct AlertRuleRow {
    id: String,
    name: String,
    metric: String,
    threshold: f64,
    duration_minutes: i64,
    aggregation: String,
    enabled: i64,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Deserialize)]
struct AlertServerRow {
    server_id: String,
}

#[derive(Debug, Deserialize)]
pub struct AlertMetricRow {
    pub server_id: String,
    pub name: String,
    pub value: f64,
}

#[derive(Debug, Deserialize)]
struct AlertStateRow {
    rule_id: String,
    server_id: String,
}

fn now() -> i64 {
    Date::now().as_millis() as i64 / 1000
}

fn text(value: &str) -> JsValue {
    JsValue::from_str(value)
}

fn number(value: impl ToString) -> JsValue {
    JsValue::from_f64(value.to_string().parse::<f64>().unwrap_or(0.0))
}

pub async fn list_servers(db: &D1Database, include_hidden: bool) -> Result<Vec<ServerView>> {
    let filter = if include_hidden {
        ""
    } else {
        "WHERE s.hidden = 0"
    };
    let query = format!("{SERVER_SELECT} {filter} ORDER BY s.sort_order ASC, s.created_at ASC");
    db.prepare(query).all().await?.results()
}

pub async fn get_server(
    db: &D1Database,
    id: &str,
    include_hidden: bool,
) -> Result<Option<ServerView>> {
    let hidden = if include_hidden {
        ""
    } else {
        "AND s.hidden = 0"
    };
    let query = format!("{SERVER_SELECT} WHERE s.id = ?1 {hidden} LIMIT 1");
    db.prepare(query).bind(&[text(id)])?.first(None).await
}

pub async fn get_token_hash(db: &D1Database, id: &str) -> Result<Option<TokenHashRow>> {
    db.prepare("SELECT token_hash, hidden FROM servers WHERE id = ?1")
        .bind(&[text(id)])?
        .first(None)
        .await
}

pub async fn agent_config(
    db: &D1Database,
    id: &str,
    latest_agent_version: &str,
    _settings: &SettingsView,
) -> Result<Option<AgentConfigView>> {
    let mut config: Option<AgentConfigView> = db
        .prepare(
            "SELECT report_interval, collect_interval, network_interface, auto_update, \
             ?2 AS latest_agent_version FROM servers WHERE id = ?1",
        )
        .bind(&[text(id), text(latest_agent_version)])?
        .first(None)
        .await?;
    if let Some(config) = config.as_mut() {
        config.network_interface = config.network_interface.trim().to_string();
        config.latency_tasks = crate::latency::tasks_for_server(db, id).await?;
    }
    Ok(config)
}

pub async fn create_server(
    db: &D1Database,
    id: &str,
    token_hash: &str,
    input: &ServerInput,
) -> Result<()> {
    let timestamp = now();
    db.prepare(
        r#"INSERT INTO servers (
          id, name, region, group_name, tags, note, hidden, sort_order,
          expires_at, traffic_limit, traffic_limit_type, price, billing_cycle,
          currency, auto_renewal, public_remark, network_interface, reset_day,
          report_interval, collect_interval, rx_correction, tx_correction, offline_notify_disabled, auto_update,
          token_hash, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7,
          COALESCE((SELECT MAX(sort_order) + 1 FROM servers), 0),
          ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
          ?20, ?21, ?22, ?23, ?24, ?25, ?25)"#,
    )
    .bind(&[
        text(id),
        text(input.name.trim()),
        text(input.region.trim()),
        text(input.group_name.trim()),
        text(input.tags.trim()),
        text(input.note.trim()),
        JsValue::from_bool(input.hidden),
        input.expires_at.map(number).unwrap_or(JsValue::NULL),
        number(input.traffic_limit),
        text(input.traffic_limit_type.trim()),
        number(input.price),
        number(input.billing_cycle),
        text(input.currency.trim()),
        JsValue::from_bool(input.auto_renewal),
        text(input.public_remark.trim()),
        text(input.network_interface.trim()),
        number(input.reset_day),
        number(input.report_interval),
        number(input.collect_interval),
        number(input.rx_correction),
        number(input.tx_correction),
        JsValue::from_bool(input.offline_notify_disabled),
        JsValue::from_bool(input.auto_update),
        text(token_hash),
        number(timestamp),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn update_server(db: &D1Database, id: &str, input: &ServerInput) -> Result<bool> {
    let result = db
        .prepare(
            r#"UPDATE servers SET
              name = ?2, region = ?3, group_name = ?4, tags = ?5, note = ?6,
              hidden = ?7, expires_at = ?8, traffic_limit = ?9,
              traffic_limit_type = ?10, price = ?11, billing_cycle = ?12,
              currency = ?13, auto_renewal = ?14, public_remark = ?15,
              network_interface = ?16, reset_day = ?17, report_interval = ?18,
              collect_interval = ?19, rx_correction = ?20, tx_correction = ?21,
              offline_notify_disabled = ?22, auto_update = ?23, updated_at = ?24
            WHERE id = ?1"#,
        )
        .bind(&[
            text(id),
            text(input.name.trim()),
            text(input.region.trim()),
            text(input.group_name.trim()),
            text(input.tags.trim()),
            text(input.note.trim()),
            JsValue::from_bool(input.hidden),
            input.expires_at.map(number).unwrap_or(JsValue::NULL),
            number(input.traffic_limit),
            text(input.traffic_limit_type.trim()),
            number(input.price),
            number(input.billing_cycle),
            text(input.currency.trim()),
            JsValue::from_bool(input.auto_renewal),
            text(input.public_remark.trim()),
            text(input.network_interface.trim()),
            number(input.reset_day),
            number(input.report_interval),
            number(input.collect_interval),
            number(input.rx_correction),
            number(input.tx_correction),
            JsValue::from_bool(input.offline_notify_disabled),
            JsValue::from_bool(input.auto_update),
            number(now()),
        ])?
        .run()
        .await?;
    Ok(result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0)
}

pub async fn delete_server(db: &D1Database, id: &str) -> Result<bool> {
    let result = db
        .prepare("DELETE FROM servers WHERE id = ?1")
        .bind(&[text(id)])?
        .run()
        .await?;
    Ok(result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0)
}

pub async fn delete_servers(db: &D1Database, ids: &[String]) -> Result<()> {
    let base = db.prepare("DELETE FROM servers WHERE id = ?1");
    let mut statements = Vec::with_capacity(ids.len());
    for id in ids {
        statements.push(base.clone().bind(&[text(id)])?);
    }
    if !statements.is_empty() {
        db.batch(statements).await?;
    }
    Ok(())
}

pub async fn update_token(db: &D1Database, id: &str, token_hash: &str) -> Result<bool> {
    let result = db
        .prepare("UPDATE servers SET token_hash = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(&[text(id), text(token_hash), number(now())])?
        .run()
        .await?;
    Ok(result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0)
}

pub async fn reorder_servers(db: &D1Database, ids: &[String]) -> Result<()> {
    let base = db.prepare("UPDATE servers SET sort_order = ?2, updated_at = ?3 WHERE id = ?1");
    let timestamp = now();
    let mut statements = Vec::with_capacity(ids.len());
    for (index, id) in ids.iter().enumerate() {
        statements.push(
            base.clone()
                .bind(&[text(id), number(index), number(timestamp)])?,
        );
    }
    if !statements.is_empty() {
        db.batch(statements).await?;
    }
    Ok(())
}

pub async fn list_alert_rules(db: &D1Database) -> Result<Vec<AlertRuleView>> {
    let rows: Vec<AlertRuleRow> = db
        .prepare(
            "SELECT id, name, metric, threshold, duration_minutes, aggregation, enabled, \
             created_at, updated_at FROM alert_rules ORDER BY created_at ASC",
        )
        .all()
        .await?
        .results()?;
    let mut rules = Vec::with_capacity(rows.len());
    for row in rows {
        let servers: Vec<AlertServerRow> = db
            .prepare(
                "SELECT server_id FROM alert_rule_servers WHERE rule_id = ?1 \
                 ORDER BY server_id ASC",
            )
            .bind(&[text(&row.id)])?
            .all()
            .await?
            .results()?;
        rules.push(AlertRuleView {
            id: row.id,
            name: row.name,
            metric: row.metric,
            threshold: row.threshold,
            duration_minutes: row.duration_minutes,
            aggregation: row.aggregation,
            enabled: row.enabled,
            server_ids: servers.into_iter().map(|row| row.server_id).collect(),
            created_at: row.created_at,
            updated_at: row.updated_at,
        });
    }
    Ok(rules)
}

async fn replace_alert_rule_servers(
    db: &D1Database,
    rule_id: &str,
    server_ids: &[String],
) -> Result<()> {
    let mut statements = vec![db
        .prepare("DELETE FROM alert_rule_servers WHERE rule_id = ?1")
        .bind(&[text(rule_id)])?];
    let insert = db.prepare("INSERT INTO alert_rule_servers(rule_id, server_id) VALUES (?1, ?2)");
    for server_id in server_ids {
        statements.push(insert.clone().bind(&[text(rule_id), text(server_id)])?);
    }
    db.batch(statements).await?;
    Ok(())
}

pub async fn create_alert_rule(db: &D1Database, id: &str, input: &AlertRuleInput) -> Result<()> {
    let timestamp = now();
    db.prepare(
        "INSERT INTO alert_rules(id, name, metric, threshold, duration_minutes, \
         aggregation, enabled, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
    )
    .bind(&[
        text(id),
        text(input.name.trim()),
        text(input.metric.trim()),
        number(input.threshold),
        number(input.duration_minutes),
        text(input.aggregation.trim()),
        JsValue::from_bool(input.enabled),
        number(timestamp),
    ])?
    .run()
    .await?;
    replace_alert_rule_servers(db, id, &input.server_ids).await
}

pub async fn update_alert_rule(db: &D1Database, id: &str, input: &AlertRuleInput) -> Result<bool> {
    let result = db
        .prepare(
            "UPDATE alert_rules SET name=?2, metric=?3, threshold=?4, duration_minutes=?5, \
         aggregation=?6, enabled=?7, updated_at=?8 WHERE id=?1",
        )
        .bind(&[
            text(id),
            text(input.name.trim()),
            text(input.metric.trim()),
            number(input.threshold),
            number(input.duration_minutes),
            text(input.aggregation.trim()),
            JsValue::from_bool(input.enabled),
            number(now()),
        ])?
        .run()
        .await?;
    let changed = result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0;
    if changed {
        replace_alert_rule_servers(db, id, &input.server_ids).await?;
    }
    Ok(changed)
}

pub async fn delete_alert_rule(db: &D1Database, id: &str) -> Result<bool> {
    let result = db
        .prepare("DELETE FROM alert_rules WHERE id=?1")
        .bind(&[text(id)])?
        .run()
        .await?;
    Ok(result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0)
}

pub async fn alert_metric_values(
    db: &D1Database,
    rule: &AlertRuleView,
    current_time: i64,
) -> Result<Vec<AlertMetricRow>> {
    let metric = match rule.metric.as_str() {
        "cpu" => "h.cpu",
        "memory" => "CASE WHEN h.mem_total > 0 THEN h.mem_used * 100.0 / h.mem_total ELSE 0 END",
        "disk" => "CASE WHEN h.disk_total > 0 THEN h.disk_used * 100.0 / h.disk_total ELSE 0 END",
        "net_in" => "h.net_in / 1048576.0",
        "net_out" => "h.net_out / 1048576.0",
        _ => return Ok(Vec::new()),
    };
    let aggregate = if rule.aggregation == "continuous" {
        "MIN"
    } else {
        "AVG"
    };
    let since = current_time - rule.duration_minutes.clamp(1, 1440) * 60;
    let minimum_samples = rule.duration_minutes.clamp(1, 1440);
    let query = format!(
        "SELECT s.id AS server_id, s.name, {aggregate}({metric}) AS value \
         FROM servers s JOIN metric_history h ON h.server_id=s.id \
         WHERE h.timestamp >= ?1 AND s.hidden=0 AND (\
           NOT EXISTS(SELECT 1 FROM alert_rule_servers ars WHERE ars.rule_id=?2) OR \
           EXISTS(SELECT 1 FROM alert_rule_servers ars WHERE ars.rule_id=?2 AND ars.server_id=s.id)\
         ) GROUP BY s.id, s.name HAVING COUNT(*) >= ?3"
    );
    db.prepare(query)
        .bind(&[number(since), text(&rule.id), number(minimum_samples)])?
        .all()
        .await?
        .results()
}

pub async fn active_alert_states(db: &D1Database) -> Result<std::collections::HashSet<String>> {
    let rows: Vec<AlertStateRow> = db
        .prepare("SELECT rule_id, server_id FROM alert_states WHERE active=1")
        .all()
        .await?
        .results()?;
    Ok(rows
        .into_iter()
        .map(|row| format!("{}:{}", row.rule_id, row.server_id))
        .collect())
}

pub async fn replace_active_alert_states(
    db: &D1Database,
    keys: &std::collections::HashSet<String>,
) -> Result<()> {
    let mut statements = vec![db.prepare("DELETE FROM alert_states")];
    let insert = db.prepare(
        "INSERT INTO alert_states(rule_id, server_id, active, updated_at) VALUES (?1, ?2, 1, ?3)",
    );
    let timestamp = now();
    for key in keys {
        let Some((rule_id, server_id)) = key.split_once(':') else {
            continue;
        };
        statements.push(insert.clone().bind(&[
            text(rule_id),
            text(server_id),
            number(timestamp),
        ])?);
    }
    db.batch(statements).await?;
    Ok(())
}

pub async fn history(db: &D1Database, id: &str, hours: i64) -> Result<Vec<HistoryPoint>> {
    let hours = hours.clamp(1, 24 * 30);
    let since = now() - hours * 3600;
    let bucket = match hours {
        1 => 60,
        2..=4 => 120,
        5..=24 => 600,
        25..=168 => 3600,
        _ => 14_400,
    };
    let source = if hours <= 24 * 7 {
        "SELECT * FROM metric_history".to_string()
    } else {
        "SELECT * FROM metric_history UNION ALL SELECT * FROM metric_history_hourly".to_string()
    };
    let query = format!(
        r#"SELECT
          (timestamp / {bucket}) * {bucket} AS timestamp,
          AVG(cpu) AS cpu, AVG(load1) AS load1, AVG(load5) AS load5,
          AVG(load15) AS load15,
          CAST(AVG(mem_used) AS INTEGER) AS mem_used,
          CAST(MAX(mem_total) AS INTEGER) AS mem_total,
          CAST(AVG(swap_used) AS INTEGER) AS swap_used,
          CAST(MAX(swap_total) AS INTEGER) AS swap_total,
          CAST(AVG(disk_used) AS INTEGER) AS disk_used,
          CAST(MAX(disk_total) AS INTEGER) AS disk_total,
          AVG(net_in) AS net_in, AVG(net_out) AS net_out,
          CAST(MAX(net_rx_total) AS INTEGER) AS net_rx_total,
          CAST(MAX(net_tx_total) AS INTEGER) AS net_tx_total,
          CAST(AVG(processes) AS INTEGER) AS processes,
          CAST(AVG(tcp_connections) AS INTEGER) AS tcp_connections,
          CAST(AVG(udp_connections) AS INTEGER) AS udp_connections,
          AVG(gpu_usage) AS gpu_usage,
          AVG(disk_read_bps) AS disk_read_bps,
          AVG(disk_write_bps) AS disk_write_bps,
          AVG(disk_read_iops) AS disk_read_iops,
          AVG(disk_write_iops) AS disk_write_iops,
          AVG(disk_await_ms) AS disk_await_ms,
          AVG(disk_utilization) AS disk_utilization
        FROM ({source})
        WHERE server_id = ?1 AND timestamp >= ?2
        GROUP BY timestamp / {bucket}
        ORDER BY timestamp ASC
        LIMIT 1000"#,
    );
    db.prepare(query)
        .bind(&[text(id), number(since)])?
        .all()
        .await?
        .results()
}

pub async fn save_reports(db: &D1Database, server_id: &str, reports: &[AgentReport]) -> Result<()> {
    let Some(latest_report) = reports.iter().max_by_key(|report| report.timestamp) else {
        return Ok(());
    };
    let latest_timestamp = latest_report.timestamp;
    let latest = db
        .prepare(
            r#"INSERT INTO latest_metrics (
              server_id, timestamp, cpu, load1, load5, load15, mem_used, mem_total,
              swap_used, swap_total, disk_used, disk_total, net_in, net_out,
              net_rx_total, net_tx_total, uptime, processes, tcp_connections,
              udp_connections, cpu_cores, cpu_model, os, kernel, arch,
              virtualization, ipv4, ipv6, gpu_usage, gpu_model, agent_version,
              disk_read_bps, disk_write_bps, disk_read_iops, disk_write_iops,
              disk_await_ms, disk_utilization, disk_info, gpu_info, message
            ) VALUES (
              ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
              ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26,
              ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38,
              ?39, ?40
            ) ON CONFLICT(server_id) DO UPDATE SET
              timestamp=excluded.timestamp, cpu=excluded.cpu, load1=excluded.load1,
              load5=excluded.load5, load15=excluded.load15, mem_used=excluded.mem_used,
              mem_total=excluded.mem_total, swap_used=excluded.swap_used,
              swap_total=excluded.swap_total, disk_used=excluded.disk_used,
              disk_total=excluded.disk_total, net_in=excluded.net_in, net_out=excluded.net_out,
              net_rx_total=excluded.net_rx_total, net_tx_total=excluded.net_tx_total,
              uptime=excluded.uptime, processes=excluded.processes,
              tcp_connections=excluded.tcp_connections, udp_connections=excluded.udp_connections,
              cpu_cores=excluded.cpu_cores, cpu_model=excluded.cpu_model, os=excluded.os,
              kernel=excluded.kernel, arch=excluded.arch,
              virtualization=excluded.virtualization, ipv4=excluded.ipv4, ipv6=excluded.ipv6,
              gpu_usage=excluded.gpu_usage, gpu_model=excluded.gpu_model,
              agent_version=excluded.agent_version,
              disk_read_bps=excluded.disk_read_bps, disk_write_bps=excluded.disk_write_bps,
              disk_read_iops=excluded.disk_read_iops, disk_write_iops=excluded.disk_write_iops,
              disk_await_ms=excluded.disk_await_ms,
              disk_utilization=excluded.disk_utilization,
              disk_info=excluded.disk_info, gpu_info=excluded.gpu_info,
              message=excluded.message
            WHERE excluded.timestamp >= latest_metrics.timestamp"#,
        )
        .bind(&report_values(server_id, latest_report, latest_timestamp))?;

    let history_statement = db.prepare(
        r#"INSERT INTO metric_history (
              server_id, timestamp, cpu, load1, load5, load15,
              mem_used, mem_total, swap_used, swap_total, disk_used, disk_total,
              net_in, net_out, net_rx_total, net_tx_total, processes,
              tcp_connections, udp_connections, gpu_usage,
              disk_read_bps, disk_write_bps, disk_read_iops, disk_write_iops,
              disk_await_ms, disk_utilization
            ) VALUES (
              ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
              ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26
            ) ON CONFLICT(server_id, timestamp) DO UPDATE SET
              cpu=excluded.cpu, load1=excluded.load1, load5=excluded.load5,
              load15=excluded.load15, mem_used=excluded.mem_used,
              mem_total=excluded.mem_total, swap_used=excluded.swap_used,
              swap_total=excluded.swap_total, disk_used=excluded.disk_used,
              disk_total=excluded.disk_total, net_in=excluded.net_in,
              net_out=excluded.net_out, net_rx_total=excluded.net_rx_total,
              net_tx_total=excluded.net_tx_total, processes=excluded.processes,
              tcp_connections=excluded.tcp_connections,
              udp_connections=excluded.udp_connections, gpu_usage=excluded.gpu_usage,
              disk_read_bps=excluded.disk_read_bps,
              disk_write_bps=excluded.disk_write_bps,
              disk_read_iops=excluded.disk_read_iops,
              disk_write_iops=excluded.disk_write_iops,
              disk_await_ms=excluded.disk_await_ms,
              disk_utilization=excluded.disk_utilization"#,
    );

    // D1 keeps one representative sample per minute. The Agent can sample every few
    // seconds without multiplying long-term rows or write cost.
    let mut minute_samples = BTreeMap::new();
    for report in reports {
        minute_samples.insert(report.timestamp / 60 * 60, report);
    }
    let mut statements = Vec::with_capacity(minute_samples.len() + 1);
    statements.push(latest);
    for (timestamp, report) in minute_samples {
        statements.push(
            history_statement
                .clone()
                .bind(&history_values(server_id, report, timestamp))?,
        );
    }
    db.batch(statements).await?;
    Ok(())
}

fn report_values(server_id: &str, report: &AgentReport, timestamp: i64) -> Vec<JsValue> {
    vec![
        text(server_id),
        number(timestamp),
        number(report.cpu),
        number(report.load1),
        number(report.load5),
        number(report.load15),
        number(report.mem_used),
        number(report.mem_total),
        number(report.swap_used),
        number(report.swap_total),
        number(report.disk_used),
        number(report.disk_total),
        number(report.net_in),
        number(report.net_out),
        number(report.net_rx_total),
        number(report.net_tx_total),
        number(report.uptime),
        number(report.processes),
        number(report.tcp_connections),
        number(report.udp_connections),
        number(report.cpu_cores),
        text(&report.cpu_model),
        text(&report.os),
        text(&report.kernel),
        text(&report.arch),
        text(&report.virtualization),
        text(&report.ipv4),
        text(&report.ipv6),
        number(report.gpu_usage),
        text(&report.gpu_model),
        text(&report.agent_version),
        number(report.disk_read_bps),
        number(report.disk_write_bps),
        number(report.disk_read_iops),
        number(report.disk_write_iops),
        number(report.disk_await_ms),
        number(report.disk_utilization),
        text(&serde_json::to_string(&report.disks).unwrap_or_else(|_| "[]".to_string())),
        text(&serde_json::to_string(&report.gpus).unwrap_or_else(|_| "[]".to_string())),
        text(&report.message),
    ]
}

fn history_values(server_id: &str, report: &AgentReport, timestamp: i64) -> Vec<JsValue> {
    vec![
        text(server_id),
        number(timestamp),
        number(report.cpu),
        number(report.load1),
        number(report.load5),
        number(report.load15),
        number(report.mem_used),
        number(report.mem_total),
        number(report.swap_used),
        number(report.swap_total),
        number(report.disk_used),
        number(report.disk_total),
        number(report.net_in),
        number(report.net_out),
        number(report.net_rx_total),
        number(report.net_tx_total),
        number(report.processes),
        number(report.tcp_connections),
        number(report.udp_connections),
        number(report.gpu_usage),
        number(report.disk_read_bps),
        number(report.disk_write_bps),
        number(report.disk_read_iops),
        number(report.disk_write_iops),
        number(report.disk_await_ms),
        number(report.disk_utilization),
    ]
}

fn bool_setting(values: &HashMap<String, String>, key: &str, fallback: bool) -> bool {
    values
        .get(key)
        .map(|value| value == "true")
        .unwrap_or(fallback)
}

fn string_setting(values: &HashMap<String, String>, key: &str, fallback: &str) -> String {
    values
        .get(key)
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn integer_setting(values: &HashMap<String, String>, key: &str, fallback: i64) -> i64 {
    values
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

pub async fn settings(
    db: &D1Database,
    default_name: &str,
    default_threshold: i64,
    default_username: &str,
) -> Result<SettingsView> {
    let rows: Vec<SettingRow> = db
        .prepare("SELECT key, value FROM settings")
        .all()
        .await?
        .results()?;
    let values: HashMap<String, String> =
        rows.into_iter().map(|row| (row.key, row.value)).collect();
    Ok(SettingsView {
        site_name: values
            .get("site_name")
            .cloned()
            .unwrap_or_else(|| default_name.to_string()),
        site_description: string_setting(&values, "site_description", "轻量、实时的服务器运行状态"),
        site_announcement: string_setting(&values, "site_announcement", ""),
        favicon_url: string_setting(&values, "favicon_url", ""),
        locale: string_setting(&values, "locale", "zh-CN"),
        public_dashboard: bool_setting(&values, "public_dashboard", true),
        offline_threshold_seconds: values
            .get("offline_threshold_seconds")
            .and_then(|value| value.parse().ok())
            .unwrap_or(default_threshold),
        history_retention_days: integer_setting(&values, "history_retention_days", 30),
        default_theme: string_setting(&values, "default_theme", "system"),
        background_url: string_setting(&values, "background_url", ""),
        theme_url: string_setting(&values, "theme_url", ""),
        theme_options: values
            .get("theme_options")
            .and_then(|value| serde_json::from_str(value).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({})),
        show_search: bool_setting(&values, "show_search", true),
        show_groups: bool_setting(&values, "show_groups", true),
        show_stats: bool_setting(&values, "show_stats", true),
        show_assets: bool_setting(&values, "show_assets", true),
        show_traffic: bool_setting(&values, "show_traffic", true),
        show_speed: bool_setting(&values, "show_speed", true),
        show_price: bool_setting(&values, "show_price", true),
        show_expiry: bool_setting(&values, "show_expiry", true),
        show_latency: bool_setting(&values, "show_latency", true),
        show_uptime: bool_setting(&values, "show_uptime", true),
        admin_username: string_setting(&values, "admin_username", default_username),
        admin_password_configured: values
            .get("admin_password_hash")
            .is_some_and(|value| !value.is_empty()),
        admin_password_hash: string_setting(&values, "admin_password_hash", ""),
        turnstile_enabled: bool_setting(&values, "turnstile_enabled", false),
        turnstile_login_enabled: bool_setting(&values, "turnstile_login_enabled", false),
        turnstile_site_key: string_setting(&values, "turnstile_site_key", ""),
        turnstile_secret_key: string_setting(&values, "turnstile_secret_key", ""),
        notification_enabled: bool_setting(&values, "notification_enabled", false),
        notification_endpoint: string_setting(&values, "notification_endpoint", ""),
        notification_target: string_setting(&values, "notification_target", ""),
        offline_alert_minutes: integer_setting(&values, "offline_alert_minutes", 5),
        expiry_alert_days: integer_setting(&values, "expiry_alert_days", 7),
        cloudflare_account_id: string_setting(&values, "cloudflare_account_id", ""),
        cloudflare_api_token: string_setting(&values, "cloudflare_api_token", ""),
        cors_allowed_origins: string_setting(&values, "cors_allowed_origins", ""),
        csp_asset_origins: string_setting(&values, "csp_asset_origins", ""),
        federation_sites: values
            .get("federation_sites")
            .and_then(|value| serde_json::from_str(value).ok())
            .filter(serde_json::Value::is_array)
            .unwrap_or_else(|| serde_json::json!([])),
    })
}

pub async fn update_settings(
    db: &D1Database,
    input: &SettingsInput,
    password_hash: Option<&str>,
) -> Result<()> {
    let mut statements = Vec::new();
    let timestamp = now();
    let base = db.prepare(
        "INSERT INTO settings(key, value, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
    );
    macro_rules! push_setting {
        ($key:expr, $value:expr) => {
            statements.push(
                base.clone()
                    .bind(&[text($key), text($value), number(timestamp)])?,
            );
        };
    }

    if let Some(value) = input.site_name.as_deref() {
        push_setting!("site_name", value.trim());
    }
    if let Some(value) = input.site_description.as_deref() {
        push_setting!("site_description", value.trim());
    }
    if let Some(value) = input.site_announcement.as_deref() {
        push_setting!("site_announcement", value.trim());
    }
    if let Some(value) = input.favicon_url.as_deref() {
        push_setting!("favicon_url", value.trim());
    }
    if let Some(value) = input.locale.as_deref() {
        push_setting!("locale", value.trim());
    }
    if let Some(value) = input.public_dashboard {
        push_setting!("public_dashboard", if value { "true" } else { "false" });
    }
    if let Some(value) = input.offline_threshold_seconds {
        let value = value.clamp(30, 3600).to_string();
        push_setting!("offline_threshold_seconds", &value);
    }
    if let Some(value) = input.history_retention_days {
        let value = value.clamp(1, 365).to_string();
        push_setting!("history_retention_days", &value);
    }
    if let Some(value) = input.default_theme.as_deref() {
        push_setting!("default_theme", value);
    }
    if let Some(value) = input.background_url.as_deref() {
        push_setting!("background_url", value.trim());
    }
    if let Some(value) = input.theme_url.as_deref() {
        push_setting!("theme_url", value.trim());
    }
    if let Some(value) = input.theme_options.as_ref() {
        let serialized = value.to_string();
        push_setting!("theme_options", &serialized);
    }
    if let Some(value) = input.show_search {
        push_setting!("show_search", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_groups {
        push_setting!("show_groups", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_stats {
        push_setting!("show_stats", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_assets {
        push_setting!("show_assets", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_traffic {
        push_setting!("show_traffic", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_speed {
        push_setting!("show_speed", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_price {
        push_setting!("show_price", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_expiry {
        push_setting!("show_expiry", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_latency {
        push_setting!("show_latency", if value { "true" } else { "false" });
    }
    if let Some(value) = input.show_uptime {
        push_setting!("show_uptime", if value { "true" } else { "false" });
    }
    if let Some(value) = input.admin_username.as_deref() {
        push_setting!("admin_username", value.trim());
    }
    if let Some(value) = password_hash {
        push_setting!("admin_password_hash", value);
    }
    if let Some(value) = input.turnstile_enabled {
        push_setting!("turnstile_enabled", if value { "true" } else { "false" });
    }
    if let Some(value) = input.turnstile_login_enabled {
        push_setting!(
            "turnstile_login_enabled",
            if value { "true" } else { "false" }
        );
    }
    if let Some(value) = input.turnstile_site_key.as_deref() {
        push_setting!("turnstile_site_key", value.trim());
    }
    if let Some(value) = input.turnstile_secret_key.as_deref() {
        push_setting!("turnstile_secret_key", value.trim());
    }
    if let Some(value) = input.notification_enabled {
        push_setting!("notification_enabled", if value { "true" } else { "false" });
    }
    if let Some(value) = input.notification_endpoint.as_deref() {
        push_setting!("notification_endpoint", value.trim());
    }
    if let Some(value) = input.notification_target.as_deref() {
        push_setting!("notification_target", value.trim());
    }
    if let Some(value) = input.offline_alert_minutes {
        let value = value.clamp(2, 1440).to_string();
        push_setting!("offline_alert_minutes", &value);
    }
    if let Some(value) = input.expiry_alert_days {
        let value = value.clamp(0, 365).to_string();
        push_setting!("expiry_alert_days", &value);
    }
    macro_rules! push_trimmed {
        ($field:expr, $key:expr) => {
            if let Some(value) = $field.as_deref() {
                push_setting!($key, value.trim());
            }
        };
    }
    push_trimmed!(input.cloudflare_account_id, "cloudflare_account_id");
    push_trimmed!(input.cloudflare_api_token, "cloudflare_api_token");
    push_trimmed!(input.cors_allowed_origins, "cors_allowed_origins");
    push_trimmed!(input.csp_asset_origins, "csp_asset_origins");
    if let Some(value) = input.federation_sites.as_ref() {
        let serialized = value.to_string();
        push_setting!("federation_sites", &serialized);
    }
    if !statements.is_empty() {
        db.batch(statements).await?;
    }
    Ok(())
}

pub async fn cleanup_history(db: &D1Database, retention_days: i64) -> Result<()> {
    let cutoff = now() - retention_days.clamp(1, 365) * 86_400;
    let recent_cutoff = now() - 7 * 86_400;
    let compact = db
        .prepare(
            r#"INSERT INTO metric_history_hourly (
          server_id, timestamp, cpu, load1, load5, load15, mem_used, mem_total,
          swap_used, swap_total, disk_used, disk_total, net_in, net_out,
          net_rx_total, net_tx_total, processes, tcp_connections, udp_connections,
          gpu_usage, disk_read_bps, disk_write_bps, disk_read_iops, disk_write_iops,
          disk_await_ms, disk_utilization
        ) SELECT
          server_id, (timestamp / 3600) * 3600,
          AVG(cpu), AVG(load1), AVG(load5), AVG(load15), CAST(AVG(mem_used) AS INTEGER),
          MAX(mem_total), CAST(AVG(swap_used) AS INTEGER), MAX(swap_total),
          CAST(AVG(disk_used) AS INTEGER), MAX(disk_total), AVG(net_in), AVG(net_out),
          MAX(net_rx_total), MAX(net_tx_total), CAST(AVG(processes) AS INTEGER),
          CAST(AVG(tcp_connections) AS INTEGER), CAST(AVG(udp_connections) AS INTEGER),
          AVG(gpu_usage), AVG(disk_read_bps), AVG(disk_write_bps), AVG(disk_read_iops),
          AVG(disk_write_iops), AVG(disk_await_ms), AVG(disk_utilization)
        FROM metric_history WHERE timestamp >= ?1 AND timestamp < ?2
        GROUP BY server_id, timestamp / 3600
        ON CONFLICT(server_id, timestamp) DO UPDATE SET
          cpu=excluded.cpu, load1=excluded.load1, load5=excluded.load5,
          load15=excluded.load15, mem_used=excluded.mem_used, mem_total=excluded.mem_total,
          swap_used=excluded.swap_used, swap_total=excluded.swap_total,
          disk_used=excluded.disk_used, disk_total=excluded.disk_total,
          net_in=excluded.net_in, net_out=excluded.net_out,
          net_rx_total=excluded.net_rx_total, net_tx_total=excluded.net_tx_total,
          processes=excluded.processes, tcp_connections=excluded.tcp_connections,
          udp_connections=excluded.udp_connections, gpu_usage=excluded.gpu_usage,
          disk_read_bps=excluded.disk_read_bps, disk_write_bps=excluded.disk_write_bps,
          disk_read_iops=excluded.disk_read_iops, disk_write_iops=excluded.disk_write_iops,
          disk_await_ms=excluded.disk_await_ms, disk_utilization=excluded.disk_utilization"#,
        )
        .bind(&[number(cutoff), number(recent_cutoff)])?;
    let delete_recent = db
        .prepare("DELETE FROM metric_history WHERE timestamp < ?1")
        .bind(&[number(recent_cutoff)])?;
    let delete_archive = db
        .prepare("DELETE FROM metric_history_hourly WHERE timestamp < ?1")
        .bind(&[number(cutoff)])?;
    db.batch(vec![compact, delete_recent, delete_archive])
        .await?;
    crate::latency::cleanup_history(db, cutoff).await?;
    Ok(())
}

pub async fn clear_history(db: &D1Database) -> Result<()> {
    db.batch(vec![
        db.prepare("DELETE FROM metric_history"),
        db.prepare("DELETE FROM metric_history_hourly"),
    ])
    .await?;
    crate::latency::clear_history(db).await?;
    Ok(())
}

pub async fn exchange_rate_snapshot(
    db: &D1Database,
    base_currency: &str,
) -> Result<Option<ExchangeRateSnapshot>> {
    db.prepare(
        "SELECT base_currency, rates_json, source, rate_date, fetched_at, attempted_at \
         FROM exchange_rate_snapshots WHERE base_currency = ?1",
    )
    .bind(&[text(base_currency)])?
    .first(None)
    .await
}

pub async fn mark_exchange_rate_attempt(
    db: &D1Database,
    base_currency: &str,
    attempted_at: i64,
) -> Result<()> {
    db.prepare(
        "INSERT INTO exchange_rate_snapshots( \
           base_currency, rates_json, source, rate_date, fetched_at, attempted_at \
         ) VALUES (?1, '{}', 'default', '', 0, ?2) \
         ON CONFLICT(base_currency) DO UPDATE SET attempted_at=excluded.attempted_at",
    )
    .bind(&[text(base_currency), number(attempted_at)])?
    .run()
    .await?;
    Ok(())
}

pub async fn upsert_exchange_rate_snapshot(
    db: &D1Database,
    base_currency: &str,
    rates_json: &str,
    source: &str,
    rate_date: &str,
    fetched_at: i64,
) -> Result<()> {
    db.prepare(
        "INSERT INTO exchange_rate_snapshots( \
           base_currency, rates_json, source, rate_date, fetched_at, attempted_at \
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?5) \
         ON CONFLICT(base_currency) DO UPDATE SET \
           rates_json=excluded.rates_json, source=excluded.source, \
           rate_date=excluded.rate_date, fetched_at=excluded.fetched_at, \
           attempted_at=excluded.attempted_at",
    )
    .bind(&[
        text(base_currency),
        text(rates_json),
        text(source),
        text(rate_date),
        number(fetched_at),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn database_stats(db: &D1Database, offline_threshold: i64) -> Result<DatabaseStats> {
    let cutoff = now() - offline_threshold.clamp(30, 3600);
    db.prepare(
        r#"SELECT
          (SELECT COUNT(*) FROM servers) AS server_count,
          (SELECT COUNT(*) FROM latest_metrics WHERE timestamp >= ?1) AS online_count,
          ((SELECT COUNT(*) FROM metric_history) +
           (SELECT COUNT(*) FROM metric_history_hourly)) AS history_rows,
          (SELECT MIN(timestamp) FROM (
             SELECT timestamp FROM metric_history UNION ALL
             SELECT timestamp FROM metric_history_hourly
           )) AS oldest_history,
          (SELECT MAX(timestamp) FROM (
             SELECT timestamp FROM metric_history UNION ALL
             SELECT timestamp FROM metric_history_hourly
           )) AS newest_history"#,
    )
    .bind(&[number(cutoff)])?
    .first(None)
    .await?
    .ok_or_else(|| worker::Error::RustError("无法读取数据库统计".to_string()))
}

pub async fn get_setting(db: &D1Database, key: &str) -> Result<Option<String>> {
    let row: Option<SettingRow> = db
        .prepare("SELECT key, value FROM settings WHERE key = ?1")
        .bind(&[text(key)])?
        .first(None)
        .await?;
    Ok(row.map(|value| value.value))
}

pub async fn save_setting(db: &D1Database, key: &str, value: &str) -> Result<()> {
    db.prepare(
        "INSERT INTO settings(key, value, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
    )
    .bind(&[text(key), text(value), number(now())])?
    .run()
    .await?;
    Ok(())
}

pub async fn update_expiry(db: &D1Database, id: &str, expires_at: i64) -> Result<()> {
    db.prepare("UPDATE servers SET expires_at = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(&[text(id), number(expires_at), number(now())])?
        .run()
        .await?;
    Ok(())
}
