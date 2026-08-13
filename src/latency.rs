use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use worker::{wasm_bindgen::JsValue, D1Database, Date, Result};

use crate::models::{AgentLatencyResult, AgentLatencyTask, LatencyTaskInput};

pub const MAX_LATENCY_TASKS: usize = 128;
const MAX_HISTORY_ROWS_PER_REPORT: usize = 4096;
const MAX_HISTORY_RESPONSE_ROWS: i64 = 4000;

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

#[derive(Clone, Debug, PartialEq, Serialize)]
struct StoredLatencyResult {
    task_id: String,
    timestamp: i64,
    latency_ms: f64,
    packet_loss: f64,
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

pub async fn task_count(db: &D1Database) -> Result<i64> {
    db.prepare("SELECT COUNT(*) AS count FROM latency_tasks")
        .first(Some("count"))
        .await?
        .ok_or_else(|| worker::Error::RustError("无法读取延迟任务数量".to_string()))
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
    let server_ids = serde_json::to_string(&input.server_ids)?;
    let statements = vec![
        db.prepare(
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
        ])?,
        db.prepare(
            "INSERT INTO latency_task_servers(task_id, server_id) \
             SELECT ?1, CAST(value AS TEXT) FROM json_each(?2)",
        )
        .bind(&[text(id), text(&server_ids)])?,
    ];
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
    let Some(existing) = exists else {
        return Ok(false);
    };
    let reset_history =
        existing.task_type != input.task_type.trim() || existing.target != input.target.trim();
    let server_ids = serde_json::to_string(&input.server_ids)?;
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
        db.prepare(
            "INSERT INTO latency_task_servers(task_id, server_id) \
             SELECT ?1, CAST(value AS TEXT) FROM json_each(?2)",
        )
        .bind(&[text(id), text(&server_ids)])?,
    ];
    if reset_history {
        statements.push(
            db.prepare("DELETE FROM latency_latest WHERE task_id = ?1")
                .bind(&[text(id)])?,
        );
        statements.push(
            db.prepare("DELETE FROM latency_history WHERE task_id = ?1")
                .bind(&[text(id)])?,
        );
    } else {
        statements.push(
            db.prepare(
                "DELETE FROM latency_latest WHERE task_id = ?1 AND NOT EXISTS ( \
                   SELECT 1 FROM latency_task_servers a \
                   WHERE a.task_id = latency_latest.task_id \
                     AND a.server_id = latency_latest.server_id \
                 )",
            )
            .bind(&[text(id)])?,
        );
        statements.push(
            db.prepare(
                "DELETE FROM latency_history WHERE task_id = ?1 AND NOT EXISTS ( \
                   SELECT 1 FROM latency_task_servers a \
                   WHERE a.task_id = latency_history.task_id \
                     AND a.server_id = latency_history.server_id \
                 )",
            )
            .bind(&[text(id)])?,
        );
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
    let (latest, history) = compact_results(results, &assigned, received_at);
    if latest.is_empty() {
        return Ok(());
    }
    let latest_json = serde_json::to_string(&latest)?;
    let history_json = serde_json::to_string(&history)?;
    let latest_statement = db
        .prepare(
            "INSERT INTO latency_latest( \
               task_id, server_id, timestamp, latency_ms, packet_loss \
             ) SELECT \
               json_extract(value, '$.task_id'), ?1, \
               CAST(json_extract(value, '$.timestamp') AS INTEGER), \
               CAST(json_extract(value, '$.latency_ms') AS REAL), \
               CAST(json_extract(value, '$.packet_loss') AS REAL) \
             FROM json_each(?2) WHERE true \
             ON CONFLICT(task_id, server_id) DO UPDATE SET \
               timestamp=excluded.timestamp, latency_ms=excluded.latency_ms, \
               packet_loss=excluded.packet_loss \
             WHERE excluded.timestamp >= latency_latest.timestamp",
        )
        .bind(&[text(server_id), text(&latest_json)])?;
    let history_statement = db
        .prepare(
            "INSERT INTO latency_history( \
               task_id, server_id, timestamp, latency_ms, packet_loss \
             ) SELECT \
               json_extract(value, '$.task_id'), ?1, \
               CAST(json_extract(value, '$.timestamp') AS INTEGER), \
               CAST(json_extract(value, '$.latency_ms') AS REAL), \
               CAST(json_extract(value, '$.packet_loss') AS REAL) \
             FROM json_each(?2) WHERE true \
             ON CONFLICT(task_id, server_id, timestamp) DO UPDATE SET \
               latency_ms=excluded.latency_ms, packet_loss=excluded.packet_loss",
        )
        .bind(&[text(server_id), text(&history_json)])?;
    db.batch(vec![latest_statement, history_statement]).await?;
    Ok(())
}

fn compact_results(
    results: &[AgentLatencyResult],
    assigned: &HashSet<String>,
    received_at: i64,
) -> (Vec<StoredLatencyResult>, Vec<StoredLatencyResult>) {
    let mut latest: HashMap<String, StoredLatencyResult> = HashMap::new();
    let mut history = std::collections::BTreeMap::new();
    for result in results
        .iter()
        .filter(|result| assigned.contains(&result.task_id))
    {
        let timestamp = if (result.timestamp - received_at).abs() <= 86_400 {
            result.timestamp
        } else {
            received_at
        };
        let stored = StoredLatencyResult {
            task_id: result.task_id.clone(),
            timestamp,
            latency_ms: result.latency_ms,
            packet_loss: result.packet_loss,
        };
        if latest
            .get(&result.task_id)
            .is_none_or(|current| timestamp >= current.timestamp)
        {
            latest.insert(result.task_id.clone(), stored.clone());
        }
        let minute = timestamp / 60 * 60;
        history.insert(
            (minute, result.task_id.clone()),
            StoredLatencyResult {
                timestamp: minute,
                ..stored
            },
        );
    }
    let mut latest = latest.into_values().collect::<Vec<_>>();
    latest.sort_by(|left, right| left.task_id.cmp(&right.task_id));
    let skip = history.len().saturating_sub(MAX_HISTORY_ROWS_PER_REPORT);
    let history = history
        .into_values()
        .skip(skip)
        .collect::<Vec<StoredLatencyResult>>();
    (latest, history)
}

pub async fn latest_all(db: &D1Database) -> Result<Vec<LatencySample>> {
    db.prepare(
        "SELECT l.task_id, l.server_id, t.name, t.task_type, t.target, \
         l.timestamp, l.latency_ms, l.packet_loss \
         FROM latency_latest l INNER JOIN latency_tasks t ON t.id = l.task_id \
         INNER JOIN latency_task_servers a \
           ON a.task_id = l.task_id AND a.server_id = l.server_id \
         ORDER BY t.sort_order ASC, t.created_at ASC",
    )
    .all()
    .await?
    .results()
}

pub async fn latest_for_server(db: &D1Database, server_id: &str) -> Result<Vec<LatencySample>> {
    db.prepare(
        "SELECT l.task_id, l.server_id, t.name, t.task_type, t.target, \
         l.timestamp, l.latency_ms, l.packet_loss \
         FROM latency_latest l INNER JOIN latency_tasks t ON t.id = l.task_id \
         INNER JOIN latency_task_servers a \
           ON a.task_id = l.task_id AND a.server_id = l.server_id \
         WHERE l.server_id = ?1 \
         ORDER BY t.sort_order ASC, t.created_at ASC",
    )
    .bind(&[text(server_id)])?
    .all()
    .await?
    .results()
}

pub async fn history(db: &D1Database, server_id: &str, hours: i64) -> Result<Vec<LatencySample>> {
    let hours = hours.clamp(1, 24 * 365);
    let cutoff = (Date::now().as_millis() as i64 / 1000) - hours * 3600;
    let task_count = db
        .prepare("SELECT COUNT(*) AS count FROM latency_task_servers WHERE server_id = ?1")
        .bind(&[text(server_id)])?
        .first::<i64>(Some("count"))
        .await?
        .unwrap_or(0);
    if task_count == 0 {
        return Ok(Vec::new());
    }
    let bucket = history_bucket_seconds(hours, task_count);
    let query = format!(
        "SELECT h.task_id, h.server_id, t.name, t.task_type, t.target, \
         (h.timestamp / {bucket}) * {bucket} AS timestamp, \
         CASE WHEN SUM(CASE WHEN h.latency_ms >= 0 THEN 1 ELSE 0 END) > 0 \
           THEN AVG(CASE WHEN h.latency_ms >= 0 THEN h.latency_ms END) ELSE -1 END AS latency_ms, \
         AVG(h.packet_loss) AS packet_loss \
         FROM latency_history h INNER JOIN latency_tasks t ON t.id = h.task_id \
         INNER JOIN latency_task_servers a \
           ON a.task_id = h.task_id AND a.server_id = h.server_id \
         WHERE h.server_id = ?1 AND h.timestamp >= ?2 \
         GROUP BY h.task_id, h.server_id, t.name, t.task_type, t.target, h.timestamp / {bucket} \
         ORDER BY timestamp ASC, t.sort_order ASC LIMIT {MAX_HISTORY_RESPONSE_ROWS}"
    );
    db.prepare(query)
        .bind(&[text(server_id), number(cutoff)])?
        .all()
        .await?
        .results()
}

fn history_bucket_seconds(hours: i64, task_count: i64) -> i64 {
    let hours = hours.clamp(1, 24 * 365);
    let task_count = task_count.clamp(1, MAX_LATENCY_TASKS as i64);
    let base = match hours {
        1 => 60,
        2..=4 => 120,
        5..=24 => 600,
        25..=168 => 3600,
        169..=720 => 14_400,
        _ => 86_400,
    };
    let points_per_task = (MAX_HISTORY_RESPONSE_ROWS / task_count).max(1);
    let bounded = (hours * 3600 + points_per_task - 1) / points_per_task;
    base.max(bounded)
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

#[cfg(test)]
mod tests {
    use super::{compact_results, history_bucket_seconds, MAX_HISTORY_ROWS_PER_REPORT};
    use crate::models::AgentLatencyResult;
    use std::collections::HashSet;

    #[test]
    fn compacts_latency_results_by_task_and_minute() {
        let assigned = HashSet::from(["task-a".to_string()]);
        let results = vec![
            AgentLatencyResult {
                task_id: "task-a".to_string(),
                timestamp: 121,
                latency_ms: 10.0,
                packet_loss: 0.0,
            },
            AgentLatencyResult {
                task_id: "task-a".to_string(),
                timestamp: 149,
                latency_ms: 20.0,
                packet_loss: 25.0,
            },
            AgentLatencyResult {
                task_id: "unassigned".to_string(),
                timestamp: 149,
                latency_ms: 1.0,
                packet_loss: 0.0,
            },
        ];
        let (latest, history) = compact_results(&results, &assigned, 150);
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].timestamp, 149);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].timestamp, 120);
        assert_eq!(history[0].latency_ms, 20.0);
    }

    #[test]
    fn bounds_compacted_latency_history() {
        let assigned = (0..128)
            .map(|index| format!("task-{index}"))
            .collect::<HashSet<_>>();
        let results = (0..40)
            .flat_map(|minute| {
                (0..128).map(move |task| AgentLatencyResult {
                    task_id: format!("task-{task}"),
                    timestamp: 1_000_000 + minute * 60,
                    latency_ms: task as f64,
                    packet_loss: 0.0,
                })
            })
            .collect::<Vec<_>>();
        let (_, history) = compact_results(&results, &assigned, results.last().unwrap().timestamp);
        assert_eq!(history.len(), MAX_HISTORY_ROWS_PER_REPORT);
        assert_eq!(
            history.last().unwrap().timestamp,
            results.last().unwrap().timestamp / 60 * 60
        );
    }

    #[test]
    fn bounds_latency_history_response_across_tasks() {
        assert_eq!(history_bucket_seconds(1, 1), 60);
        assert_eq!(history_bucket_seconds(24, 1), 600);
        assert!(history_bucket_seconds(1, 128) >= 117);
        assert!(24 * 3600 / history_bucket_seconds(24, 128) * 128 <= 4000);
        assert_eq!(history_bucket_seconds(720, 1), 14_400);
    }
}
