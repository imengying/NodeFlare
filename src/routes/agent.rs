use std::net::IpAddr;

use worker::{Context, D1Database, Env, Request, Response, Result};

use crate::auth::bearer_token;
use crate::db;
use crate::latency;
use crate::live;
use crate::models::AgentReportBatch;
use crate::sha256_hex;
use crate::{
    client_ip, error, no_content, now, public_server, request_json, validate_report,
    AGENT_JSON_MAX_BYTES, MAX_AGENT_LATENCY_RESULTS, MAX_AGENT_SAMPLES,
};

async fn config_response(
    database: &D1Database,
    server_id: &str,
    agent_config_hash: &str,
    received_at: i64,
    changed_status: u16,
) -> Result<Response> {
    let Some(config) = db::agent_config(database, server_id).await? else {
        return error("节点不存在", 404);
    };
    let config_json = serde_json::to_string(&config)?;
    let config_hash = sha256_hex(&config_json);
    let mut response = if agent_config_hash == config_hash {
        no_content()?
    } else {
        crate::json(&config, changed_status)?
    };
    response
        .headers_mut()
        .set("X-Agent-Config-Sha256", &config_hash)?;
    response
        .headers_mut()
        .set("X-NodeFlare-Server-Time", &received_at.to_string())?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    Ok(response)
}

pub(crate) async fn report(
    mut req: Request,
    env: Env,
    ctx: Context,
    database: &D1Database,
) -> Result<Response> {
    let agent_config_hash = req
        .headers()
        .get("X-Agent-Config-Sha256")?
        .unwrap_or_default();
    let token = match bearer_token(&req) {
        Some(value) => value,
        None => return error("缺少 Agent Token", 401),
    };
    let batch: AgentReportBatch = match request_json(&mut req, AGENT_JSON_MAX_BYTES).await {
        Ok(value) => value,
        Err(_) => return error("指标格式无效", 400),
    };
    if batch.samples.is_empty() || batch.samples.len() > MAX_AGENT_SAMPLES {
        return error("每批应包含 1 至 720 个样本", 400);
    }
    let received_at = now();
    for report in &batch.samples {
        if report.timestamp <= 0 || (report.timestamp - received_at).abs() > 7200 {
            return error("样本时间戳超出允许范围", 400);
        }
        if let Some(message) = validate_report(report) {
            return error(message, 400);
        }
    }
    let Some(identity) = db::get_agent_identity(database, &token).await? else {
        return error("Agent Token 无效", 401);
    };
    let server_id = identity.id;
    if let Some(ip) = client_ip(&req).and_then(|value| value.parse::<IpAddr>().ok()) {
        db::update_last_ip(database, &server_id, &ip.to_string()).await?;
    }
    let latency_result_count = batch
        .samples
        .iter()
        .flat_map(|report| report.latency_results.iter())
        .count();
    if latency_result_count > MAX_AGENT_LATENCY_RESULTS {
        return error("每批最多包含 4096 条延迟结果", 400);
    }
    let mut history_aggregate = db::HistoryMetricAggregate::default();
    history_aggregate.extend(&batch.samples);
    let alert_point = history_aggregate.point();
    db::save_reports(database, &server_id, &batch.samples).await?;
    if let Some(point) = alert_point.as_ref() {
        if let Err(error) = live::record_alert_sample(&env, &server_id, point).await {
            worker::console_warn!("resource alert sample write failed: {error:?}");
        }
    }

    if identity.hidden == 0 {
        let server = db::get_server(database, &server_id, false).await?;
        let samples = latency::latest_for_server(database, &server_id).await?;
        let payload = server
            .map(|server| public_server(server, samples))
            .map(|server| {
                serde_json::to_string(&serde_json::json!({
                    "type": "server",
                    "server": server
                }))
            })
            .transpose()?;
        let broadcast_server_id = server_id.clone();
        if let Some(payload) = payload {
            ctx.wait_until(async move {
                let _ = live::broadcast(&env, &broadcast_server_id, &payload).await;
            });
        }
    }
    config_response(database, &server_id, &agent_config_hash, received_at, 202).await
}

pub(crate) async fn config(req: &Request, database: &D1Database) -> Result<Response> {
    let agent_config_hash = req
        .headers()
        .get("X-Agent-Config-Sha256")?
        .unwrap_or_default();
    let token = match bearer_token(req) {
        Some(value) => value,
        None => return error("缺少 Agent Token", 401),
    };
    let Some(identity) = db::get_agent_identity(database, &token).await? else {
        return error("Agent Token 无效", 401);
    };
    config_response(database, &identity.id, &agent_config_hash, now(), 200).await
}

pub(crate) async fn live_websocket(
    req: Request,
    env: &Env,
    database: &D1Database,
) -> Result<Response> {
    let token = match bearer_token(&req) {
        Some(value) => value,
        None => return error("缺少 Agent Token", 401),
    };
    let Some(identity) = db::get_agent_identity(database, &token).await? else {
        return error("Agent Token 无效", 401);
    };
    if let Some(ip) = client_ip(&req).and_then(|value| value.parse::<IpAddr>().ok()) {
        db::update_last_ip(database, &identity.id, &ip.to_string()).await?;
    }
    let Some(context) = db::agent_live_context(database, &identity.id).await? else {
        return error("节点不存在", 404);
    };
    live::upgrade_agent(req, env, &identity.id, identity.hidden != 0, context).await
}
