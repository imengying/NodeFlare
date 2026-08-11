use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use worker::{wasm_bindgen::JsValue, D1Database, Date, Result};

use crate::models::{AgentLatencyResult, AgentLatencyTask, LatencyTaskInput};

#[derive(Debug, Deserialize)]
struct TaskRow {
    id: String,
    name: String,
    task_type: String,
    target: String,
    interval_seconds: i64,
    default_enabled: i64,
}

#[derive(Debug, Deserialize)]
struct AssignmentRow {
    task_id: String,
    server_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LatencyTaskView {
    pub id: String,
    pub name: String,
    pub task_type: String,
    pub target: String,
    pub interval_seconds: i64,
    pub default_enabled: bool,
    pub server_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LatencySample {
    pub task_id: String,
    pub server_id: String,
    pub name: String,
    pub task_type: String,
    pub target: String,
    pub timestamp: i64,
    pub latency_ms: f64,
    pub packet_loss: f64,
}

fn text(value: &str) -> JsValue {
    JsValue::from_str(value)
}

fn number(value: impl ToString) -> JsValue {
    JsValue::from_f64(value.to_string().parse::<f64>().unwrap_or(0.0))
}

pub async fn list_tasks(db: &D1Database) -> Result<Vec<LatencyTaskView>> {
    let rows: Vec<TaskRow> = db
        .prepare(
            "SELECT id, name, task_type, target, interval_seconds, default_enabled \
             FROM latency_tasks \
             ORDER BY sort_order ASC, created_at ASC",
        )
        .all()
        .await?
        .results()?;
    let assignments: Vec<AssignmentRow> = db
        .prepare(
            "SELECT task_id, server_id FROM latency_task_servers \
             ORDER BY task_id ASC, server_id ASC",
        )
        .all()
        .await?
        .results()?;
    let mut by_task: HashMap<String, Vec<String>> = HashMap::new();
    for assignment in assignments {
        by_task
            .entry(assignment.task_id)
            .or_default()
            .push(assignment.server_id);
    }
    Ok(rows
        .into_iter()
        .map(|row| LatencyTaskView {
            server_ids: by_task.remove(&row.id).unwrap_or_default(),
            id: row.id,
            name: row.name,
            task_type: row.task_type,
            target: row.target,
            interval_seconds: row.interval_seconds,
            default_enabled: row.default_enabled != 0,
        })
        .collect())
}

pub async fn tasks_for_server(db: &D1Database, server_id: &str) -> Result<Vec<AgentLatencyTask>> {
    db.prepare(
        "SELECT t.id, t.name, t.task_type, t.target, t.interval_seconds \
         FROM latency_tasks t \
         INNER JOIN latency_task_servers a ON a.task_id = t.id \
         WHERE a.server_id = ?1 ORDER BY t.sort_order ASC, t.created_at ASC",
    )
    .bind(&[text(server_id)])?
    .all()
    .await?
    .results()
}

pub async fn create_task(
    db: &D1Database,
    id: &str,
    input: &LatencyTaskInput,
    timestamp: i64,
) -> Result<()> {
    let mut statements = vec![db
        .prepare(
            "INSERT INTO latency_tasks( \
               id, name, task_type, target, interval_seconds, default_enabled, \
               sort_order, created_at, updated_at \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, \
               COALESCE((SELECT MAX(sort_order) + 1 FROM latency_tasks), 0), ?7, ?7)",
        )
        .bind(&[
            text(id),
            text(input.name.trim()),
            text(input.task_type.trim()),
            text(input.target.trim()),
            number(input.interval_seconds),
            JsValue::from_bool(input.default_enabled),
            number(timestamp),
        ])?];
    let assignment =
        db.prepare("INSERT INTO latency_task_servers(task_id, server_id) VALUES (?1, ?2)");
    for server_id in &input.server_ids {
        statements.push(assignment.clone().bind(&[text(id), text(server_id)])?);
    }
    db.batch(statements).await?;
    Ok(())
}

pub async fn update_task(
    db: &D1Database,
    id: &str,
    input: &LatencyTaskInput,
    timestamp: i64,
) -> Result<bool> {
    let exists: Option<TaskRow> = db
        .prepare(
            "SELECT id, name, task_type, target, interval_seconds, default_enabled \
             FROM latency_tasks WHERE id = ?1",
        )
        .bind(&[text(id)])?
        .first(None)
        .await?;
    if exists.is_none() {
        return Ok(false);
    }
    let mut statements = vec![
        db.prepare(
            "UPDATE latency_tasks SET name = ?2, task_type = ?3, target = ?4, \
             interval_seconds = ?5, default_enabled = ?6, updated_at = ?7 WHERE id = ?1",
        )
        .bind(&[
            text(id),
            text(input.name.trim()),
            text(input.task_type.trim()),
            text(input.target.trim()),
            number(input.interval_seconds),
            JsValue::from_bool(input.default_enabled),
            number(timestamp),
        ])?,
        db.prepare("DELETE FROM latency_task_servers WHERE task_id = ?1")
            .bind(&[text(id)])?,
    ];
    let assignment =
        db.prepare("INSERT INTO latency_task_servers(task_id, server_id) VALUES (?1, ?2)");
    for server_id in &input.server_ids {
        statements.push(assignment.clone().bind(&[text(id), text(server_id)])?);
    }
    db.batch(statements).await?;
    Ok(true)
}

pub async fn delete_task(db: &D1Database, id: &str) -> Result<bool> {
    let result = db
        .prepare("DELETE FROM latency_tasks WHERE id = ?1")
        .bind(&[text(id)])?
        .run()
        .await?;
    Ok(result.meta()?.and_then(|meta| meta.changes).unwrap_or(0) > 0)
}

pub async fn assign_defaults(db: &D1Database, server_id: &str) -> Result<()> {
    db.prepare(
        "INSERT OR IGNORE INTO latency_task_servers(task_id, server_id) \
         SELECT id, ?1 FROM latency_tasks WHERE default_enabled = 1",
    )
    .bind(&[text(server_id)])?
    .run()
    .await?;
    Ok(())
}

pub async fn save_results(
    db: &D1Database,
    server_id: &str,
    results: &[AgentLatencyResult],
    received_at: i64,
) -> Result<()> {
    if results.is_empty() {
        return Ok(());
    }
    let assigned: HashSet<String> = tasks_for_server(db, server_id)
        .await?
        .into_iter()
        .map(|task| task.id)
        .collect();
    if results
        .iter()
        .any(|result| !assigned.contains(&result.task_id))
    {
        return Err(worker::Error::RustError(
            "延迟结果包含未分配给该节点的任务".to_string(),
        ));
    }

    let latest = db.prepare(
        "INSERT INTO latency_latest( \
           task_id, server_id, timestamp, latency_ms, packet_loss \
         ) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(task_id, server_id) DO UPDATE SET \
           timestamp=excluded.timestamp, latency_ms=excluded.latency_ms, \
           packet_loss=excluded.packet_loss \
         WHERE excluded.timestamp >= latency_latest.timestamp",
    );
    let historical = db.prepare(
        "INSERT INTO latency_history( \
           task_id, server_id, timestamp, latency_ms, packet_loss \
         ) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(task_id, server_id, timestamp) DO UPDATE SET \
           latency_ms=excluded.latency_ms, packet_loss=excluded.packet_loss",
    );
    let mut statements = Vec::with_capacity(results.len() * 2);
    for result in results {
        let timestamp = if (result.timestamp - received_at).abs() <= 86_400 {
            result.timestamp
        } else {
            received_at
        };
        let values = [
            text(&result.task_id),
            text(server_id),
            number(timestamp),
            number(result.latency_ms),
            number(result.packet_loss),
        ];
        statements.push(latest.clone().bind(&values)?);
        statements.push(historical.clone().bind(&values)?);
    }
    db.batch(statements).await?;
    Ok(())
}

pub async fn latest_all(db: &D1Database) -> Result<Vec<LatencySample>> {
    db.prepare(
        "SELECT l.task_id, l.server_id, t.name, t.task_type, t.target, \
         l.timestamp, l.latency_ms, l.packet_loss \
         FROM latency_latest l INNER JOIN latency_tasks t ON t.id = l.task_id \
         ORDER BY t.sort_order ASC, t.created_at ASC",
    )
    .all()
    .await?
    .results()
}

pub async fn history(db: &D1Database, server_id: &str, hours: i64) -> Result<Vec<LatencySample>> {
    let cutoff = (Date::now().as_millis() as i64 / 1000) - hours.clamp(1, 24 * 365) * 3600;
    db.prepare(
        "SELECT h.task_id, h.server_id, t.name, t.task_type, t.target, \
         h.timestamp, h.latency_ms, h.packet_loss \
         FROM latency_history h INNER JOIN latency_tasks t ON t.id = h.task_id \
         WHERE h.server_id = ?1 AND h.timestamp >= ?2 \
         ORDER BY h.timestamp ASC, t.sort_order ASC",
    )
    .bind(&[text(server_id), number(cutoff)])?
    .all()
    .await?
    .results()
}

pub async fn cleanup_history(db: &D1Database, cutoff: i64) -> Result<()> {
    db.prepare("DELETE FROM latency_history WHERE timestamp < ?1")
        .bind(&[number(cutoff)])?
        .run()
        .await?;
    Ok(())
}

pub async fn clear_history(db: &D1Database) -> Result<()> {
    db.prepare("DELETE FROM latency_history").run().await?;
    Ok(())
}
