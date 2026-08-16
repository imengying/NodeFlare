use std::net::IpAddr;

use worker::{Context, D1Database, Env, Request, Response, Result};

use crate::db;
use crate::live;
use crate::latency;
use crate::{
    client_ip, error, no_content, now, public_server, request_json, validate_report,
    AGENT_JSON_MAX_BYTES, MAX_AGENT_LATENCY_RESULTS, MAX_AGENT_SAMPLES,
};
use crate::auth::bearer_token;
use crate::models::AgentReportBatch;
use crate::sha256_hex;

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
    let latency_results = batch
        .samples
        .iter()
        .flat_map(|report| report.latency_results.iter())
        .cloned()
        .collect::<Vec<_>>();
    if latency_results.len() > MAX_AGENT_LATENCY_RESULTS {
        return error("每批最多包含 4096 条延迟结果", 400);
    }
    db::save_reports(database, &server_id, &batch.samples).await?;
    latency::save_results(database, &server_id, &latency_results, received_at).await?;

    let public_dashboard = db::get_setting(database, "public_dashboard")
        .await?
        .is_none_or(|value| value == "true");
    if identity.hidden == 0 && public_dashboard {
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
    let Some(config) = db::agent_config(database, &server_id).await? else {
        return error("节点不存在", 404);
    };
    let config_json = serde_json::to_string(&config)?;
    let config_hash = sha256_hex(&config_json);
    if agent_config_hash == config_hash {
        let mut response = no_content()?;
        response
            .headers_mut()
            .set("X-Agent-Config-Sha256", &config_hash)?;
        response
            .headers_mut()
            .set("X-NodeFlare-Server-Time", &received_at.to_string())?;
        response.headers_mut().set("Cache-Control", "no-store")?;
        return Ok(response);
    }
    let mut response = crate::json(&config, 202)?;
    response
        .headers_mut()
        .set("X-Agent-Config-Sha256", &config_hash)?;
    response
        .headers_mut()
        .set("X-NodeFlare-Server-Time", &received_at.to_string())?;
    Ok(response)
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
    let Some(context) = db::agent_live_context(database, &identity.id).await? else {
        return error("节点不存在", 404);
    };
    live::upgrade_agent(req, env, &identity.id, identity.hidden != 0, context).await
}
