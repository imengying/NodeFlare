use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize, Serializer};
use worker::{wasm_bindgen::JsValue, D1Database, Date, Result};

use crate::models::{
    AgentConfigView, AgentIdentityRow, AgentReport, AlertRuleInput, AlertRuleView, HistoryPoint,
    ServerInput, ServerView, SettingsInput, ThemeInput, ThemeView,
};

const SERVER_SELECT: &str = r#"
SELECT
  s.id, s.name, s.region, s.group_name, s.tags, s.hidden,
  s.expires_at, s.traffic_limit, s.traffic_limit_type,
  s.price, s.billing_cycle, s.currency, s.auto_renewal,
  s.last_ip,
  s.network_interface, s.reset_day, s.report_interval, s.collect_interval,
  s.rx_correction, s.tx_correction, s.offline_notify_disabled, s.auto_update,
  s.created_at,
  m.timestamp, m.cpu, m.load1, m.load5, m.load15, m.mem_used, m.mem_total,
  m.swap_used, m.swap_total, m.disk_used, m.disk_total, m.net_in, m.net_out,
  CASE WHEN m.net_rx_total IS NULL THEN NULL ELSE COALESCE(c.used_rx, m.net_rx_total) + s.rx_correction END AS net_rx_total,
  CASE WHEN m.net_tx_total IS NULL THEN NULL ELSE COALESCE(c.used_tx, m.net_tx_total) + s.tx_correction END AS net_tx_total,
  m.uptime, m.processes, m.tcp_connections,
  m.udp_connections, m.cpu_cores, m.cpu_model, m.os, m.kernel, m.arch,
  m.virtualization, m.gpu_usage, m.gpu_model, m.agent_version,
  m.disk_read_bps, m.disk_write_bps, m.disk_read_iops, m.disk_write_iops,
  m.disk_await_ms, m.disk_utilization, m.disk_info, m.gpu_info
FROM servers s
LEFT JOIN latest_metrics m ON m.server_id = s.id
LEFT JOIN traffic_cycles c ON c.server_id = s.id
"#;

pub const SECRET_MASK: &str = "********";

fn secret_for_api(value: &str) -> &str {
    if value.is_empty() {
        ""
    } else {
        SECRET_MASK
    }
}

fn serialize_secret<S>(value: &str, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(secret_for_api(value))
}

#[derive(Debug, serde::Deserialize)]
struct SettingRow {
    key: String,
    value: String,
}

#[derive(Debug, Deserialize)]
struct ThemeRow {
    id: String,
    name: String,
    description: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct AgentConfigRow {
    report_interval: i64,
    collect_interval: i64,
    network_interface: String,
    auto_update: i64,
}

#[derive(Debug, Deserialize)]
pub struct AgentLiveContext {
    pub report_interval: i64,
    pub collect_interval: i64,
    pub reset_day: i64,
    pub cycle_key: i64,
    pub raw_rx: i64,
    pub raw_tx: i64,
    pub used_rx: i64,
    pub used_tx: i64,
    pub rx_correction: i64,
    pub tx_correction: i64,
    pub last_persisted_at: i64,
}

#[derive(Serialize)]
struct TrafficCounterSample {
    timestamp: i64,
    cycle_key: i64,
    reset_day: i64,
    raw_rx: i64,
    raw_tx: i64,
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
    #[serde(skip_serializing)]
    pub history_cache_version: i64,
    pub default_theme: String,
    pub active_theme_id: String,
    pub background_url: String,
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
    #[serde(skip_serializing)]
    pub password_client_salt: String,
    pub turnstile_enabled: bool,
    pub turnstile_login_enabled: bool,
    #[serde(serialize_with = "serialize_secret")]
    pub turnstile_site_key: String,
    #[serde(serialize_with = "serialize_secret")]
    pub turnstile_secret_key: String,
    pub notification_enabled: bool,
    #[serde(serialize_with = "serialize_secret")]
    pub notification_endpoint: String,
    pub notification_target: String,
    pub offline_alert_minutes: i64,
    pub expiry_alert_days: i64,
    #[serde(serialize_with = "serialize_secret")]
    pub cloudflare_account_id: String,
    #[serde(serialize_with = "serialize_secret")]
    pub cloudflare_api_token: String,
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
    pub sample_count: i64,
    pub first_timestamp: i64,
    pub last_timestamp: i64,
    pub report_interval: i64,
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

fn civil_date_from_unix_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

pub(crate) fn traffic_cycle_key(timestamp: i64, reset_day: i64) -> i64 {
    let (year, month, day) = civil_date_from_unix_days(timestamp.div_euclid(86_400));
    let boundary = reset_day.clamp(1, 31).min(days_in_month(year, month));
    let current_month = year * 12 + month - 1;
    if day >= boundary {
        current_month
    } else {
        current_month - 1
    }
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

pub async fn get_agent_identity(db: &D1Database, token: &str) -> Result<Option<AgentIdentityRow>> {
    db.prepare("SELECT id, hidden FROM servers WHERE token = ?1 LIMIT 1")
        .bind(&[text(token)])?
        .first(None)
        .await
}

pub async fn agent_live_context(db: &D1Database, id: &str) -> Result<Option<AgentLiveContext>> {
    let mut context: Option<AgentLiveContext> = db
        .prepare(
            r#"SELECT
             s.report_interval,
             s.collect_interval,
             s.reset_day,
             COALESCE(c.cycle_key, 0) AS cycle_key,
             COALESCE(c.raw_rx, m.net_rx_total, 0) AS raw_rx,
             COALESCE(c.raw_tx, m.net_tx_total, 0) AS raw_tx,
             COALESCE(c.used_rx, m.net_rx_total, 0) AS used_rx,
             COALESCE(c.used_tx, m.net_tx_total, 0) AS used_tx,
             s.rx_correction,
             s.tx_correction,
             COALESCE(m.timestamp, 0) AS last_persisted_at
           FROM servers s
           LEFT JOIN latest_metrics m ON m.server_id = s.id
           LEFT JOIN traffic_cycles c ON c.server_id = s.id
           WHERE s.id = ?1
           LIMIT 1"#,
        )
        .bind(&[text(id)])?
        .first(None)
        .await?;
    if let Some(context) = context.as_mut() {
        if context.cycle_key == 0 {
            context.cycle_key = traffic_cycle_key(crate::now(), context.reset_day);
        }
    }
    Ok(context)
}

pub async fn get_agent_token(db: &D1Database, id: &str) -> Result<Option<String>> {
    db.prepare("SELECT token FROM servers WHERE id = ?1 LIMIT 1")
        .bind(&[text(id)])?
        .first(Some("token"))
        .await
}

pub async fn update_last_ip(db: &D1Database, id: &str, ip: &str) -> Result<()> {
    db.prepare("UPDATE servers SET last_ip = ?2 WHERE id = ?1 AND last_ip != ?2")
        .bind(&[text(id), text(ip)])?
        .run()
        .await?;
    Ok(())
}

pub async fn agent_config(db: &D1Database, id: &str) -> Result<Option<AgentConfigView>> {
    let row: Option<AgentConfigRow> = db
        .prepare(
            "SELECT report_interval, collect_interval, network_interface, auto_update \
             FROM servers WHERE id = ?1",
        )
        .bind(&[text(id)])?
        .first(None)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(AgentConfigView {
        report_interval: row.report_interval,
        collect_interval: row.collect_interval,
        network_interface: row.network_interface.trim().to_string(),
        auto_update: row.auto_update != 0,
        latency_tasks: crate::latency::tasks_for_server(db, id).await?,
    }))
}

pub async fn create_server(
    db: &D1Database,
    id: &str,
    token: &str,
    input: &ServerInput,
) -> Result<()> {
    let timestamp = now();
    db.prepare(
        r#"INSERT INTO servers (
          id, name, region, group_name, tags, hidden, sort_order,
          expires_at, traffic_limit, traffic_limit_type, price, billing_cycle,
          currency, auto_renewal, network_interface, reset_day,
          report_interval, collect_interval, rx_correction, tx_correction, offline_notify_disabled, auto_update,
          token, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6,
          COALESCE((SELECT MAX(sort_order) + 1 FROM servers), 0),
          ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
          ?18, ?19, ?20, ?21, ?22, ?23, ?23)"#,
    )
    .bind(&[
        text(id),
        text(input.name.trim()),
        text(input.region.trim()),
        text(input.group_name.trim()),
        text(input.tags.trim()),
        JsValue::from_bool(input.hidden),
        input.expires_at.map(number).unwrap_or(JsValue::NULL),
        number(input.traffic_limit),
        text(input.traffic_limit_type.trim()),
        number(input.price),
        number(input.billing_cycle),
        text(input.currency.trim()),
        JsValue::from_bool(input.auto_renewal),
        text(input.network_interface.trim()),
        number(input.reset_day),
        number(input.report_interval),
        number(input.collect_interval),
        number(input.rx_correction),
        number(input.tx_correction),
        JsValue::from_bool(input.offline_notify_disabled),
        JsValue::from_bool(input.auto_update),
        text(token),
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
              name = ?2, region = ?3, group_name = ?4, tags = ?5,
              hidden = ?6, expires_at = ?7, traffic_limit = ?8,
              traffic_limit_type = ?9, price = ?10, billing_cycle = ?11,
              currency = ?12, auto_renewal = ?13,
              network_interface = ?14, reset_day = ?15, report_interval = ?16,
              collect_interval = ?17, rx_correction = ?18, tx_correction = ?19,
              offline_notify_disabled = ?20, auto_update = ?21, updated_at = ?22
            WHERE id = ?1"#,
        )
        .bind(&[
            text(id),
            text(input.name.trim()),
            text(input.region.trim()),
            text(input.group_name.trim()),
            text(input.tags.trim()),
            JsValue::from_bool(input.hidden),
            input.expires_at.map(number).unwrap_or(JsValue::NULL),
            number(input.traffic_limit),
            text(input.traffic_limit_type.trim()),
            number(input.price),
            number(input.billing_cycle),
            text(input.currency.trim()),
            JsValue::from_bool(input.auto_renewal),
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
            "SELECT id, name, metric, threshold, duration_minutes, aggregation, enabled \
             FROM alert_rules ORDER BY created_at ASC",
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
            enabled: row.enabled != 0,
            server_ids: servers.into_iter().map(|row| row.server_id).collect(),
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
    let query = format!(
        "SELECT s.id AS server_id, s.name, {aggregate}({metric}) AS value, \
           COUNT(*) AS sample_count, MIN(h.timestamp) AS first_timestamp, \
           MAX(h.timestamp) AS last_timestamp, s.report_interval \
         FROM servers s JOIN metric_history h ON h.server_id=s.id \
         WHERE h.timestamp >= ?1 AND s.hidden=0 AND (\
           NOT EXISTS(SELECT 1 FROM alert_rule_servers ars WHERE ars.rule_id=?2) OR \
           EXISTS(SELECT 1 FROM alert_rule_servers ars WHERE ars.rule_id=?2 AND ars.server_id=s.id)\
         ) GROUP BY s.id, s.name, s.report_interval"
    );
    let rows: Vec<AlertMetricRow> = db
        .prepare(query)
        .bind(&[number(since), text(&rule.id)])?
        .all()
        .await?
        .results()?;
    Ok(rows
        .into_iter()
        .filter(|row| alert_window_covered(row, rule.duration_minutes, current_time))
        .collect())
}

fn alert_window_covered(row: &AlertMetricRow, duration_minutes: i64, current_time: i64) -> bool {
    let window_seconds = duration_minutes.clamp(1, 1440) * 60;
    let report_interval = row.report_interval.clamp(15, 3600).max(60);
    let expected_samples = ((window_seconds + report_interval - 1) / report_interval).max(1);
    let required_samples = if expected_samples == 1 {
        1
    } else {
        ((expected_samples * 3 + 4) / 5).max(2)
    };
    let fresh_within = (report_interval * 2).max(120);
    row.sample_count >= required_samples
        && row.first_timestamp <= row.last_timestamp
        && current_time.saturating_sub(row.last_timestamp) <= fresh_within
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

pub async fn sync_active_alert_states(
    db: &D1Database,
    previous: &std::collections::HashSet<String>,
    current: &std::collections::HashSet<String>,
) -> Result<()> {
    let mut statements = Vec::new();
    let delete = db.prepare("DELETE FROM alert_states WHERE rule_id=?1 AND server_id=?2");
    let insert = db.prepare(
        "INSERT INTO alert_states(rule_id, server_id, active, updated_at) VALUES (?1, ?2, 1, ?3) \
         ON CONFLICT(rule_id, server_id) DO UPDATE SET active=1, updated_at=excluded.updated_at",
    );
    let timestamp = now();
    for key in previous.difference(current) {
        let Some((rule_id, server_id)) = key.split_once(':') else {
            continue;
        };
        statements.push(delete.clone().bind(&[text(rule_id), text(server_id)])?);
    }
    for key in current.difference(previous) {
        let Some((rule_id, server_id)) = key.split_once(':') else {
            continue;
        };
        statements.push(insert.clone().bind(&[
            text(rule_id),
            text(server_id),
            number(timestamp),
        ])?);
    }
    if !statements.is_empty() {
        db.batch(statements).await?;
    }
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
              virtualization, gpu_usage, gpu_model, agent_version,
              disk_read_bps, disk_write_bps, disk_read_iops, disk_write_iops,
              disk_await_ms, disk_utilization, disk_info, gpu_info
            ) VALUES (
              ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
              ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26,
              ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37
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
              virtualization=excluded.virtualization, gpu_usage=excluded.gpu_usage, gpu_model=excluded.gpu_model,
              agent_version=excluded.agent_version,
              disk_read_bps=excluded.disk_read_bps, disk_write_bps=excluded.disk_write_bps,
              disk_read_iops=excluded.disk_read_iops, disk_write_iops=excluded.disk_write_iops,
              disk_await_ms=excluded.disk_await_ms,
              disk_utilization=excluded.disk_utilization,
              disk_info=excluded.disk_info, gpu_info=excluded.gpu_info
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
            ) SELECT
              ?1, CAST(json_extract(value, '$.timestamp') AS INTEGER),
              CAST(json_extract(value, '$.cpu') AS REAL),
              CAST(json_extract(value, '$.load1') AS REAL),
              CAST(json_extract(value, '$.load5') AS REAL),
              CAST(json_extract(value, '$.load15') AS REAL),
              CAST(json_extract(value, '$.mem_used') AS INTEGER),
              CAST(json_extract(value, '$.mem_total') AS INTEGER),
              CAST(json_extract(value, '$.swap_used') AS INTEGER),
              CAST(json_extract(value, '$.swap_total') AS INTEGER),
              CAST(json_extract(value, '$.disk_used') AS INTEGER),
              CAST(json_extract(value, '$.disk_total') AS INTEGER),
              CAST(json_extract(value, '$.net_in') AS REAL),
              CAST(json_extract(value, '$.net_out') AS REAL),
              CAST(json_extract(value, '$.net_rx_total') AS INTEGER),
              CAST(json_extract(value, '$.net_tx_total') AS INTEGER),
              CAST(json_extract(value, '$.processes') AS INTEGER),
              CAST(json_extract(value, '$.tcp_connections') AS INTEGER),
              CAST(json_extract(value, '$.udp_connections') AS INTEGER),
              CAST(json_extract(value, '$.gpu_usage') AS REAL),
              CAST(json_extract(value, '$.disk_read_bps') AS REAL),
              CAST(json_extract(value, '$.disk_write_bps') AS REAL),
              CAST(json_extract(value, '$.disk_read_iops') AS REAL),
              CAST(json_extract(value, '$.disk_write_iops') AS REAL),
              CAST(json_extract(value, '$.disk_await_ms') AS REAL),
              CAST(json_extract(value, '$.disk_utilization') AS REAL)
            FROM json_each(?2) WHERE true
            ON CONFLICT(server_id, timestamp) DO UPDATE SET
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
    let history_samples = minute_samples
        .into_iter()
        .map(|(timestamp, report)| HistoryPoint {
            timestamp,
            cpu: report.cpu,
            load1: report.load1,
            load5: report.load5,
            load15: report.load15,
            mem_used: report.mem_used,
            mem_total: report.mem_total,
            swap_used: report.swap_used,
            swap_total: report.swap_total,
            disk_used: report.disk_used,
            disk_total: report.disk_total,
            net_in: report.net_in,
            net_out: report.net_out,
            net_rx_total: report.net_rx_total,
            net_tx_total: report.net_tx_total,
            processes: report.processes,
            tcp_connections: report.tcp_connections,
            udp_connections: report.udp_connections,
            gpu_usage: report.gpu_usage,
            disk_read_bps: report.disk_read_bps,
            disk_write_bps: report.disk_write_bps,
            disk_read_iops: report.disk_read_iops,
            disk_write_iops: report.disk_write_iops,
            disk_await_ms: report.disk_await_ms,
            disk_utilization: report.disk_utilization,
        })
        .collect::<Vec<_>>();
    let history_json = serde_json::to_string(&history_samples)?;
    let history = history_statement.bind(&[text(server_id), text(&history_json)])?;
    let reset_day = db
        .prepare("SELECT reset_day FROM servers WHERE id = ?1")
        .bind(&[text(server_id)])?
        .first::<i64>(Some("reset_day"))
        .await?
        .unwrap_or(1);
    let mut traffic_samples = reports
        .iter()
        .map(|report| TrafficCounterSample {
            timestamp: report.timestamp,
            cycle_key: traffic_cycle_key(report.timestamp, reset_day),
            reset_day,
            raw_rx: report.net_rx_total,
            raw_tx: report.net_tx_total,
        })
        .collect::<Vec<_>>();
    traffic_samples.sort_by_key(|sample| sample.timestamp);
    let traffic_json = serde_json::to_string(&traffic_samples)?;
    let traffic = db
        .prepare(
            r#"INSERT INTO traffic_cycles(
                 server_id, cycle_key, reset_day, timestamp, raw_rx, raw_tx, used_rx, used_tx
               ) SELECT
                 ?1,
                 CAST(json_extract(value, '$.cycle_key') AS INTEGER),
                 CAST(json_extract(value, '$.reset_day') AS INTEGER),
                 CAST(json_extract(value, '$.timestamp') AS INTEGER),
                 CAST(json_extract(value, '$.raw_rx') AS INTEGER),
                 CAST(json_extract(value, '$.raw_tx') AS INTEGER),
                 CAST(json_extract(value, '$.raw_rx') AS INTEGER),
                 CAST(json_extract(value, '$.raw_tx') AS INTEGER)
               FROM json_each(?2) WHERE true ORDER BY CAST(json_extract(value, '$.timestamp') AS INTEGER)
               ON CONFLICT(server_id) DO UPDATE SET
                 cycle_key=excluded.cycle_key,
                 reset_day=excluded.reset_day,
                 timestamp=excluded.timestamp,
                 raw_rx=excluded.raw_rx,
                 raw_tx=excluded.raw_tx,
                 used_rx=CASE
                   WHEN excluded.cycle_key != traffic_cycles.cycle_key THEN 0
                   WHEN excluded.reset_day != traffic_cycles.reset_day
                     THEN 0
                   WHEN excluded.raw_rx >= traffic_cycles.raw_rx
                     THEN traffic_cycles.used_rx + excluded.raw_rx - traffic_cycles.raw_rx
                   ELSE traffic_cycles.used_rx + excluded.raw_rx
                 END,
                 used_tx=CASE
                   WHEN excluded.cycle_key != traffic_cycles.cycle_key THEN 0
                   WHEN excluded.reset_day != traffic_cycles.reset_day
                     THEN 0
                   WHEN excluded.raw_tx >= traffic_cycles.raw_tx
                     THEN traffic_cycles.used_tx + excluded.raw_tx - traffic_cycles.raw_tx
                   ELSE traffic_cycles.used_tx + excluded.raw_tx
                 END
               WHERE excluded.timestamp > traffic_cycles.timestamp"#,
        )
        .bind(&[text(server_id), text(&traffic_json)])?;
    db.batch(vec![latest, history, traffic]).await?;
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

fn non_empty_string_setting(values: &HashMap<String, String>, key: &str, fallback: &str) -> String {
    values
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.trim())
        .to_string()
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
    default_retention: i64,
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
        history_retention_days: integer_setting(
            &values,
            "history_retention_days",
            default_retention.clamp(1, 365),
        ),
        history_cache_version: integer_setting(&values, "history_cache_version", 0),
        default_theme: string_setting(&values, "default_theme", "system"),
        active_theme_id: string_setting(&values, "active_theme_id", crate::theme::BUILTIN_THEME_ID),
        background_url: string_setting(&values, "background_url", ""),
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
        admin_username: non_empty_string_setting(&values, "admin_username", default_username),
        admin_password_configured: values
            .get("admin_password_hash")
            .is_some_and(|value| !value.is_empty()),
        admin_password_hash: string_setting(&values, "admin_password_hash", ""),
        password_client_salt: string_setting(&values, "password_client_salt", ""),
        turnstile_enabled: bool_setting(&values, "turnstile_enabled", false),
        turnstile_login_enabled: bool_setting(&values, "turnstile_login_enabled", true),
        turnstile_site_key: string_setting(&values, "turnstile_site_key", ""),
        turnstile_secret_key: string_setting(&values, "turnstile_secret_key", ""),
        notification_enabled: bool_setting(&values, "notification_enabled", false),
        notification_endpoint: string_setting(&values, "notification_endpoint", ""),
        notification_target: string_setting(&values, "notification_target", ""),
        offline_alert_minutes: integer_setting(&values, "offline_alert_minutes", 5),
        expiry_alert_days: integer_setting(&values, "expiry_alert_days", 7),
        cloudflare_account_id: string_setting(&values, "cloudflare_account_id", ""),
        cloudflare_api_token: string_setting(&values, "cloudflare_api_token", ""),
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
    if let Some(value) = input.active_theme_id.as_deref() {
        push_setting!("active_theme_id", value.trim());
    }
    if let Some(value) = input.background_url.as_deref() {
        push_setting!("background_url", value.trim());
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
    if let Some(value) = input
        .turnstile_site_key
        .as_deref()
        .filter(|value| value.trim() != SECRET_MASK)
    {
        push_setting!("turnstile_site_key", value.trim());
    }
    if let Some(value) = input
        .turnstile_secret_key
        .as_deref()
        .filter(|value| value.trim() != SECRET_MASK)
    {
        push_setting!("turnstile_secret_key", value.trim());
    }
    if let Some(value) = input.notification_enabled {
        push_setting!("notification_enabled", if value { "true" } else { "false" });
    }
    if let Some(value) = input
        .notification_endpoint
        .as_deref()
        .filter(|value| value.trim() != SECRET_MASK)
    {
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
    if let Some(value) = input
        .cloudflare_account_id
        .as_deref()
        .filter(|value| value.trim() != SECRET_MASK)
    {
        push_setting!("cloudflare_account_id", value.trim());
    }
    if let Some(value) = input
        .cloudflare_api_token
        .as_deref()
        .filter(|value| value.trim() != SECRET_MASK)
    {
        push_setting!("cloudflare_api_token", value.trim());
    }
    if !statements.is_empty() {
        db.batch(statements).await?;
    }
    Ok(())
}

pub async fn list_themes(db: &D1Database, active_id: &str) -> Result<Vec<ThemeView>> {
    let rows: Vec<ThemeRow> = db
        .prepare("SELECT id, name, description, url FROM themes ORDER BY created_at DESC")
        .all()
        .await?
        .results()?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let active = row.id == active_id;
            ThemeView {
                id: row.id,
                name: row.name,
                description: row.description,
                url: row.url,
                builtin: false,
                active,
            }
        })
        .collect())
}

pub async fn create_theme(
    db: &D1Database,
    id: &str,
    input: &ThemeInput,
    source_url: &str,
    timestamp: i64,
) -> Result<()> {
    db.prepare(
        "INSERT INTO themes(id, name, description, url, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&[
        text(id),
        text(input.name.trim()),
        text(input.description.trim()),
        text(source_url),
        number(timestamp),
    ])?
    .run()
    .await?;
    Ok(())
}

pub async fn theme_exists(db: &D1Database, id: &str) -> Result<bool> {
    let row: Option<ThemeRow> = db
        .prepare("SELECT id, name, description, url FROM themes WHERE id = ?1")
        .bind(&[text(id)])?
        .first(None)
        .await?;
    Ok(row.is_some())
}

pub async fn theme_url(db: &D1Database, id: &str) -> Result<Option<String>> {
    let row: Option<ThemeRow> = db
        .prepare("SELECT id, name, description, url FROM themes WHERE id = ?1")
        .bind(&[text(id)])?
        .first(None)
        .await?;
    row.map(|theme| crate::theme::resolve_url(&theme.url).map(|resolved| resolved.resolved_url))
        .transpose()
}

pub async fn set_active_theme(db: &D1Database, id: &str) -> Result<bool> {
    if id != crate::theme::BUILTIN_THEME_ID && !theme_exists(db, id).await? {
        return Ok(false);
    }
    save_setting(db, "active_theme_id", id).await?;
    Ok(true)
}

pub async fn delete_theme(db: &D1Database, id: &str) -> Result<bool> {
    let result = db
        .prepare("DELETE FROM themes WHERE id = ?1")
        .bind(&[text(id)])?
        .run()
        .await?;
    let deleted = result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0;
    if deleted && get_setting(db, "active_theme_id").await?.as_deref() == Some(id) {
        save_setting(db, "active_theme_id", crate::theme::BUILTIN_THEME_ID).await?;
    }
    Ok(deleted)
}

pub async fn cleanup_history(db: &D1Database, retention_days: i64) -> Result<()> {
    let (cutoff, recent_cutoff) = history_cutoffs(now(), retention_days);
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

fn history_cutoffs(current: i64, retention_days: i64) -> (i64, i64) {
    let cutoff = current - retention_days.clamp(1, 365) * 86_400;
    let recent_cutoff = (current - 7 * 86_400).max(cutoff);
    (cutoff, recent_cutoff)
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

pub async fn increment_setting(db: &D1Database, key: &str) -> Result<()> {
    db.prepare(
        "INSERT INTO settings(key, value, updated_at) VALUES (?1, '1', ?2) \
         ON CONFLICT(key) DO UPDATE SET \
           value = CAST(CAST(settings.value AS INTEGER) + 1 AS TEXT), \
           updated_at = excluded.updated_at",
    )
    .bind(&[text(key), number(now())])?
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

#[cfg(test)]
mod tests {
    use super::{
        alert_window_covered, history_cutoffs, non_empty_string_setting, secret_for_api,
        traffic_cycle_key, AlertMetricRow, SECRET_MASK,
    };
    use std::collections::HashMap;

    fn row(samples: i64, last_timestamp: i64, report_interval: i64) -> AlertMetricRow {
        AlertMetricRow {
            server_id: "server".to_string(),
            name: "Server".to_string(),
            value: 90.0,
            sample_count: samples,
            first_timestamp: last_timestamp - samples.saturating_sub(1) * 60,
            last_timestamp,
            report_interval,
        }
    }

    #[test]
    fn alert_coverage_tracks_report_interval_and_freshness() {
        let current = 10_000;
        assert!(alert_window_covered(&row(6, current - 30, 60), 10, current));
        assert!(!alert_window_covered(
            &row(5, current - 30, 60),
            10,
            current
        ));
        assert!(alert_window_covered(
            &row(2, current - 60, 300),
            10,
            current
        ));
        assert!(!alert_window_covered(
            &row(2, current - 700, 300),
            10,
            current
        ));
    }

    #[test]
    fn masks_stored_secrets_in_api_responses() {
        assert_eq!(secret_for_api(""), "");
        assert_eq!(secret_for_api("stored-secret"), SECRET_MASK);
    }

    #[test]
    fn requires_an_explicit_admin_username() {
        let mut values = HashMap::from([("admin_username".to_string(), String::new())]);
        assert_eq!(
            non_empty_string_setting(&values, "admin_username", "operator"),
            "operator"
        );
        assert_eq!(non_empty_string_setting(&values, "admin_username", ""), "");
        values.insert("admin_username".to_string(), "owner".to_string());
        assert_eq!(
            non_empty_string_setting(&values, "admin_username", "operator"),
            "owner"
        );
    }

    #[test]
    fn honors_short_and_long_history_retention() {
        let current = 10_000_000;
        let (one_day, one_day_recent) = history_cutoffs(current, 1);
        assert_eq!(one_day, current - 86_400);
        assert_eq!(one_day_recent, one_day);

        let (thirty_days, thirty_days_recent) = history_cutoffs(current, 30);
        assert_eq!(thirty_days, current - 30 * 86_400);
        assert_eq!(thirty_days_recent, current - 7 * 86_400);
    }

    #[test]
    fn assigns_monthly_traffic_cycles_with_short_months() {
        let timestamp = |year: i64, month: i64, day: i64| {
            let year = year - i64::from(month <= 2);
            let era = year.div_euclid(400);
            let year_of_era = year - era * 400;
            let month_prime = month + if month > 2 { -3 } else { 9 };
            let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
            let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
            (era * 146_097 + day_of_era - 719_468) * 86_400
        };
        assert_eq!(traffic_cycle_key(timestamp(2026, 8, 9), 10), 2026 * 12 + 6);
        assert_eq!(traffic_cycle_key(timestamp(2026, 8, 10), 10), 2026 * 12 + 7);
        assert_eq!(traffic_cycle_key(timestamp(2028, 2, 28), 31), 2028 * 12);
        assert_eq!(traffic_cycle_key(timestamp(2028, 2, 29), 31), 2028 * 12 + 1);
    }
}
