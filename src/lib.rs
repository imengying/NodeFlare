mod auth;
mod cloudflare;
mod db;
mod exchange;
mod latency;
mod live;
mod models;
mod notify;
mod theme;
mod turnstile;

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use worker::*;

use crate::auth::{
    bearer_token, create_session, create_theme_preview, create_turnstile_proof, hash_password,
    is_admin, sha256_hex, verify_credentials, verify_theme_preview, verify_turnstile_proof,
};
use crate::models::{
    AgentReport, AgentReportBatch, AlertRuleInput, ApiError, HistoryPoint, LatencyTaskInput,
    LoginRequest, ServerBatchInput, ServerInput, ServerOrderInput, ServerView, SettingsInput,
    ThemePreviewInput, TurnstileVerifyRequest,
};

const AGENT_SCRIPT: &str = include_str!("../agent/agent.sh");
const AGENT_WINDOWS_SCRIPT: &str = include_str!("../agent/install.ps1");
const AGENT_MACOS_SCRIPT: &str = include_str!("../agent/install-macos.sh");
const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const ADMIN_HTML: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin.html"));
const ADMIN_SCRIPT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin.js"));
const ADMIN_STYLE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin.css"));

#[derive(Serialize)]
struct Success {
    success: bool,
}

fn now() -> i64 {
    Date::now().as_millis() as i64 / 1000
}

fn env_text(env: &Env, key: &str, fallback: &str) -> String {
    env.var(key)
        .map(|v| v.to_string())
        .unwrap_or_else(|_| fallback.to_string())
}

fn env_number(env: &Env, key: &str, fallback: i64) -> i64 {
    env_text(env, key, &fallback.to_string())
        .parse()
        .unwrap_or(fallback)
}

fn json<T: Serialize>(value: &T, status: u16) -> Result<Response> {
    let mut response = Response::from_json(value)?.with_status(status);
    let headers = response.headers_mut();
    headers.set("Cache-Control", "no-store")?;
    headers.set("X-Content-Type-Options", "nosniff")?;
    headers.set("Referrer-Policy", "no-referrer")?;
    headers.set(
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=()",
    )?;
    Ok(response)
}

fn error(message: &str, status: u16) -> Result<Response> {
    json(&ApiError { error: message }, status)
}

fn mutable_response(response: Response) -> Result<Response> {
    let status = response.status_code();
    let headers = Headers::new();
    for (name, value) in response.headers().entries() {
        headers.append(&name, &value)?;
    }
    let (_, body) = response.into_parts();
    Ok(Response::from_body(body)?
        .with_status(status)
        .with_headers(headers))
}

fn embedded_admin_response(body: &[u8], content_type: &str) -> Result<Response> {
    let mut response = Response::from_bytes(body.to_vec())?;
    let headers = response.headers_mut();
    headers.set("Content-Type", content_type)?;
    headers.set("Cache-Control", "no-store")?;
    headers.set("X-Content-Type-Options", "nosniff")?;
    headers.set("X-Frame-Options", "DENY")?;
    headers.set("Referrer-Policy", "strict-origin-when-cross-origin")?;
    headers.set(
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=()",
    )?;
    headers.set(
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' https://challenges.cloudflare.com; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; connect-src 'self'; frame-src https://challenges.cloudflare.com; font-src 'self' data:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
    )?;
    Ok(response)
}

fn valid_ping_target(value: &str) -> bool {
    let raw = value.trim();
    if raw.is_empty() {
        return true;
    }
    if raw.len() > 60
        || raw.contains("://")
        || raw
            .chars()
            .any(|character| character.is_whitespace() || "/@?#\\[]".contains(character))
        || raw.matches(':').count() > 1
    {
        return false;
    }

    let (host, port) = raw
        .split_once(':')
        .map_or((raw, None), |(host, port)| (host, Some(port)));
    if let Some(port) = port {
        if port.is_empty()
            || port.len() > 5
            || !port.chars().all(|character| character.is_ascii_digit())
            || port.parse::<u16>().ok().is_none_or(|port| port == 0)
        {
            return false;
        }
    }
    if host.is_empty() || host.len() > 50 || host.starts_with('.') || host.ends_with('.') {
        return false;
    }

    let labels = host.split('.').collect::<Vec<_>>();
    let ipv4_like = labels.len() == 4
        && labels
            .iter()
            .all(|label| !label.is_empty() && label.chars().all(|c| c.is_ascii_digit()));
    if ipv4_like {
        return labels.iter().all(|label| label.parse::<u8>().is_ok());
    }
    labels.iter().all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
            && label
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
            && label
                .chars()
                .last()
                .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

fn valid_cloudflare_account_id(value: &str) -> bool {
    let value = value.trim();
    value.is_empty()
        || (value.len() == 32 && value.chars().all(|character| character.is_ascii_hexdigit()))
}

fn valid_cloudflare_api_token(value: &str) -> bool {
    let value = value.trim();
    value.chars().count() <= 512 && !value.chars().any(char::is_whitespace)
}

fn configured_origins(value: &str) -> Option<Vec<String>> {
    let mut origins = Vec::new();
    for raw in value.split(|character: char| character == ',' || character.is_whitespace()) {
        let raw = raw.trim().trim_end_matches('/');
        if raw.is_empty() {
            continue;
        }
        if raw == "*" {
            origins.push(raw.to_string());
            continue;
        }
        let Ok(url) = Url::parse(raw) else {
            return None;
        };
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        origins.push(url.origin().ascii_serialization());
    }
    origins.sort();
    origins.dedup();
    Some(origins)
}

fn origin_allowed(origin: &str, request_url: &Url, configured: &str) -> bool {
    let normalized = origin.trim().trim_end_matches('/');
    normalized == request_url.origin().ascii_serialization()
        || configured_origins(configured).is_some_and(|origins| {
            origins
                .iter()
                .any(|allowed| allowed == "*" || allowed == normalized)
        })
}

fn valid_federation_sites(value: &serde_json::Value) -> bool {
    let Some(sites) = value.as_array() else {
        return false;
    };
    sites.len() <= 16
        && sites.iter().all(|site| {
            let Some(object) = site.as_object() else {
                return false;
            };
            let name = object
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            let url = object
                .get("url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .trim_end_matches('/');
            !name.is_empty()
                && name.chars().count() <= 80
                && Url::parse(url).ok().is_some_and(|parsed| {
                    parsed.scheme() == "https"
                        && parsed.host_str().is_some()
                        && parsed.path() == "/"
                        && parsed.query().is_none()
                        && parsed.fragment().is_none()
                })
        })
}

fn csp_sources(value: &str) -> String {
    configured_origins(value)
        .unwrap_or_default()
        .into_iter()
        .filter(|origin| origin != "*")
        .collect::<Vec<_>>()
        .join(" ")
}

fn validate_latency_task(input: &LatencyTaskInput) -> Option<&'static str> {
    let name_len = input.name.trim().chars().count();
    if !(1..=80).contains(&name_len) {
        return Some("任务名称长度应为 1 至 80 个字符");
    }
    if !matches!(input.task_type.as_str(), "tcp" | "icmp") {
        return Some("延迟类型仅支持 TCP 或 ICMP");
    }
    let target = input.target.trim();
    if target.is_empty() || !valid_ping_target(target) {
        return Some("目标应为域名、IPv4 或 TCP host:port");
    }
    if input.task_type == "icmp" && target.contains(':') {
        return Some("ICMP 目标不能包含端口");
    }
    if !(30..=3600).contains(&input.interval_seconds) {
        return Some("延迟测试间隔应为 30 至 3600 秒");
    }
    if input.server_ids.len() > 500
        || input
            .server_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 80)
        || input.server_ids.iter().collect::<HashSet<_>>().len() != input.server_ids.len()
    {
        return Some("服务器选择列表无效");
    }
    if !input.default_enabled && input.server_ids.is_empty() {
        return Some("请至少选择一个服务器，或开启默认分配");
    }
    None
}

fn validate_alert_rule(input: &AlertRuleInput) -> Option<&'static str> {
    if !(1..=80).contains(&input.name.trim().chars().count()) {
        return Some("规则名称长度应为 1 至 80 个字符");
    }
    if !matches!(
        input.metric.as_str(),
        "cpu" | "memory" | "disk" | "net_in" | "net_out"
    ) {
        return Some("告警指标无效");
    }
    let maximum = if matches!(input.metric.as_str(), "cpu" | "memory" | "disk") {
        100.0
    } else {
        1_000_000.0
    };
    if !input.threshold.is_finite() || input.threshold <= 0.0 || input.threshold > maximum {
        return Some("告警阈值超出允许范围");
    }
    if !(1..=1440).contains(&input.duration_minutes) {
        return Some("告警时间窗口应为 1 至 1440 分钟");
    }
    if !matches!(input.aggregation.as_str(), "average" | "continuous") {
        return Some("告警判断方式无效");
    }
    if input.server_ids.len() > 500
        || input
            .server_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 80)
        || input.server_ids.iter().collect::<HashSet<_>>().len() != input.server_ids.len()
    {
        return Some("告警服务器列表无效");
    }
    None
}

fn validate_server(input: &ServerInput) -> Option<&'static str> {
    let name_len = input.name.trim().chars().count();
    if !(1..=80).contains(&name_len) {
        return Some("节点名称长度应为 1 至 80 个字符");
    }
    if input.region.chars().count() > 16 || input.group_name.chars().count() > 40 {
        return Some("地区或分组字段过长");
    }
    if input.tags.chars().count() > 240
        || input.note.chars().count() > 1000
        || input.public_remark.chars().count() > 1000
    {
        return Some("标签或备注字段过长");
    }
    if input.traffic_limit < 0 {
        return Some("流量限额不能为负数");
    }
    if !matches!(
        input.traffic_limit_type.as_str(),
        "sum" | "max" | "min" | "up" | "down"
    ) {
        return Some("流量计算方式无效");
    }
    if !input.price.is_finite() || !(-2.0..=1_000_000_000.0).contains(&input.price) {
        return Some("价格无效");
    }
    if !(1..=3650).contains(&input.billing_cycle) {
        return Some("计费周期应为 1 至 3650 天");
    }
    if input.currency.len() != 3
        || !input
            .currency
            .chars()
            .all(|value| value.is_ascii_alphabetic())
    {
        return Some("币种应为 3 位字母代码");
    }
    if !(1..=31).contains(&input.reset_day) {
        return Some("流量重置日应为 1 至 31");
    }
    if !(15..=3600).contains(&input.report_interval) {
        return Some("上报间隔应为 15 至 3600 秒");
    }
    if !(2..=60).contains(&input.collect_interval) || input.collect_interval > input.report_interval
    {
        return Some("采样间隔应为 2 至 60 秒且不能大于上报间隔");
    }
    if input.network_interface.chars().count() > 160 {
        return Some("统计网卡配置字段过长");
    }
    if input.rx_correction < 0 || input.tx_correction < 0 {
        return Some("流量修正值不能为负数");
    }
    None
}

fn validate_report(report: &AgentReport) -> Option<&'static str> {
    let floats = [
        report.cpu,
        report.load1,
        report.load5,
        report.load15,
        report.net_in,
        report.net_out,
        report.gpu_usage,
        report.disk_read_bps,
        report.disk_write_bps,
        report.disk_read_iops,
        report.disk_write_iops,
        report.disk_await_ms,
        report.disk_utilization,
    ];
    if floats.iter().any(|value| !value.is_finite()) {
        return Some("指标包含非法数值");
    }
    if !(0.0..=100.0).contains(&report.cpu) || !(0.0..=100.0).contains(&report.gpu_usage) {
        return Some("CPU 或 GPU 使用率超出范围");
    }
    let counters = [
        report.mem_used,
        report.mem_total,
        report.swap_used,
        report.swap_total,
        report.disk_used,
        report.disk_total,
        report.net_rx_total,
        report.net_tx_total,
        report.uptime,
        report.processes,
        report.tcp_connections,
        report.udp_connections,
        report.cpu_cores,
    ];
    if counters.iter().any(|value| *value < 0) {
        return Some("计数指标不能为负数");
    }
    if report.message.chars().count() > 500 || report.gpu_model.chars().count() > 240 {
        return Some("探针文本字段过长");
    }
    if report.agent_version.chars().count() > 80 {
        return Some("探针版本字段过长");
    }
    if report.disks.len() > 64
        || report.gpus.len() > 32
        || report.disks.iter().any(|disk| {
            disk.name.chars().count() > 120
                || disk.mount_point.chars().count() > 240
                || disk.used < 0
                || disk.total < 0
                || ![
                    disk.read_bps,
                    disk.write_bps,
                    disk.read_iops,
                    disk.write_iops,
                    disk.await_ms,
                    disk.utilization,
                ]
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0)
        })
        || report.gpus.iter().any(|gpu| {
            gpu.model.chars().count() > 240
                || gpu.memory_used < 0
                || gpu.memory_total < 0
                || !gpu.usage.is_finite()
                || !(0.0..=100.0).contains(&gpu.usage)
        })
    {
        return Some("磁盘或 GPU 明细无效");
    }
    if report.latency_results.len() > 128
        || report
            .latency_results
            .iter()
            .map(|result| &result.task_id)
            .collect::<HashSet<_>>()
            .len()
            != report.latency_results.len()
    {
        return Some("延迟结果列表无效");
    }
    if report.latency_results.iter().any(|result| {
        result.task_id.is_empty()
            || result.task_id.len() > 80
            || result.timestamp <= 0
            || !result.latency_ms.is_finite()
            || !result.packet_loss.is_finite()
            || !(-1.0..=100_000.0).contains(&result.latency_ms)
            || !(0.0..=100.0).contains(&result.packet_loss)
    }) {
        return Some("延迟结果数值无效");
    }
    None
}

fn public_server(mut server: ServerView) -> ServerView {
    server.note.clear();
    server.network_interface.clear();
    server.auto_update = 0;
    server
}

fn server_json(
    server: ServerView,
    offline_threshold: i64,
    latency: Vec<latency::LatencySample>,
) -> serde_json::Value {
    let timestamp = server.timestamp.unwrap_or(0);
    let uptime = server.uptime.unwrap_or(0);
    let expires_at = server
        .expires_at
        .map(|value| cloudflare::date_from_unix_days(value.div_euclid(86_400)))
        .unwrap_or_default();
    let traffic_limit = if server.traffic_limit > 0 {
        format!("{:.3}GB", server.traffic_limit as f64 / 1024_f64.powi(3))
    } else {
        String::new()
    };
    let mib = |value: Option<i64>| value.unwrap_or(0) as f64 / 1024_f64.powi(2);
    let mut value = serde_json::to_value(&server).unwrap_or_else(|_| serde_json::json!({}));
    let disks = server
        .disk_info
        .as_deref()
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    let gpus = server
        .gpu_info
        .as_deref()
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        .unwrap_or_else(|| serde_json::json!([]));
    if let Some(object) = value.as_object_mut() {
        object.extend(
            serde_json::json!({
                "server_group": server.group_name,
                "is_hidden": server.hidden,
                "is_online": timestamp > 0 && now() - timestamp <= offline_threshold,
                "last_updated": timestamp * 1000,
                "report_timestamp": timestamp * 1000,
                "boot_time": (timestamp - uptime.max(0)) * 1000,
                "cpu_info": server.cpu_model.clone().unwrap_or_default(),
                "kernel_version": server.kernel.clone().unwrap_or_default(),
                "gpu": server.gpu_usage.unwrap_or(0.0),
                "gpu_info": gpus.clone(),
                "disks": disks,
                "gpus": gpus,
                "ip_v4": server.ipv4.clone().unwrap_or_default(),
                "ip_v6": server.ipv6.clone().unwrap_or_default(),
                "ram_used": mib(server.mem_used),
                "ram_total": mib(server.mem_total),
                "swap_used": mib(server.swap_used),
                "swap_total": mib(server.swap_total),
                "disk_used": mib(server.disk_used),
                "disk_total": mib(server.disk_total),
                "net_in_speed": server.net_in.unwrap_or(0.0),
                "net_out_speed": server.net_out.unwrap_or(0.0),
                "net_rx": server.net_rx_total.unwrap_or(0),
                "net_tx": server.net_tx_total.unwrap_or(0),
                "tcp_conn": server.tcp_connections.unwrap_or(0),
                "udp_conn": server.udp_connections.unwrap_or(0),
                "load_avg": format!("{} {} {}", server.load1.unwrap_or(0.0), server.load5.unwrap_or(0.0), server.load15.unwrap_or(0.0)),
                "expire_date": expires_at,
                "expired_at": expires_at,
                "traffic_limit": traffic_limit,
                "traffic_calc_type": server.traffic_limit_type,
                "interface": server.network_interface,
                "latency": latency,
            })
            .as_object()
            .cloned()
            .unwrap_or_default(),
        );
    }
    value
}

fn history_json(point: HistoryPoint) -> serde_json::Value {
    let mib = |value: i64| value as f64 / 1024_f64.powi(2);
    serde_json::json!({
        "timestamp": point.timestamp * 1000,
        "cpu": point.cpu,
        "load_avg": format!("{} {} {}", point.load1, point.load5, point.load15),
        "ram_used": mib(point.mem_used),
        "ram_total": mib(point.mem_total),
        "swap_used": mib(point.swap_used),
        "swap_total": mib(point.swap_total),
        "disk_used": mib(point.disk_used),
        "disk_total": mib(point.disk_total),
        "net_in_speed": point.net_in,
        "net_out_speed": point.net_out,
        "net_rx": point.net_rx_total,
        "net_tx": point.net_tx_total,
        "processes": point.processes,
        "tcp_conn": point.tcp_connections,
        "udp_conn": point.udp_connections,
        "gpu": point.gpu_usage,
        "disk_read_bps": point.disk_read_bps,
        "disk_write_bps": point.disk_write_bps,
        "disk_read_iops": point.disk_read_iops,
        "disk_write_iops": point.disk_write_iops,
        "disk_await_ms": point.disk_await_ms,
        "disk_utilization": point.disk_utilization,
    })
}

fn request_cookie(req: &Request, name: &str) -> Option<String> {
    let raw = req.headers().get("Cookie").ok().flatten()?;
    raw.split(';').find_map(|entry| {
        let (key, value) = entry.trim().split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

fn query_value(req: &Request, name: &str) -> Option<String> {
    req.url()
        .ok()?
        .query_pairs()
        .find_map(|(key, value)| (key == name).then(|| value.to_string()))
}

fn theme_html_response(
    html: String,
    preview: Option<(&str, &str)>,
    extra_sources: &str,
) -> Result<Response> {
    let mut response = Response::ok(html)?;
    response
        .headers_mut()
        .set("Content-Type", "text/html; charset=utf-8")?;
    response
        .headers_mut()
        .set("X-Content-Type-Options", "nosniff")?;
    response.headers_mut().set("X-Frame-Options", "DENY")?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    response.headers_mut().set("Content-Security-Policy", &format!(
        "default-src 'self'; script-src 'self' 'unsafe-inline' https://challenges.cloudflare.com {extra_sources}; style-src 'self' 'unsafe-inline' {extra_sources}; img-src 'self' data: https:; connect-src 'self' ws: wss: {extra_sources}; frame-src https://challenges.cloudflare.com; font-src 'self' data: {extra_sources}; object-src 'none'; base-uri 'self'; frame-ancestors 'none'"
    ))?;
    if let Some((theme_url, proof)) = preview {
        response.headers_mut().append(
            "Set-Cookie",
            &format!("cf_monitor_theme_preview={theme_url}; Path=/; Max-Age=600; HttpOnly; SameSite=Strict"),
        )?;
        response.headers_mut().append(
            "Set-Cookie",
            &format!(
                "cf_monitor_theme_proof={proof}; Path=/; Max-Age=600; HttpOnly; SameSite=Strict"
            ),
        )?;
    }
    Ok(response)
}

fn preview_theme(req: &Request, env: &Env, password_hash: &str) -> Option<(String, String)> {
    let query_theme = query_value(req, "theme_url");
    let query_proof = query_value(req, "theme_proof");
    if let (Some(theme_url), Some(proof)) = (query_theme, query_proof) {
        let normalized = theme::parse_theme_url(&theme_url)?.normalized;
        return verify_theme_preview(&proof, env, password_hash, &normalized)
            .then_some((normalized, proof));
    }
    let theme_url = request_cookie(req, "cf_monitor_theme_preview")?;
    let proof = request_cookie(req, "cf_monitor_theme_proof")?;
    let normalized = theme::parse_theme_url(&theme_url)?.normalized;
    verify_theme_preview(&proof, env, password_hash, &normalized).then_some((normalized, proof))
}

fn client_ip(req: &Request) -> Option<String> {
    req.headers()
        .get("CF-Connecting-IP")
        .ok()
        .flatten()
        .or_else(|| req.headers().get("X-Forwarded-For").ok().flatten())
        .map(|value| value.split(',').next().unwrap_or("").trim().to_string())
        .filter(|value| !value.is_empty())
}

fn request_turnstile_proof(req: &Request) -> Option<String> {
    req.headers()
        .get("X-Turnstile-Verified")
        .ok()
        .flatten()
        .or_else(|| request_cookie(req, "cf_monitor_turnstile"))
        .or_else(|| {
            req.url()
                .ok()?
                .query_pairs()
                .find_map(|(key, value)| (key == "turnstile_verified").then(|| value.to_string()))
        })
}

fn server_id(path: &str, prefix: &str) -> Option<String> {
    let id = path.strip_prefix(prefix)?.trim_matches('/');
    if id.is_empty() || id.contains('/') || id.len() > 80 {
        None
    } else {
        Some(id.to_string())
    }
}

async fn handle(mut req: Request, env: Env, ctx: Context) -> Result<Response> {
    let method = req.method();
    let path = req.path();

    if method == Method::Get {
        match path.as_str() {
            "/admin" | "/admin/" | "/admin/index.html" => {
                return embedded_admin_response(ADMIN_HTML, "text/html; charset=utf-8")
            }
            "/admin-assets/admin.js" => {
                return embedded_admin_response(
                    ADMIN_SCRIPT,
                    "application/javascript; charset=utf-8",
                )
            }
            "/admin-assets/admin.css" => {
                return embedded_admin_response(ADMIN_STYLE, "text/css; charset=utf-8")
            }
            _ => {}
        }
    }

    if method == Method::Get && path == "/agent.sh" {
        let mut response = Response::ok(AGENT_SCRIPT)?;
        response
            .headers_mut()
            .set("Content-Type", "text/x-shellscript; charset=utf-8")?;
        response
            .headers_mut()
            .set("Cache-Control", "public, max-age=3600")?;
        return Ok(response);
    }
    if method == Method::Get && path == "/agent.ps1" {
        let mut response = Response::ok(AGENT_WINDOWS_SCRIPT)?;
        response
            .headers_mut()
            .set("Content-Type", "text/plain; charset=utf-8")?;
        response
            .headers_mut()
            .set("Cache-Control", "public, max-age=3600")?;
        return Ok(response);
    }
    if method == Method::Get && path == "/agent-macos.sh" {
        let mut response = Response::ok(AGENT_MACOS_SCRIPT)?;
        response
            .headers_mut()
            .set("Content-Type", "text/x-shellscript; charset=utf-8")?;
        response
            .headers_mut()
            .set("Cache-Control", "public, max-age=3600")?;
        return Ok(response);
    }

    let database = env.d1("DB")?;

    let default_name = env_text(&env, "SITE_NAME", "CF Monitor");
    let default_threshold = env_number(&env, "OFFLINE_THRESHOLD_SECONDS", 180).clamp(30, 3600);
    let default_username = env_text(&env, "ADMIN_USERNAME", "admin");
    let settings = db::settings(
        &database,
        &default_name,
        default_threshold,
        &default_username,
    )
    .await?;

    if method == Method::Get && path == "/api/config" {
        let authorized = is_admin(&req, &env, &settings.admin_password_hash);
        let existing_proof = request_turnstile_proof(&req)
            .filter(|value| verify_turnstile_proof(value, &env, &settings.admin_password_hash));
        let mut issued_proof = None;
        if settings.turnstile_enabled && existing_proof.is_none() && !authorized {
            if let Some(token) = req.headers().get("X-Turnstile-Token").ok().flatten() {
                if turnstile::verify(
                    &token,
                    &settings.turnstile_secret_key,
                    client_ip(&req).as_deref(),
                )
                .await
                .unwrap_or(false)
                {
                    issued_proof = create_turnstile_proof(&env, &settings.admin_password_hash);
                }
            }
        }
        let verified = !settings.turnstile_enabled
            || authorized
            || existing_proof.is_some()
            || issued_proof.is_some();
        let cookie_proof = issued_proof.clone();
        let returned_proof = issued_proof.or(existing_proof);
        let mut response = json(
            &serde_json::json!({
                "site_name": settings.site_name,
                "site_title": settings.site_name,
                "site_description": settings.site_description,
                "site_announcement": settings.site_announcement,
                "favicon_url": settings.favicon_url,
                "locale": settings.locale,
                "public_dashboard": settings.public_dashboard,
                "is_public": settings.public_dashboard,
                "authorization": authorized,
                "offline_threshold_seconds": settings.offline_threshold_seconds,
                "history_retention_days": settings.history_retention_days,
                "long_history_points": 120,
                "default_theme": settings.default_theme,
                "display_mode": "bar",
                "background_url": settings.background_url,
                "theme_url": settings.theme_url,
                "theme_options": settings.theme_options,
                "federation_sites": settings.federation_sites,
                "show_search": settings.show_search,
                "show_groups": settings.show_groups,
                "show_stats": settings.show_stats,
                "show_assets": settings.show_assets,
                "show_traffic": settings.show_traffic,
                "show_speed": settings.show_speed,
                "show_price": settings.show_price,
                "show_expiry": settings.show_expiry,
                "show_latency": settings.show_latency,
                "show_uptime": settings.show_uptime,
                "turnstile_enabled": settings.turnstile_enabled,
                "turnstile_login_enabled": settings.turnstile_login_enabled || settings.turnstile_enabled,
                "turnstile_site_key": settings.turnstile_site_key,
                "verified": verified,
                "turnstile_verified": returned_proof,
                "websocket": true
            }),
            200,
        )?;
        if let Some(proof) = cookie_proof {
            response.headers_mut().append(
                "Set-Cookie",
                &format!(
                    "cf_monitor_turnstile={proof}; Path=/; Max-Age=3600; HttpOnly; SameSite=Strict"
                ),
            )?;
        }
        return Ok(response);
    }

    if method == Method::Get && (path == "/api/themes" || path == "/theme") {
        return match theme::store().await {
            Ok(store) => json(&store, 200),
            Err(err) => {
                console_error!("theme store request failed: {err}");
                json(&serde_json::json!({ "schema": 1, "themes": [] }), 200)
            }
        };
    }

    if method == Method::Post && path == "/api/admin/login" {
        if settings.admin_password_hash.is_empty() && env.secret("ADMIN_PASSWORD").is_err() {
            return error("尚未配置 ADMIN_PASSWORD", 503);
        }
        let input: LoginRequest = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("请求格式无效", 400),
        };
        if input.username.trim().is_empty() || input.password.is_empty() {
            return error("请输入用户名和密码", 400);
        }
        if settings.turnstile_enabled || settings.turnstile_login_enabled {
            if settings.turnstile_secret_key.is_empty() {
                return error("Turnstile 密钥未配置", 503);
            }
            let verified = turnstile::verify(
                &input.turnstile_token,
                &settings.turnstile_secret_key,
                client_ip(&req).as_deref(),
            )
            .await
            .unwrap_or(false);
            if !verified {
                return error("Cloudflare 人机验证失败，请重试", 403);
            }
        }
        if !verify_credentials(
            &env,
            &settings.admin_username,
            &settings.admin_password_hash,
            &input.username,
            &input.password,
        ) {
            return error("用户名或密码错误", 401);
        }
        let Some(token) = create_session(&env, &settings.admin_password_hash) else {
            return error("会话密钥未配置", 503);
        };
        return json(&serde_json::json!({ "token": token }), 200);
    }

    if method == Method::Post && path == "/api/turnstile/verify" {
        if !settings.turnstile_enabled {
            return error("全站人机验证未启用", 400);
        }
        let input: TurnstileVerifyRequest = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("验证请求格式无效", 400),
        };
        let verified = turnstile::verify(
            &input.token,
            &settings.turnstile_secret_key,
            client_ip(&req).as_deref(),
        )
        .await
        .unwrap_or(false);
        if !verified {
            return error("Cloudflare 人机验证失败，请重试", 403);
        }
        let Some(proof) = create_turnstile_proof(&env, &settings.admin_password_hash) else {
            return error("验证凭据签名失败", 503);
        };
        let mut response = json(&serde_json::json!({ "verification": proof }), 200)?;
        response.headers_mut().append(
            "Set-Cookie",
            &format!(
                "cf_monitor_turnstile={proof}; Path=/; Max-Age=3600; HttpOnly; SameSite=Strict"
            ),
        )?;
        return Ok(response);
    }

    if method == Method::Post && path == "/api/agent/report" {
        let token = match bearer_token(&req) {
            Some(value) => value,
            None => return error("缺少探针凭据", 401),
        };
        let batch: AgentReportBatch = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("指标格式无效", 400),
        };
        if batch.server_id.is_empty() || batch.server_id.len() > 80 {
            return error("节点 ID 无效", 400);
        }
        if batch.samples.is_empty() || batch.samples.len() > 720 {
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
        let Some(row) = db::get_token_hash(&database, &batch.server_id).await? else {
            return error("节点不存在", 404);
        };
        if sha256_hex(&token) != row.token_hash {
            return error("探针凭据无效", 401);
        }
        let latency_results = batch
            .samples
            .iter()
            .flat_map(|report| report.latency_results.iter())
            .cloned()
            .collect::<Vec<_>>();
        if !latency_results.is_empty() {
            let assigned: HashSet<String> = latency::tasks_for_server(&database, &batch.server_id)
                .await?
                .into_iter()
                .map(|task| task.id)
                .collect();
            if latency_results
                .iter()
                .any(|result| !assigned.contains(&result.task_id))
            {
                return error("延迟结果包含未分配给该节点的任务", 400);
            }
        }
        db::save_reports(&database, &batch.server_id, &batch.samples).await?;
        latency::save_results(&database, &batch.server_id, &latency_results, received_at).await?;

        let public_dashboard = db::settings(
            &database,
            &default_name,
            default_threshold,
            &default_username,
        )
        .await?
        .public_dashboard;
        if row.hidden == 0 && public_dashboard {
            let latest = batch
                .samples
                .iter()
                .max_by_key(|report| report.timestamp)
                .expect("batch is non-empty");
            let payload = serde_json::to_string(&serde_json::json!({
                "type": "metrics",
                "server_id": batch.server_id,
                "timestamp": latest.timestamp,
                "metrics": latest
            }))?;
            let mib = |value: i64| value as f64 / 1024_f64.powi(2);
            let samples = batch.samples.iter().map(|report| serde_json::json!({
                "ts": report.timestamp * 1000,
                "data": {
                    "timestamp": report.timestamp * 1000,
                    "cpu": report.cpu,
                    "load_avg": format!("{} {} {}", report.load1, report.load5, report.load15),
                    "ram_used": mib(report.mem_used),
                    "ram_total": mib(report.mem_total),
                    "swap_used": mib(report.swap_used),
                    "swap_total": mib(report.swap_total),
                    "disk_used": mib(report.disk_used),
                    "disk_total": mib(report.disk_total),
                    "net_in_speed": report.net_in,
                    "net_out_speed": report.net_out,
                    "net_rx": report.net_rx_total,
                    "net_tx": report.net_tx_total,
                    "processes": report.processes,
                    "tcp_conn": report.tcp_connections,
                    "udp_conn": report.udp_connections,
                    "gpu": report.gpu_usage,
                }
            })).collect::<Vec<_>>();
            let compatibility_payload = serde_json::to_string(&serde_json::json!({
                "type": "batchUpdate",
                "updates": [{
                    "serverId": batch.server_id,
                    "samples": samples
                }]
            }))?;
            let server_id = batch.server_id.clone();
            ctx.wait_until(async move {
                let _ = live::broadcast(&env, &server_id, &payload).await;
                let _ = live::broadcast(&env, &server_id, &compatibility_payload).await;
            });
        }
        let config =
            db::agent_config(&database, &batch.server_id, AGENT_VERSION, &settings).await?;
        return json(
            &serde_json::json!({ "success": true, "config": config }),
            202,
        );
    }

    let admin = is_admin(&req, &env, &settings.admin_password_hash);
    let turnstile_verified = !settings.turnstile_enabled
        || admin
        || request_turnstile_proof(&req).is_some_and(|value| {
            verify_turnstile_proof(&value, &env, &settings.admin_password_hash)
        });

    if method == Method::Get && path == "/api/exchange-rates" {
        if !turnstile_verified {
            return error("请先完成 Cloudflare 人机验证", 403);
        }
        if !settings.public_dashboard && !admin {
            return error("仪表盘未公开", 401);
        }
        let rates = exchange::current(&database, now()).await?;
        let mut response = json(&rates, 200)?;
        response
            .headers_mut()
            .set("Cache-Control", "public, max-age=300")?;
        return Ok(response);
    }

    if method == Method::Get && path == "/api/ws" {
        if !turnstile_verified {
            return error("请先完成 Cloudflare 人机验证", 403);
        }
        if !settings.public_dashboard && !admin {
            return error("仪表盘未公开", 401);
        }
        return live::upgrade(req, &env).await;
    }

    if method == Method::Get && path == "/api/servers" {
        if !turnstile_verified {
            return error("请先完成 Cloudflare 人机验证", 403);
        }
        if !settings.public_dashboard && !admin {
            return error("仪表盘未公开", 401);
        }
        let raw_servers = db::list_servers(&database, false).await?;
        let mut latency_by_server: HashMap<String, Vec<latency::LatencySample>> = HashMap::new();
        for sample in latency::latest_all(&database).await? {
            latency_by_server
                .entry(sample.server_id.clone())
                .or_default()
                .push(sample);
        }
        let mut region_stats = serde_json::Map::new();
        for server in &raw_servers {
            let region = server.region.trim().to_ascii_uppercase();
            if region.is_empty() {
                continue;
            }
            let count = region_stats
                .get(&region)
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
                + 1;
            region_stats.insert(region, count.into());
        }
        let servers: Vec<_> = raw_servers
            .into_iter()
            .map(public_server)
            .map(|server| {
                let samples = latency_by_server.remove(&server.id).unwrap_or_default();
                server_json(server, settings.offline_threshold_seconds, samples)
            })
            .collect();
        return json(
            &serde_json::json!({
                "servers": servers,
                "server_time": now(),
                "latestReportUpdates": [],
                "regionStats": region_stats,
                "sysConfig": {
                    "display_mode": "bar",
                    "show_price": settings.show_price,
                    "show_expire": settings.show_expiry,
                    "show_tf": settings.show_traffic,
                    "show_time": settings.show_uptime,
                    "show_search": settings.show_search,
                    "show_groups": settings.show_groups
                }
            }),
            200,
        );
    }

    if method == Method::Get && path == "/api/server" {
        if !turnstile_verified {
            return error("请先完成 Cloudflare 人机验证", 403);
        }
        if !settings.public_dashboard && !admin {
            return error("仪表盘未公开", 401);
        }
        let id = req
            .url()?
            .query_pairs()
            .find_map(|(key, value)| (key == "id").then(|| value.to_string()))
            .unwrap_or_default();
        if id.is_empty() || id.len() > 80 {
            return error("节点 ID 无效", 400);
        }
        return match db::get_server(&database, &id, false).await? {
            Some(server) => {
                let samples = latency::latest_all(&database)
                    .await?
                    .into_iter()
                    .filter(|sample| sample.server_id == id)
                    .collect();
                json(
                    &server_json(
                        public_server(server),
                        settings.offline_threshold_seconds,
                        samples,
                    ),
                    200,
                )
            }
            None => error("节点不存在", 404),
        };
    }

    if method == Method::Get && path.starts_with("/api/servers/") {
        if !turnstile_verified {
            return error("请先完成 Cloudflare 人机验证", 403);
        }
        if !settings.public_dashboard && !admin {
            return error("仪表盘未公开", 401);
        }
        let Some(id) = server_id(&path, "/api/servers/") else {
            return error("节点 ID 无效", 400);
        };
        return match db::get_server(&database, &id, false).await? {
            Some(server) => {
                let samples = latency::latest_all(&database)
                    .await?
                    .into_iter()
                    .filter(|sample| sample.server_id == id)
                    .collect();
                json(
                    &server_json(
                        public_server(server),
                        settings.offline_threshold_seconds,
                        samples,
                    ),
                    200,
                )
            }
            None => error("节点不存在", 404),
        };
    }

    if method == Method::Get && path == "/api/history/all" {
        if !turnstile_verified {
            return error("请先完成 Cloudflare 人机验证", 403);
        }
        if !settings.public_dashboard && !admin {
            return error("仪表盘未公开", 401);
        }
        let url = req.url()?;
        let id = url
            .query_pairs()
            .find_map(|(key, value)| (key == "id").then(|| value.to_string()))
            .unwrap_or_default();
        if id.is_empty() || id.len() > 80 || db::get_server(&database, &id, false).await?.is_none()
        {
            return error("节点不存在", 404);
        }
        let hours = url
            .query_pairs()
            .find_map(|(key, value)| (key == "hours").then(|| value.parse::<f64>().ok()))
            .flatten()
            .map(|value| value.ceil() as i64)
            .unwrap_or(24);
        let points: Vec<_> = db::history(&database, &id, hours)
            .await?
            .into_iter()
            .map(history_json)
            .collect();
        return json(&points, 200);
    }

    if method == Method::Get && path.starts_with("/api/history/") {
        if !turnstile_verified {
            return error("请先完成 Cloudflare 人机验证", 403);
        }
        if !settings.public_dashboard && !admin {
            return error("仪表盘未公开", 401);
        }
        let Some(id) = server_id(&path, "/api/history/") else {
            return error("节点 ID 无效", 400);
        };
        if db::get_server(&database, &id, false).await?.is_none() {
            return error("节点不存在", 404);
        }
        let hours = req
            .url()?
            .query_pairs()
            .find(|(key, _)| key == "hours")
            .and_then(|(_, value)| value.parse::<i64>().ok())
            .unwrap_or(24);
        let points = db::history(&database, &id, hours).await?;
        return json(&serde_json::json!({ "points": points }), 200);
    }

    if method == Method::Get && path.starts_with("/api/latency/") {
        if !turnstile_verified {
            return error("请先完成 Cloudflare 人机验证", 403);
        }
        if !settings.public_dashboard && !admin {
            return error("仪表盘未公开", 401);
        }
        let Some(id) = server_id(&path, "/api/latency/") else {
            return error("节点 ID 无效", 400);
        };
        if db::get_server(&database, &id, false).await?.is_none() {
            return error("节点不存在", 404);
        }
        let hours = req
            .url()?
            .query_pairs()
            .find(|(key, _)| key == "hours")
            .and_then(|(_, value)| value.parse::<i64>().ok())
            .unwrap_or(24);
        let tasks = latency::tasks_for_server(&database, &id).await?;
        let points = latency::history(&database, &id, hours).await?;
        return json(
            &serde_json::json!({ "tasks": tasks, "points": points }),
            200,
        );
    }

    if path.starts_with("/api/admin/") && !admin {
        return error("登录已失效", 401);
    }

    if method == Method::Get && path == "/api/admin/latency-tasks" {
        return json(
            &serde_json::json!({ "tasks": latency::list_tasks(&database).await? }),
            200,
        );
    }

    if method == Method::Post && path == "/api/admin/latency-tasks" {
        let input: LatencyTaskInput = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("延迟任务格式无效", 400),
        };
        if let Some(message) = validate_latency_task(&input) {
            return error(message, 400);
        }
        let server_ids: HashSet<String> = db::list_servers(&database, true)
            .await?
            .into_iter()
            .map(|server| server.id)
            .collect();
        if input.server_ids.iter().any(|id| !server_ids.contains(id)) {
            return error("延迟任务包含不存在的服务器", 400);
        }
        let namespace = env.durable_object("LIVE_HUB")?;
        let id = namespace
            .unique_id()?
            .to_string()
            .chars()
            .take(16)
            .collect::<String>();
        latency::create_task(&database, &id, &input, now()).await?;
        return json(&serde_json::json!({ "id": id }), 201);
    }

    if (method == Method::Patch || method == Method::Delete)
        && path.starts_with("/api/admin/latency-tasks/")
    {
        let Some(id) = server_id(&path, "/api/admin/latency-tasks/") else {
            return error("延迟任务 ID 无效", 400);
        };
        if method == Method::Delete {
            return if latency::delete_task(&database, &id).await? {
                json(&Success { success: true }, 200)
            } else {
                error("延迟任务不存在", 404)
            };
        }
        let input: LatencyTaskInput = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("延迟任务格式无效", 400),
        };
        if let Some(message) = validate_latency_task(&input) {
            return error(message, 400);
        }
        let server_ids: HashSet<String> = db::list_servers(&database, true)
            .await?
            .into_iter()
            .map(|server| server.id)
            .collect();
        if input
            .server_ids
            .iter()
            .any(|server_id| !server_ids.contains(server_id))
        {
            return error("延迟任务包含不存在的服务器", 400);
        }
        return if latency::update_task(&database, &id, &input, now()).await? {
            json(&Success { success: true }, 200)
        } else {
            error("延迟任务不存在", 404)
        };
    }

    if method == Method::Get && path == "/api/admin/alert-rules" {
        return json(
            &serde_json::json!({ "rules": db::list_alert_rules(&database).await? }),
            200,
        );
    }

    if method == Method::Post && path == "/api/admin/alert-rules" {
        if db::list_alert_rules(&database).await?.len() >= 20 {
            return error("最多可创建 20 条资源告警规则", 400);
        }
        let input: AlertRuleInput = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("告警规则格式无效", 400),
        };
        if let Some(message) = validate_alert_rule(&input) {
            return error(message, 400);
        }
        let server_ids: HashSet<String> = db::list_servers(&database, true)
            .await?
            .into_iter()
            .map(|server| server.id)
            .collect();
        if input.server_ids.iter().any(|id| !server_ids.contains(id)) {
            return error("告警规则包含不存在的服务器", 400);
        }
        let id = env
            .durable_object("LIVE_HUB")?
            .unique_id()?
            .to_string()
            .chars()
            .take(16)
            .collect::<String>();
        db::create_alert_rule(&database, &id, &input).await?;
        return json(&serde_json::json!({ "id": id }), 201);
    }

    if (method == Method::Patch || method == Method::Delete)
        && path.starts_with("/api/admin/alert-rules/")
    {
        let Some(id) = server_id(&path, "/api/admin/alert-rules/") else {
            return error("告警规则 ID 无效", 400);
        };
        if method == Method::Delete {
            return if db::delete_alert_rule(&database, &id).await? {
                json(&Success { success: true }, 200)
            } else {
                error("告警规则不存在", 404)
            };
        }
        let input: AlertRuleInput = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("告警规则格式无效", 400),
        };
        if let Some(message) = validate_alert_rule(&input) {
            return error(message, 400);
        }
        let server_ids: HashSet<String> = db::list_servers(&database, true)
            .await?
            .into_iter()
            .map(|server| server.id)
            .collect();
        if input
            .server_ids
            .iter()
            .any(|server_id| !server_ids.contains(server_id))
        {
            return error("告警规则包含不存在的服务器", 400);
        }
        return if db::update_alert_rule(&database, &id, &input).await? {
            json(&Success { success: true }, 200)
        } else {
            error("告警规则不存在", 404)
        };
    }

    if method == Method::Get && path == "/api/admin/servers" {
        let servers = db::list_servers(&database, true).await?;
        return json(&serde_json::json!({ "servers": servers }), 200);
    }

    if method == Method::Delete && path == "/api/admin/servers" {
        let input: ServerBatchInput = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("批量删除格式无效", 400),
        };
        if input.ids.is_empty()
            || input.ids.len() > 500
            || input.ids.iter().any(|id| id.is_empty() || id.len() > 80)
            || input.ids.iter().collect::<HashSet<_>>().len() != input.ids.len()
        {
            return error("批量删除列表无效", 400);
        }
        db::delete_servers(&database, &input.ids).await?;
        return json(&Success { success: true }, 200);
    }

    if method == Method::Patch && path == "/api/admin/servers/order" {
        let input: ServerOrderInput = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("排序格式无效", 400),
        };
        if input.ids.len() > 500
            || input.ids.iter().any(|id| id.is_empty() || id.len() > 80)
            || input.ids.iter().collect::<HashSet<_>>().len() != input.ids.len()
        {
            return error("节点排序列表无效", 400);
        }
        let current = db::list_servers(&database, true).await?;
        let current_ids: HashSet<_> = current.iter().map(|server| &server.id).collect();
        if input.ids.len() != current_ids.len()
            || input.ids.iter().any(|id| !current_ids.contains(id))
        {
            return error("排序列表必须包含全部节点", 400);
        }
        db::reorder_servers(&database, &input.ids).await?;
        return json(&Success { success: true }, 200);
    }

    if method == Method::Post && path == "/api/admin/servers" {
        let input: ServerInput = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("节点格式无效", 400),
        };
        if let Some(message) = validate_server(&input) {
            return error(message, 400);
        }
        let namespace = env.durable_object("LIVE_HUB")?;
        let id_source = namespace.unique_id()?.to_string();
        let id = id_source.chars().take(16).collect::<String>();
        let token = namespace.unique_id()?.to_string();
        db::create_server(&database, &id, &sha256_hex(&token), &input).await?;
        latency::assign_defaults(&database, &id).await?;
        return json(&serde_json::json!({ "id": id, "agent_token": token }), 201);
    }

    if method == Method::Post && path.starts_with("/api/admin/servers/") && path.ends_with("/token")
    {
        let raw_id = path
            .strip_prefix("/api/admin/servers/")
            .and_then(|value| value.strip_suffix("/token"))
            .unwrap_or("")
            .trim_matches('/');
        if raw_id.is_empty() || raw_id.contains('/') || raw_id.len() > 80 {
            return error("节点 ID 无效", 400);
        }
        let namespace = env.durable_object("LIVE_HUB")?;
        let token = namespace.unique_id()?.to_string();
        return if db::update_token(&database, raw_id, &sha256_hex(&token)).await? {
            json(&serde_json::json!({ "agent_token": token }), 200)
        } else {
            error("节点不存在", 404)
        };
    }

    if (method == Method::Patch || method == Method::Delete)
        && path.starts_with("/api/admin/servers/")
    {
        let Some(id) = server_id(&path, "/api/admin/servers/") else {
            return error("节点 ID 无效", 400);
        };
        if method == Method::Delete {
            return if db::delete_server(&database, &id).await? {
                json(&Success { success: true }, 200)
            } else {
                error("节点不存在", 404)
            };
        }
        let input: ServerInput = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("节点格式无效", 400),
        };
        if let Some(message) = validate_server(&input) {
            return error(message, 400);
        }
        return if db::update_server(&database, &id, &input).await? {
            json(&Success { success: true }, 200)
        } else {
            error("节点不存在", 404)
        };
    }

    if method == Method::Get && path == "/api/admin/settings" {
        return json(&settings, 200);
    }

    if method == Method::Get && path == "/api/admin/theme-settings" {
        return json(&theme::settings_schema(&settings.theme_url).await?, 200);
    }

    if method == Method::Post && path == "/api/admin/themes/preview" {
        let input: ThemePreviewInput = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("主题地址格式无效", 400),
        };
        let Some(source) = theme::parse_theme_url(&input.theme_url) else {
            return error("主题地址必须是 GitHub tree 构建目录", 400);
        };
        if theme::load_index(&source.normalized, &settings.site_name)
            .await?
            .is_none()
        {
            return error("主题目录中未找到可用的 index.html", 400);
        }
        let Some(proof) =
            create_theme_preview(&env, &settings.admin_password_hash, &source.normalized)
        else {
            return error("无法创建主题预览", 503);
        };
        return json(
            &serde_json::json!({ "theme_url": source.normalized, "proof": proof }),
            200,
        );
    }

    if method == Method::Post && path == "/api/admin/themes/preview/clear" {
        let mut response = json(&Success { success: true }, 200)?;
        response.headers_mut().append(
            "Set-Cookie",
            "cf_monitor_theme_preview=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict",
        )?;
        response.headers_mut().append(
            "Set-Cookie",
            "cf_monitor_theme_proof=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict",
        )?;
        return Ok(response);
    }

    if method == Method::Post && path == "/api/admin/exchange-rates/refresh" {
        return match exchange::refresh(&database, now(), true).await {
            Ok((rates, _)) => json(&rates, 200),
            Err(err) => {
                console_error!("manual exchange-rate refresh failed: {err}");
                error("汇率更新失败，已保留数据库中的旧汇率", 502)
            }
        };
    }

    if method == Method::Get && path == "/api/admin/database" {
        let stats = db::database_stats(&database, settings.offline_threshold_seconds).await?;
        return json(&stats, 200);
    }

    if method == Method::Get && path == "/api/admin/cloudflare-usage" {
        let account_id = if settings.cloudflare_account_id.trim().is_empty() {
            env.secret("CLOUDFLARE_ACCOUNT_ID")
                .or_else(|_| env.var("CLOUDFLARE_ACCOUNT_ID"))
                .map(|value| value.to_string())
                .unwrap_or_default()
        } else {
            settings.cloudflare_account_id.clone()
        };
        let token = if settings.cloudflare_api_token.trim().is_empty() {
            env.secret("CLOUDFLARE_API_TOKEN")
                .or_else(|_| env.var("CLOUDFLARE_API_TOKEN"))
                .map(|value| value.to_string())
                .unwrap_or_default()
        } else {
            settings.cloudflare_api_token.clone()
        };
        if account_id.trim().is_empty() || token.trim().is_empty() {
            return error("尚未配置 Cloudflare 用量查询凭据", 503);
        }
        return match cloudflare::usage(token.trim(), account_id.trim(), now()).await {
            Ok(usage) => json(&usage, 200),
            Err(err) => {
                console_error!("cloudflare usage query failed: {err}");
                error("Cloudflare 用量查询失败，请检查 Token 权限和账户 ID", 502)
            }
        };
    }

    if method == Method::Delete && path == "/api/admin/history" {
        db::clear_history(&database).await?;
        return json(&Success { success: true }, 200);
    }

    if method == Method::Post && path == "/api/admin/notifications/test" {
        if settings.notification_endpoint.trim().is_empty() {
            return error("请先填写 Telegram Bot Token 和 Chat ID", 400);
        }
        if let Err(err) = notify::send(&settings, "CF Monitor 测试通知：通知渠道配置成功。").await
        {
            console_error!("test notification failed: {err}");
            return error("测试通知发送失败，请检查 Bot Token 和 Chat ID", 502);
        }
        return json(&Success { success: true }, 200);
    }

    if method == Method::Patch && path == "/api/admin/settings" {
        let mut input: SettingsInput = match req.json().await {
            Ok(value) => value,
            Err(_) => return error("设置格式无效", 400),
        };
        if input
            .site_name
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.chars().count() > 80)
        {
            return error("站点名称长度应为 1 至 80 个字符", 400);
        }
        if input
            .site_description
            .as_ref()
            .is_some_and(|value| value.chars().count() > 240)
        {
            return error("站点描述不能超过 240 个字符", 400);
        }
        if input
            .site_announcement
            .as_ref()
            .is_some_and(|value| value.chars().count() > 1000)
        {
            return error("站点公告不能超过 1000 个字符", 400);
        }
        if input
            .default_theme
            .as_deref()
            .is_some_and(|value| !matches!(value, "system" | "light" | "dark"))
        {
            return error("默认主题无效", 400);
        }
        if input
            .background_url
            .as_ref()
            .is_some_and(|value| value.chars().count() > 1000)
        {
            return error("背景地址过长", 400);
        }
        if input.theme_options.as_ref().is_some_and(|value| {
            let Some(options) = value.as_object() else {
                return true;
            };
            options.len() > 40
                || value.to_string().len() > 12_000
                || options.iter().any(|(key, value)| {
                    key.is_empty()
                        || key.len() > 64
                        || !key
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
                        || !(value.is_boolean()
                            || value.is_number()
                            || value
                                .as_str()
                                .is_some_and(|text| text.chars().count() <= 500))
                })
        }) {
            return error("主题设置格式无效", 400);
        }
        if let Some(value) = input.theme_url.as_deref() {
            if value.chars().count() > 1000 {
                return error("主题地址过长", 400);
            }
            let normalized = if value.trim().is_empty() {
                String::new()
            } else {
                let Some(source) = theme::parse_theme_url(value) else {
                    return error("主题地址必须是 GitHub tree 构建目录", 400);
                };
                source.normalized
            };
            if !normalized.is_empty()
                && normalized != settings.theme_url
                && theme::load_index(&normalized, &settings.site_name)
                    .await?
                    .is_none()
            {
                return error("主题目录中未找到可用的 index.html", 400);
            }
            input.theme_url = Some(normalized);
        }
        if input.admin_username.as_ref().is_some_and(|value| {
            value.trim().is_empty()
                || value.chars().count() > 64
                || value.chars().any(char::is_whitespace)
        }) {
            return error("用户名应为 1 至 64 个不含空格的字符", 400);
        }
        if input
            .new_password
            .as_ref()
            .is_some_and(|value| !value.is_empty() && !(8..=128).contains(&value.chars().count()))
        {
            return error("新密码长度应为 8 至 128 个字符", 400);
        }
        let enabled = input
            .turnstile_enabled
            .unwrap_or(settings.turnstile_enabled);
        let login_enabled = input
            .turnstile_login_enabled
            .unwrap_or(settings.turnstile_login_enabled);
        let site_key = input
            .turnstile_site_key
            .as_deref()
            .unwrap_or(&settings.turnstile_site_key)
            .trim();
        let secret_key = input
            .turnstile_secret_key
            .as_deref()
            .unwrap_or(&settings.turnstile_secret_key)
            .trim();
        if (enabled || login_enabled) && (site_key.is_empty() || secret_key.is_empty()) {
            return error("启用 Turnstile 前必须填写站点密钥和私钥", 400);
        }
        if input
            .notification_enabled
            .unwrap_or(settings.notification_enabled)
        {
            let token = input
                .notification_endpoint
                .as_deref()
                .unwrap_or(&settings.notification_endpoint)
                .trim();
            let chat_id = input
                .notification_target
                .as_deref()
                .unwrap_or(&settings.notification_target)
                .trim();
            if !notify::valid_config(token, chat_id) {
                return error("Telegram Bot Token 或 Chat ID 格式无效", 400);
            }
        }
        if input
            .cloudflare_account_id
            .as_deref()
            .is_some_and(|value| !valid_cloudflare_account_id(value))
        {
            return error("Cloudflare Account ID 应为 32 位十六进制字符", 400);
        }
        if input
            .cloudflare_api_token
            .as_deref()
            .is_some_and(|value| !valid_cloudflare_api_token(value))
        {
            return error("Cloudflare API Token 格式无效", 400);
        }
        if input
            .locale
            .as_deref()
            .is_some_and(|value| !matches!(value, "zh-CN" | "en"))
        {
            return error("界面语言仅支持简体中文或 English", 400);
        }
        if input.favicon_url.as_deref().is_some_and(|value| {
            !value.trim().is_empty()
                && Url::parse(value.trim())
                    .ok()
                    .is_none_or(|url| url.scheme() != "https")
        }) {
            return error("站点图标必须使用 HTTPS 地址", 400);
        }
        if input
            .cors_allowed_origins
            .as_deref()
            .is_some_and(|value| configured_origins(value).is_none())
            || input
                .csp_asset_origins
                .as_deref()
                .is_some_and(|value| configured_origins(value).is_none())
        {
            return error("来源列表只能包含完整的 HTTP(S) Origin", 400);
        }
        if input
            .federation_sites
            .as_ref()
            .is_some_and(|value| !valid_federation_sites(value))
        {
            return error("聚合站点应包含名称和 HTTPS 根地址，最多 16 个", 400);
        }
        let password_hash = if let Some(password) = input
            .new_password
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            let namespace = env.durable_object("LIVE_HUB")?;
            Some(hash_password(password, &namespace.unique_id()?.to_string()))
        } else {
            None
        };
        db::update_settings(&database, &input, password_hash.as_deref()).await?;
        let updated = db::settings(
            &database,
            &default_name,
            default_threshold,
            &default_username,
        )
        .await?;
        let token = if password_hash.is_some() {
            create_session(&env, &updated.admin_password_hash)
        } else {
            None
        };
        return json(
            &serde_json::json!({ "settings": updated, "token": token }),
            200,
        );
    }

    let preview = preview_theme(&req, &env, &settings.admin_password_hash);
    let effective_theme = preview
        .as_ref()
        .map(|(theme_url, _)| theme_url.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(settings.theme_url.as_str());

    if method == Method::Get && path.starts_with("/assets/") && !effective_theme.is_empty() {
        return theme::asset(effective_theme, path.trim_start_matches("/assets/")).await;
    }

    if method == Method::Get
        && !effective_theme.is_empty()
        && (path == "/favicon.ico" || path.starts_with("/flags/") || path.starts_with("/os-icons/"))
    {
        return theme::compatibility_asset(path.trim_start_matches('/')).await;
    }

    if method == Method::Get && (path == "/" || path == "/index.html") {
        if query_value(&req, "theme_url").is_some() && preview.is_none() {
            return error("主题预览已失效，请从后台重新打开", 401);
        }
        if !effective_theme.is_empty() {
            let Some(html) = theme::load_index(effective_theme, &settings.site_name).await? else {
                return error("当前第三方主题无法加载", 502);
            };
            return theme_html_response(
                html,
                preview
                    .as_ref()
                    .map(|(theme_url, proof)| (theme_url.as_str(), proof.as_str())),
                &csp_sources(&settings.csp_asset_origins),
            );
        }
    }

    if path.starts_with("/api/") {
        return error("接口不存在", 404);
    }

    let response = env.assets("ASSETS")?.fetch_request(req).await?;
    let mut response = mutable_response(response)?;
    let headers = response.headers_mut();
    headers.set("X-Content-Type-Options", "nosniff")?;
    headers.set("X-Frame-Options", "DENY")?;
    headers.set("Referrer-Policy", "strict-origin-when-cross-origin")?;
    headers.set(
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=()",
    )?;
    let extra_sources = csp_sources(&settings.csp_asset_origins);
    headers.set("Content-Security-Policy", &format!(
        "default-src 'self'; script-src 'self' https://challenges.cloudflare.com; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; connect-src 'self' ws: wss: {extra_sources}; frame-src https://challenges.cloudflare.com; font-src 'self' data: {extra_sources}; object-src 'none'; base-uri 'self'; frame-ancestors 'none'"
    ))?;
    Ok(response)
}

#[event(fetch, respond_with_errors)]
async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    let path = req.path();
    let method = req.method();
    let request_url = req.url().ok();
    let origin = req.headers().get("Origin").ok().flatten();
    let configured = if origin.is_some() && path.starts_with("/api/") {
        match env.d1("DB") {
            Ok(database) => db::get_setting(&database, "cors_allowed_origins")
                .await
                .ok()
                .flatten()
                .unwrap_or_default(),
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };
    let allow_origin = origin
        .as_deref()
        .zip(request_url.as_ref())
        .filter(|(origin, url)| origin_allowed(origin, url, &configured))
        .map(|(origin, _)| origin.to_string());

    if path == "/api/ws" && origin.is_some() && allow_origin.is_none() {
        return error("WebSocket Origin 未获授权", 403);
    }
    if method == Method::Options && path.starts_with("/api/") {
        let Some(origin) = allow_origin else {
            return error("跨域来源未获授权", 403);
        };
        let mut response = Response::empty()?.with_status(204);
        let headers = response.headers_mut();
        headers.set("Access-Control-Allow-Origin", &origin)?;
        headers.set(
            "Access-Control-Allow-Methods",
            "GET, POST, PATCH, DELETE, OPTIONS",
        )?;
        headers.set(
            "Access-Control-Allow-Headers",
            "Authorization, Content-Type, X-Turnstile-Verified, X-Turnstile-Token",
        )?;
        headers.set("Access-Control-Max-Age", "86400")?;
        headers.set("Vary", "Origin")?;
        return Ok(response);
    }

    match handle(req, env, ctx).await {
        Ok(mut response) => {
            if let Some(origin) = allow_origin {
                response
                    .headers_mut()
                    .set("Access-Control-Allow-Origin", &origin)?;
                response.headers_mut().set("Vary", "Origin")?;
            }
            Ok(response)
        }
        Err(err) => {
            console_error!("request failed: {err}");
            error("服务暂时不可用，请检查 D1 迁移与绑定", 500)
        }
    }
}

#[event(scheduled)]
async fn scheduled(_event: ScheduledEvent, env: Env, _ctx: ScheduleContext) {
    let Ok(database) = env.d1("DB") else {
        return;
    };
    let default_name = env_text(&env, "SITE_NAME", "CF Monitor");
    let default_threshold = env_number(&env, "OFFLINE_THRESHOLD_SECONDS", 180);
    let default_username = env_text(&env, "ADMIN_USERNAME", "admin");
    let settings = db::settings(
        &database,
        &default_name,
        default_threshold,
        &default_username,
    )
    .await;
    let retention = match &settings {
        Ok(settings) => settings.history_retention_days,
        Err(_) => env_number(&env, "HISTORY_RETENTION_DAYS", 30),
    };
    let current = now();
    let last_cleanup = db::get_setting(&database, "last_history_cleanup")
        .await
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if current - last_cleanup >= 86_400 {
        if let Err(err) = db::cleanup_history(&database, retention).await {
            console_error!("history cleanup failed: {err}");
        } else {
            let _ = db::save_setting(&database, "last_history_cleanup", &current.to_string()).await;
        }
    }
    match exchange::refresh(&database, current, false).await {
        Ok((rates, true)) => console_log!(
            "exchange rates updated from {} for {}",
            rates.source,
            rates.date
        ),
        Ok((_, false)) => {}
        Err(err) => console_error!("exchange-rate refresh failed: {err}"),
    }
    if let Ok(settings) = settings {
        if let Err(err) = notify::check_alerts(&database, &settings).await {
            console_error!("alert check failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        valid_cloudflare_account_id, valid_cloudflare_api_token, valid_ping_target,
        validate_latency_task, ADMIN_HTML, ADMIN_SCRIPT, ADMIN_STYLE,
    };
    use crate::models::LatencyTaskInput;

    #[test]
    fn validates_latency_targets() {
        for target in [
            "",
            "gd-ct-dualstack.ip.zstaticcdn.com",
            "127.0.0.1",
            "example.com:443",
            "router_1.local:80",
        ] {
            assert!(valid_ping_target(target), "expected valid target: {target}");
        }

        for target in [
            "https://example.com",
            "example.com/path",
            "example.com:0",
            "example.com:65536",
            "example.com:abc",
            "999.1.1.1",
            "[::1]:443",
            "bad host",
            "-example.com",
        ] {
            assert!(
                !valid_ping_target(target),
                "expected invalid target: {target}"
            );
        }
    }

    #[test]
    fn validates_dynamic_latency_tasks() {
        let mut task = LatencyTaskInput {
            name: "Cloudflare TCP".to_string(),
            task_type: "tcp".to_string(),
            target: "1.1.1.1:443".to_string(),
            interval_seconds: 60,
            default_enabled: false,
            server_ids: vec!["node-a".to_string()],
        };
        assert_eq!(validate_latency_task(&task), None);

        task.task_type = "icmp".to_string();
        assert_eq!(validate_latency_task(&task), Some("ICMP 目标不能包含端口"));
        task.target = "1.1.1.1".to_string();
        task.server_ids.clear();
        assert_eq!(
            validate_latency_task(&task),
            Some("请至少选择一个服务器，或开启默认分配")
        );
        task.default_enabled = true;
        assert_eq!(validate_latency_task(&task), None);
    }

    #[test]
    fn validates_cloudflare_usage_credentials() {
        assert!(valid_cloudflare_account_id(""));
        assert!(valid_cloudflare_account_id(
            "0123456789abcdef0123456789ABCDEF"
        ));
        assert!(!valid_cloudflare_account_id("account-id"));
        assert!(valid_cloudflare_api_token(""));
        assert!(valid_cloudflare_api_token("token_abc-123"));
        assert!(!valid_cloudflare_api_token("token with spaces"));
    }

    #[test]
    fn embeds_the_admin_frontend() {
        assert!(ADMIN_HTML.starts_with(b"<!doctype html>"));
        assert!(ADMIN_HTML
            .windows(b"/admin-assets/admin.js".len())
            .any(|value| value == b"/admin-assets/admin.js"));
        assert!(!ADMIN_SCRIPT.is_empty());
        assert!(!ADMIN_STYLE.is_empty());
    }
}
