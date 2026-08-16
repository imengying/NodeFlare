use serde::{Deserialize, Serialize};
use worker::*;

use crate::db::AgentLiveContext;
use crate::models::{AgentLatencyResult, AgentReport};

const MAX_LIVE_SAMPLES: usize = 720;
const MAX_LIVE_LATENCY_RESULTS: usize = 4096;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentLiveBatch {
    #[serde(rename = "type")]
    message_type: String,
    samples: Vec<AgentReport>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TrafficAttachment {
    reset_day: i64,
    cycle_key: i64,
    raw_rx: i64,
    raw_tx: i64,
    used_rx: i64,
    used_tx: i64,
    rx_correction: i64,
    tx_correction: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct SocketAttachment {
    role: String,
    server_id: Option<String>,
    hidden: bool,
    report_interval: i64,
    collect_interval: i64,
    last_d1_write_at: i64,
    traffic: Option<TrafficAttachment>,
}

fn active_report_interval_ms(report_interval: i64, collect_interval: i64) -> i64 {
    let report_interval = report_interval.clamp(15, 3600);
    let collect_interval = collect_interval.clamp(2, 60).min(report_interval);
    ((report_interval + 14) / 15)
        .max(collect_interval)
        .clamp(2, 60)
        * 1000
}

fn apply_live_traffic(report: &mut AgentReport, traffic: &mut TrafficAttachment) {
    let cycle_key = crate::db::traffic_cycle_key(report.timestamp, traffic.reset_day);
    if cycle_key != traffic.cycle_key {
        traffic.cycle_key = cycle_key;
        traffic.raw_rx = report.net_rx_total;
        traffic.raw_tx = report.net_tx_total;
        traffic.used_rx = 0;
        traffic.used_tx = 0;
    } else {
        let rx_delta = if report.net_rx_total >= traffic.raw_rx {
            report.net_rx_total - traffic.raw_rx
        } else {
            report.net_rx_total
        };
        let tx_delta = if report.net_tx_total >= traffic.raw_tx {
            report.net_tx_total - traffic.raw_tx
        } else {
            report.net_tx_total
        };
        traffic.used_rx = traffic.used_rx.saturating_add(rx_delta);
        traffic.used_tx = traffic.used_tx.saturating_add(tx_delta);
        traffic.raw_rx = report.net_rx_total;
        traffic.raw_tx = report.net_tx_total;
    }
    report.net_rx_total = traffic.used_rx.saturating_add(traffic.rx_correction);
    report.net_tx_total = traffic.used_tx.saturating_add(traffic.tx_correction);
}

fn batch_update_payload(
    server_id: &str,
    reports: &[AgentReport],
    traffic: &mut TrafficAttachment,
) -> Result<String> {
    batch_update_payload_at(server_id, reports, traffic, crate::now())
}

fn batch_update_payload_at(
    server_id: &str,
    reports: &[AgentReport],
    traffic: &mut TrafficAttachment,
    envelope_timestamp: i64,
) -> Result<String> {
    let mut reports = reports.to_vec();
    reports.sort_by_key(|report| report.timestamp);
    let mut samples = Vec::with_capacity(reports.len());
    for mut report in reports {
        apply_live_traffic(&mut report, traffic);
        let timestamp = report.timestamp;
        let mut data = serde_json::to_value(report)?;
        if let Some(data) = data.as_object_mut() {
            data.remove("timestamp");
        }
        samples.push(serde_json::json!({ "ts": timestamp, "data": data }));
    }
    Ok(serde_json::to_string(&serde_json::json!({
        "type": "batchUpdate",
        "ts": envelope_timestamp,
        "updates": [{
            "serverId": server_id,
            "samples": samples
        }]
    }))?)
}

fn agent_attachment(
    server_id: String,
    hidden: bool,
    context: AgentLiveContext,
) -> SocketAttachment {
    SocketAttachment {
        role: "agent".to_string(),
        server_id: Some(server_id),
        hidden,
        report_interval: context.report_interval.clamp(15, 3600),
        collect_interval: context.collect_interval.clamp(2, 60),
        last_d1_write_at: context.last_persisted_at,
        traffic: Some(TrafficAttachment {
            reset_day: context.reset_day.clamp(1, 31),
            cycle_key: context.cycle_key,
            raw_rx: context.raw_rx,
            raw_tx: context.raw_tx,
            used_rx: context.used_rx,
            used_tx: context.used_tx,
            rx_correction: context.rx_correction,
            tx_correction: context.tx_correction,
        }),
    }
}

fn required_i64_header(req: &Request, name: &str) -> Result<i64> {
    req.headers()
        .get(name)?
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| Error::RustError(format!("missing or invalid {name}")))
}

fn live_context_from_headers(req: &Request) -> Result<AgentLiveContext> {
    Ok(AgentLiveContext {
        report_interval: required_i64_header(req, "X-Live-Report-Interval")?,
        collect_interval: required_i64_header(req, "X-Live-Collect-Interval")?,
        reset_day: required_i64_header(req, "X-Live-Reset-Day")?,
        cycle_key: required_i64_header(req, "X-Live-Cycle-Key")?,
        raw_rx: required_i64_header(req, "X-Live-Raw-Rx")?,
        raw_tx: required_i64_header(req, "X-Live-Raw-Tx")?,
        used_rx: required_i64_header(req, "X-Live-Used-Rx")?,
        used_tx: required_i64_header(req, "X-Live-Used-Tx")?,
        rx_correction: required_i64_header(req, "X-Live-Rx-Correction")?,
        tx_correction: required_i64_header(req, "X-Live-Tx-Correction")?,
        last_persisted_at: required_i64_header(req, "X-Live-Last-Persisted-At")?,
    })
}

fn dashboard_attachment() -> SocketAttachment {
    SocketAttachment {
        role: "dashboard".to_string(),
        server_id: None,
        hidden: false,
        report_interval: 0,
        collect_interval: 0,
        last_d1_write_at: 0,
        traffic: None,
    }
}

#[durable_object(websocket)]
pub struct LiveHub {
    state: State,
    env: Env,
}

impl DurableObject for LiveHub {
    fn new(state: State, env: Env) -> Self {
        if let Ok(pair) = WebSocketRequestResponsePair::new("ping", "pong") {
            state.set_websocket_auto_response(&pair);
        }
        Self { state, env }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if req.method() == Method::Post {
            if req.headers().get("X-Live-Action")?.as_deref() == Some("disconnect-agents") {
                let server_ids = req.json::<Vec<String>>().await?;
                for server_id in server_ids {
                    if server_id.is_empty() || server_id.len() > 80 || server_id.contains('/') {
                        continue;
                    }
                    for socket in self
                        .state
                        .get_websockets_with_tag(&format!("agent:{server_id}"))
                    {
                        let _ = socket.close(Some(1008), Some("server configuration changed"));
                    }
                }
                return Response::empty();
            }
            let payload = req.text().await?;
            let server_id = req.headers().get("X-Server-ID")?.unwrap_or_default();
            let mut sockets = self.state.get_websockets_with_tag("all");
            if !server_id.is_empty() {
                sockets.extend(
                    self.state
                        .get_websockets_with_tag(&format!("server:{server_id}")),
                );
            }
            for socket in sockets {
                if socket.send_with_str(&payload).is_err() {
                    let _ = socket.close(Some(1011), Some("send failed"));
                }
            }
            return Response::empty();
        }

        let is_upgrade = req
            .headers()
            .get("Upgrade")?
            .map(|value| value.eq_ignore_ascii_case("websocket"))
            .unwrap_or(false);
        if !is_upgrade {
            return Response::error("WebSocket upgrade required", 426);
        }

        let pair = WebSocketPair::new()?;
        if let Some(server_id) = req.headers().get("X-Live-Agent-ID")? {
            if server_id.is_empty() || server_id.len() > 80 || server_id.contains('/') {
                return Response::error("Invalid live Agent identity", 400);
            }
            let hidden = req
                .headers()
                .get("X-Live-Agent-Hidden")?
                .is_some_and(|value| value == "1");
            let context = match live_context_from_headers(&req) {
                Ok(context) => context,
                Err(_) => return Response::error("Invalid live Agent context", 400),
            };
            pair.server.serialize_attachment(agent_attachment(
                server_id.clone(),
                hidden,
                context,
            ))?;
            self.state
                .accept_websocket_with_tags(&pair.server, &[&format!("agent:{server_id}")]);
            return Response::from_websocket(pair.client);
        }
        let server_id = req
            .url()?
            .query_pairs()
            .find_map(|(key, value)| (key == "server_id").then(|| value.to_string()))
            .filter(|value| !value.is_empty() && value.len() <= 80 && !value.contains('/'));
        let tag = server_id.map_or_else(|| "all".to_string(), |id| format!("server:{id}"));
        pair.server.serialize_attachment(dashboard_attachment())?;
        self.state.accept_websocket_with_tags(&pair.server, &[&tag]);
        Response::from_websocket(pair.client)
    }

    async fn websocket_message(
        &self,
        ws: WebSocket,
        message: WebSocketIncomingMessage,
    ) -> Result<()> {
        let Some(mut attachment) = ws.deserialize_attachment::<SocketAttachment>()? else {
            return Ok(());
        };
        if attachment.role != "agent" {
            return Ok(());
        }
        let WebSocketIncomingMessage::String(message) = message else {
            return Ok(());
        };
        if message.len() > 1024 * 1024 {
            return ws.close(Some(1009), Some("live metric payload too large"));
        }
        let Ok(batch) = serde_json::from_str::<AgentLiveBatch>(&message) else {
            return ws.close(Some(1007), Some("invalid live metric payload"));
        };
        if batch.message_type != "update"
            || batch.samples.is_empty()
            || batch.samples.len() > MAX_LIVE_SAMPLES
        {
            return ws.close(Some(1007), Some("invalid live metric batch"));
        }
        let received_at = crate::now();
        let mut latency_results = Vec::<AgentLatencyResult>::new();
        for report in &batch.samples {
            if report.timestamp <= 0
                || (report.timestamp - received_at).abs() > 7200
                || crate::validate_report(report).is_some()
            {
                return ws.close(Some(1007), Some("invalid live metric sample"));
            }
            latency_results.extend(report.latency_results.iter().cloned());
        }
        if latency_results.len() > MAX_LIVE_LATENCY_RESULTS {
            return ws.close(Some(1009), Some("too many live latency results"));
        }
        let Some(server_id) = attachment.server_id.clone() else {
            return ws.close(Some(1008), Some("missing server identity"));
        };
        let mut traffic = attachment
            .traffic
            .take()
            .ok_or_else(|| Error::RustError("missing live traffic state".to_string()))?;
        let payload = batch_update_payload(&server_id, &batch.samples, &mut traffic)?;
        attachment.traffic = Some(traffic);

        let report_interval = attachment.report_interval.clamp(15, 3600);
        let due_for_d1 = received_at.saturating_sub(attachment.last_d1_write_at) >= report_interval;
        let mut persisted = false;
        if due_for_d1 {
            let database = self.env.d1("DB")?;
            if let Err(error) = crate::db::save_reports(&database, &server_id, &batch.samples).await
            {
                console_warn!("live D1 metrics write failed for {server_id}: {error:?}");
            } else if let Err(error) =
                crate::latency::save_results(&database, &server_id, &latency_results, received_at)
                    .await
            {
                console_warn!("live D1 latency write failed for {server_id}: {error:?}");
            } else {
                attachment.last_d1_write_at = received_at;
                persisted = true;
            }
        }

        if !attachment.hidden {
            let mut sockets = self.state.get_websockets_with_tag("all");
            sockets.extend(
                self.state
                    .get_websockets_with_tag(&format!("server:{server_id}")),
            );
            let has_dashboard = !sockets.is_empty();
            for dashboard in sockets {
                if dashboard.send_with_str(&payload).is_err() {
                    let _ = dashboard.close(Some(1011), Some("send failed"));
                }
            }
            let next_wss_ms = if has_dashboard {
                active_report_interval_ms(attachment.report_interval, attachment.collect_interval)
            } else {
                report_interval * 1000
            };
            let next_d1_ms = if persisted {
                report_interval * 1000
            } else {
                let due_at = attachment.last_d1_write_at.saturating_add(report_interval);
                due_at.saturating_sub(received_at).saturating_mul(1000)
            };
            ws.serialize_attachment(attachment)?;
            ws.send_with_str(&serde_json::to_string(&serde_json::json!({
                "type": "ack",
                "ts": received_at,
                "persisted": persisted,
                "nextD1WriteAfterMs": next_d1_ms,
                "nextWssReportAfterMs": next_wss_ms
            }))?)?;
        } else {
            let next_d1_ms = if persisted {
                report_interval * 1000
            } else {
                let due_at = attachment.last_d1_write_at.saturating_add(report_interval);
                due_at.saturating_sub(received_at).saturating_mul(1000)
            };
            ws.serialize_attachment(attachment)?;
            ws.send_with_str(&serde_json::to_string(&serde_json::json!({
                "type": "ack",
                "ts": received_at,
                "persisted": persisted,
                "nextD1WriteAfterMs": next_d1_ms,
                "nextWssReportAfterMs": report_interval * 1000
            }))?)?;
        }
        Ok(())
    }

    async fn websocket_close(
        &self,
        ws: WebSocket,
        code: usize,
        reason: String,
        _was_clean: bool,
    ) -> Result<()> {
        ws.close(Some(code as u16), Some(reason))
    }

    async fn websocket_error(&self, ws: WebSocket, _error: Error) -> Result<()> {
        ws.close(Some(1011), Some("socket error"))
    }
}

pub async fn broadcast(env: &Env, server_id: &str, payload: &str) -> Result<()> {
    let namespace = env.durable_object("LIVE_HUB")?;
    let stub = namespace.id_from_name("dashboard")?.get_stub()?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_body(Some(worker::wasm_bindgen::JsValue::from_str(payload)));
    let req = Request::new_with_init("https://live.internal/push", &init)?;
    req.headers().set("X-Server-ID", server_id)?;
    stub.fetch_with_request(req).await?;
    Ok(())
}

pub async fn disconnect_agents(env: &Env, server_ids: &[String]) -> Result<()> {
    if server_ids.is_empty() {
        return Ok(());
    }
    let namespace = env.durable_object("LIVE_HUB")?;
    let stub = namespace.id_from_name("dashboard")?.get_stub()?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_body(Some(worker::wasm_bindgen::JsValue::from_str(
            &serde_json::to_string(server_ids)?,
        )));
    let req = Request::new_with_init("https://live.internal/disconnect-agents", &init)?;
    req.headers().set("Content-Type", "application/json")?;
    req.headers().set("X-Live-Action", "disconnect-agents")?;
    stub.fetch_with_request(req).await?;
    Ok(())
}

pub async fn upgrade(req: Request, env: &Env) -> Result<Response> {
    let namespace = env.durable_object("LIVE_HUB")?;
    let stub = namespace.id_from_name("dashboard")?.get_stub()?;
    req.headers().delete("X-Live-Agent-ID")?;
    req.headers().delete("X-Live-Agent-Hidden")?;
    for header in [
        "X-Live-Report-Interval",
        "X-Live-Collect-Interval",
        "X-Live-Reset-Day",
        "X-Live-Cycle-Key",
        "X-Live-Raw-Rx",
        "X-Live-Raw-Tx",
        "X-Live-Used-Rx",
        "X-Live-Used-Tx",
        "X-Live-Rx-Correction",
        "X-Live-Tx-Correction",
        "X-Live-Last-Persisted-At",
    ] {
        req.headers().delete(header)?;
    }
    stub.fetch_with_request(req).await
}

pub async fn upgrade_agent(
    req: Request,
    env: &Env,
    server_id: &str,
    hidden: bool,
    context: AgentLiveContext,
) -> Result<Response> {
    let namespace = env.durable_object("LIVE_HUB")?;
    let stub = namespace.id_from_name("dashboard")?.get_stub()?;
    req.headers().set("X-Live-Agent-ID", server_id)?;
    req.headers()
        .set("X-Live-Agent-Hidden", if hidden { "1" } else { "0" })?;
    req.headers().set(
        "X-Live-Report-Interval",
        &context.report_interval.to_string(),
    )?;
    req.headers().set(
        "X-Live-Collect-Interval",
        &context.collect_interval.to_string(),
    )?;
    req.headers()
        .set("X-Live-Reset-Day", &context.reset_day.to_string())?;
    req.headers()
        .set("X-Live-Cycle-Key", &context.cycle_key.to_string())?;
    req.headers()
        .set("X-Live-Raw-Rx", &context.raw_rx.to_string())?;
    req.headers()
        .set("X-Live-Raw-Tx", &context.raw_tx.to_string())?;
    req.headers()
        .set("X-Live-Used-Rx", &context.used_rx.to_string())?;
    req.headers()
        .set("X-Live-Used-Tx", &context.used_tx.to_string())?;
    req.headers()
        .set("X-Live-Rx-Correction", &context.rx_correction.to_string())?;
    req.headers()
        .set("X-Live-Tx-Correction", &context.tx_correction.to_string())?;
    req.headers().set(
        "X-Live-Last-Persisted-At",
        &context.last_persisted_at.to_string(),
    )?;
    req.headers().delete("Authorization")?;
    stub.fetch_with_request(req).await
}

#[cfg(test)]
mod tests {
    use super::{active_report_interval_ms, batch_update_payload_at, TrafficAttachment};
    use crate::models::AgentReport;

    #[test]
    fn live_payload_contains_cycle_adjusted_traffic_and_latency() {
        let report = AgentReport {
            timestamp: 1_234,
            cpu: 12.5,
            load1: 0.5,
            load5: 0.4,
            load15: 0.3,
            mem_used: 1,
            mem_total: 2,
            swap_used: 0,
            swap_total: 0,
            disk_used: 3,
            disk_total: 4,
            net_in: 100.0,
            net_out: 200.0,
            net_rx_total: 300,
            net_tx_total: 400,
            uptime: 500,
            processes: 10,
            tcp_connections: 2,
            udp_connections: 1,
            cpu_cores: 2,
            cpu_model: "CPU".to_string(),
            os: "Linux".to_string(),
            kernel: "6.0".to_string(),
            arch: "x86_64".to_string(),
            virtualization: "kvm".to_string(),
            gpu_usage: 0.0,
            gpu_model: String::new(),
            agent_version: "test".to_string(),
            disk_read_bps: 0.0,
            disk_write_bps: 0.0,
            disk_read_iops: 0.0,
            disk_write_iops: 0.0,
            disk_await_ms: 0.0,
            disk_utilization: 0.0,
            disks: vec![],
            gpus: vec![],
            latency_results: vec![],
        };
        let mut traffic = TrafficAttachment {
            reset_day: 1,
            cycle_key: crate::db::traffic_cycle_key(1_234, 1),
            raw_rx: 100,
            raw_tx: 100,
            used_rx: 500,
            used_tx: 700,
            rx_correction: 10,
            tx_correction: 20,
        };
        let payload: serde_json::Value = serde_json::from_str(
            &batch_update_payload_at("node-a", &[report], &mut traffic, 1_235).unwrap(),
        )
        .unwrap();
        assert_eq!(payload["type"], "batchUpdate");
        assert_eq!(payload["updates"][0]["serverId"], "node-a");
        assert_eq!(payload["updates"][0]["samples"][0]["ts"], 1_234);
        assert_eq!(payload["updates"][0]["samples"][0]["data"]["cpu"], 12.5);
        assert_eq!(
            payload["updates"][0]["samples"][0]["data"]["net_rx_total"],
            710
        );
        assert!(payload["updates"][0]["samples"][0]["data"]
            .get("timestamp")
            .is_none());
        assert_eq!(active_report_interval_ms(60, 5), 5_000);
    }
}
