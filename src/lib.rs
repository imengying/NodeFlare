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
use std::net::Ipv4Addr;

use futures_util::TryStreamExt;
use serde::{de::DeserializeOwned, Serialize};
use worker::*;

use crate::auth::{
    bearer_token, create_admin_jwt, create_theme_preview_proof, create_turnstile_proof,
    hash_password, is_admin, random_salt, sha256_hex, verify_credentials,
    verify_theme_preview_proof, verify_turnstile_proof,
};
use crate::models::{
    AgentReport, AgentReportBatch, AlertRuleInput, ApiError, LatencyTaskInput, LoginRequest,
    ServerBatchInput, ServerInput, ServerOrderInput, ServerView, SettingsInput, ThemeInput,
    ThemeView, TurnstileVerifyRequest,
};

const ADMIN_HTML: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin.html"));
const ADMIN_SCRIPT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin.js"));
const ADMIN_STYLE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/admin.css"));
const HISTORY_CACHE_SECONDS: i64 = 30;
const API_JSON_MAX_BYTES: usize = 1024 * 1024;
const AGENT_JSON_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
struct Success {
    success: bool,
}

#[derive(Serialize)]
struct PublicServer {
    id: String,
    name: String,
    region: String,
    group_name: String,
    tags: String,
    expires_at: Option<i64>,
    traffic_limit: i64,
    traffic_limit_type: String,
    price: f64,
    billing_cycle: i64,
    currency: String,
    auto_renewal: i64,
    public_remark: String,
    reset_day: i64,
    timestamp: Option<i64>,
    cpu: Option<f64>,
    load1: Option<f64>,
    load5: Option<f64>,
    load15: Option<f64>,
    mem_used: Option<i64>,
    mem_total: Option<i64>,
    swap_used: Option<i64>,
    swap_total: Option<i64>,
    disk_used: Option<i64>,
    disk_total: Option<i64>,
    net_in: Option<f64>,
    net_out: Option<f64>,
    net_rx_total: Option<i64>,
    net_tx_total: Option<i64>,
    uptime: Option<i64>,
    processes: Option<i64>,
    tcp_connections: Option<i64>,
    udp_connections: Option<i64>,
    cpu_cores: Option<i64>,
    cpu_model: Option<String>,
    os: Option<String>,
    kernel: Option<String>,
    arch: Option<String>,
    virtualization: Option<String>,
    gpu_usage: Option<f64>,
    gpu_model: Option<String>,
    agent_version: Option<String>,
    disk_read_bps: Option<f64>,
    disk_write_bps: Option<f64>,
    disk_read_iops: Option<f64>,
    disk_write_iops: Option<f64>,
    disk_await_ms: Option<f64>,
    disk_utilization: Option<f64>,
    message: Option<String>,
    disks: serde_json::Value,
    gpus: serde_json::Value,
    latency: Vec<latency::LatencySample>,
}

fn now() -> i64 {
    Date::now().as_millis() as i64 / 1000
}

fn env_text(env: &Env, key: &str, fallback: &str) -> String {
    env.var(key)
        .ok()
        .map(|v| v.to_string())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn env_number(env: &Env, key: &str, fallback: i64) -> i64 {
    env_text(env, key, &fallback.to_string())
        .parse()
        .unwrap_or(fallback)
}

fn env_secret_text(env: &Env, key: &str) -> String {
    env.secret(key)
        .or_else(|_| env.var(key))
        .map(|value| value.to_string())
        .unwrap_or_default()
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

async fn request_json<T: DeserializeOwned>(req: &mut Request, limit: usize) -> Result<T> {
    if req
        .headers()
        .get("Content-Length")?
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length == 0 || length > limit)
    {
        return Err(Error::RustError(
            "JSON request body size is invalid".to_string(),
        ));
    }
    let mut bytes = Vec::new();
    let mut stream = req.stream()?;
    while let Some(mut chunk) = stream.try_next().await? {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > limit)
        {
            return Err(Error::RustError(
                "JSON request body is too large".to_string(),
            ));
        }
        bytes.append(&mut chunk);
    }
    serde_json::from_slice(&bytes).map_err(|error| Error::RustError(error.to_string()))
}

fn error(message: &str, status: u16) -> Result<Response> {
    json(&ApiError { error: message }, status)
}

fn rate_limited() -> Result<Response> {
    let mut response = error("请求过于频繁，请稍后重试", 429)?;
    response.headers_mut().set("Retry-After", "60")?;
    Ok(response)
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

fn secure_public_response(mut response: Response) -> Result<Response> {
    let headers = response.headers_mut();
    headers.set("X-Content-Type-Options", "nosniff")?;
    headers.set("X-Frame-Options", "DENY")?;
    headers.set("Referrer-Policy", "strict-origin-when-cross-origin")?;
    headers.set(
        "Permissions-Policy",
        "camera=(), microphone=(), geolocation=()",
    )?;
    headers.set(
        "Content-Security-Policy",
        "default-src 'self'; script-src 'self' https://challenges.cloudflare.com; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; connect-src 'self' ws: wss:; frame-src https://challenges.cloudflare.com; font-src 'self' data:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'",
    )?;
    Ok(response)
}

fn remote_theme_content_type(path: &str) -> Option<&'static str> {
    let path = path.split('?').next().unwrap_or(path);
    if path == "index.html" {
        return Some("text/html; charset=utf-8");
    }
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("js" | "mjs") => Some("application/javascript; charset=utf-8"),
        Some("css") => Some("text/css; charset=utf-8"),
        Some("json" | "map") => Some("application/json; charset=utf-8"),
        Some("svg") => Some("image/svg+xml"),
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("ico") => Some("image/x-icon"),
        Some("woff") => Some("font/woff"),
        Some("woff2") => Some("font/woff2"),
        Some("ttf") => Some("font/ttf"),
        Some("wasm") => Some("application/wasm"),
        _ => None,
    }
}

fn remote_theme_failure(message: &str) -> Result<Response> {
    let mut response = Response::error(message, 502)?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    secure_public_response(response)
}

async fn remote_theme_response(path: &str, base: &str) -> Result<Option<Response>> {
    let (remote_url, relative) =
        if path == "/" || path == "/index.html" || path.starts_with("/instance/") {
            let Some(index) = theme::index_url(base) else {
                return Ok(Some(remote_theme_failure("远程主题来源不受支持")?));
            };
            (index, "index.html".to_string())
        } else if let Some(url) = theme::asset_url(base, path) {
            (url, path.trim_start_matches('/').to_string())
        } else {
            return Ok(Some(remote_theme_failure("远程主题资源路径不受支持")?));
        };
    let is_index = relative == "index.html";
    let cache = Cache::default();
    if let Ok(Some(response)) = cache.get(remote_url.as_str(), false).await {
        return Ok(Some(secure_public_response(mutable_response(response)?)?));
    }
    let request = Request::new(&remote_url, Method::Get)?;
    let mut response = match Fetch::Request(request).send().await {
        Ok(response) if (200..300).contains(&response.status_code()) => response,
        Ok(response) => {
            console_warn!(
                "remote theme returned HTTP {} for {}",
                response.status_code(),
                remote_url
            );
            return if is_index {
                Ok(None)
            } else {
                Ok(Some(remote_theme_failure("远程主题资源暂时不可用")?))
            };
        }
        Err(error) => {
            console_warn!("remote theme request failed for {remote_url}: {error}");
            return if is_index {
                Ok(None)
            } else {
                Ok(Some(remote_theme_failure("远程主题资源加载失败")?))
            };
        }
    };
    let limit = if is_index {
        theme::INDEX_MAX_BYTES
    } else {
        theme::ASSET_MAX_BYTES
    };
    let Some(body) = theme::read_response_limited(&mut response, limit).await? else {
        return if is_index {
            Ok(None)
        } else {
            Ok(Some(remote_theme_failure("远程主题资源大小无效")?))
        };
    };
    let mut response = Response::from_bytes(body)?;
    let headers = response.headers_mut();
    headers.set("Cache-Control", "public, max-age=300")?;
    if let Some(content_type) = remote_theme_content_type(&relative) {
        headers.set("Content-Type", content_type)?;
    }
    let mut response = secure_public_response(response)?;
    if let Ok(cached) = response.cloned() {
        let _ = cache.put(remote_url.as_str(), cached).await;
    }
    Ok(Some(response))
}

async fn remote_theme_preview_response(
    relative: &str,
    base: &str,
    preview_prefix: &str,
) -> Result<Response> {
    if relative.starts_with("assets/") {
        let path = format!("/{relative}");
        return Ok(remote_theme_response(&path, base)
            .await?
            .unwrap_or(remote_theme_failure("主题预览资源不存在")?));
    }
    if !relative.is_empty() && relative != "index.html" && !relative.starts_with("instance/") {
        return Response::error("Theme preview path not found", 404);
    }
    let Some(index) = theme::index_url(base) else {
        return remote_theme_failure("远程主题来源不受支持");
    };
    let request = Request::new(&index, Method::Get)?;
    let mut response = Fetch::Request(request).send().await?;
    if !(200..300).contains(&response.status_code()) {
        return remote_theme_failure("主题预览页面暂时不可用");
    }
    let Some(body) = theme::read_response_limited(&mut response, theme::INDEX_MAX_BYTES).await?
    else {
        return remote_theme_failure("主题预览页面大小无效");
    };
    let html = String::from_utf8(body)
        .map_err(|_| Error::RustError("主题 index.html 不是 UTF-8 文本".to_string()))?;
    let asset_prefix = format!("{preview_prefix}/assets/");
    let html = html
        .replace("\"/assets/", &format!("\"{asset_prefix}"))
        .replace("'/assets/", &format!("'{asset_prefix}"));
    let mut response = Response::from_html(html)?;
    response.headers_mut().set("Cache-Control", "no-store")?;
    secure_public_response(response)
}

fn valid_ping_target(value: &str) -> bool {
    let raw = value.trim();
    if raw.is_empty() {
        return false;
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
        return host.parse::<Ipv4Addr>().is_ok_and(public_probe_ipv4);
    }
    let lower = host.to_ascii_lowercase();
    if labels.len() < 2
        || ["local", "localhost", "internal", "lan", "localdomain"]
            .iter()
            .any(|suffix| lower == *suffix || lower.ends_with(&format!(".{suffix}")))
        || lower == "home.arpa"
        || lower.ends_with(".home.arpa")
    {
        return false;
    }
    labels.iter().all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
            && label
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphanumeric())
            && label
                .chars()
                .last()
                .is_some_and(|character| character.is_ascii_alphanumeric())
    })
}

fn public_probe_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
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

fn valid_password_derived(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn submitted_secret<'a>(submitted: Option<&'a str>, stored: &'a str) -> &'a str {
    match submitted.map(str::trim) {
        Some(db::SECRET_MASK) | None => stored.trim(),
        Some(value) => value,
    }
}

fn same_origin(origin: &str, request_url: &Url) -> bool {
    origin.trim().trim_end_matches('/') == request_url.origin().ascii_serialization()
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
        return Some("目标应为公网域名、公网 IPv4 或 TCP host:port");
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

fn validate_theme(input: &ThemeInput) -> Option<&'static str> {
    if !(1..=80).contains(&input.name.trim().chars().count()) {
        return Some("主题名称长度应为 1 至 80 个字符");
    }
    if input.description.trim().chars().count() > 300 {
        return Some("主题说明不能超过 300 个字符");
    }
    if theme::normalize_url(&input.url).is_none() {
        return Some("主题 URL 必须是 GitHub tree 地址");
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
    if !input.price.is_finite() || !(-1.0..=1_000_000_000.0).contains(&input.price) {
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

fn public_server(server: ServerView, latency: Vec<latency::LatencySample>) -> PublicServer {
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
    PublicServer {
        id: server.id,
        name: server.name,
        region: server.region,
        group_name: server.group_name,
        tags: server.tags,
        expires_at: server.expires_at,
        traffic_limit: server.traffic_limit,
        traffic_limit_type: server.traffic_limit_type,
        price: server.price,
        billing_cycle: server.billing_cycle,
        currency: server.currency,
        auto_renewal: server.auto_renewal,
        public_remark: server.public_remark,
        reset_day: server.reset_day,
        timestamp: server.timestamp,
        cpu: server.cpu,
        load1: server.load1,
        load5: server.load5,
        load15: server.load15,
        mem_used: server.mem_used,
        mem_total: server.mem_total,
        swap_used: server.swap_used,
        swap_total: server.swap_total,
        disk_used: server.disk_used,
        disk_total: server.disk_total,
        net_in: server.net_in,
        net_out: server.net_out,
        net_rx_total: server.net_rx_total,
        net_tx_total: server.net_tx_total,
        uptime: server.uptime,
        processes: server.processes,
        tcp_connections: server.tcp_connections,
        udp_connections: server.udp_connections,
        cpu_cores: server.cpu_cores,
        cpu_model: server.cpu_model,
        os: server.os,
        kernel: server.kernel,
        arch: server.arch,
        virtualization: server.virtualization,
        gpu_usage: server.gpu_usage,
        gpu_model: server.gpu_model,
        agent_version: server.agent_version,
        disk_read_bps: server.disk_read_bps,
        disk_write_bps: server.disk_write_bps,
        disk_read_iops: server.disk_read_iops,
        disk_write_iops: server.disk_write_iops,
        disk_await_ms: server.disk_await_ms,
        disk_utilization: server.disk_utilization,
        message: server.message,
        disks,
        gpus,
        latency,
    }
}

fn request_cookie(req: &Request, name: &str) -> Option<String> {
    let raw = req.headers().get("Cookie").ok().flatten()?;
    raw.split(';').find_map(|entry| {
        let (key, value) = entry.trim().split_once('=')?;
        (key == name).then(|| value.to_string())
    })
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

fn rate_limit_binding(method: Method, path: &str) -> Option<&'static str> {
    if method == Method::Post && matches!(path, "/api/admin/login" | "/api/turnstile/verify") {
        Some("AUTH_RATE_LIMITER")
    } else if method == Method::Post && path == "/api/agent/report" {
        Some("AGENT_RATE_LIMITER")
    } else if path.starts_with("/api/") {
        Some("API_RATE_LIMITER")
    } else {
        None
    }
}

fn rate_limit_key(req: &Request) -> String {
    let identity = client_ip(req).unwrap_or_else(|| {
        req.headers()
            .get("User-Agent")
            .ok()
            .flatten()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "anonymous".to_string())
    });
    sha256_hex(&identity)
}

fn allow_on_rate_limit_failure(binding: &str) -> bool {
    binding != "AUTH_RATE_LIMITER"
}

async fn request_within_rate_limit(env: &Env, binding: &str, key: String) -> bool {
    let limiter = match env.rate_limiter(binding) {
        Ok(value) => value,
        Err(error) => {
            console_warn!("rate limit binding {binding} unavailable: {error}");
            return allow_on_rate_limit_failure(binding);
        }
    };
    match limiter.limit(key).await {
        Ok(outcome) => outcome.success,
        Err(error) => {
            console_warn!("rate limit check failed for {binding}: {error}");
            allow_on_rate_limit_failure(binding)
        }
    }
}

fn requested_hours(req: &Request, maximum: i64) -> Result<i64> {
    Ok(req
        .url()?
        .query_pairs()
        .find(|(key, _)| key == "hours")
        .and_then(|(_, value)| value.parse::<i64>().ok())
        .unwrap_or(24)
        .clamp(1, maximum))
}

fn history_cache_key(
    request_url: &Url,
    kind: &str,
    server_id: &str,
    hours: i64,
    version: i64,
) -> String {
    let mut url = request_url.clone();
    url.set_path(&format!("/__nodeflare-cache/{kind}/{server_id}"));
    url.set_query(Some(&format!("hours={hours}&version={version}")));
    url.set_fragment(None);
    url.to_string()
}

async fn cached_history_response(key: &str) -> Option<Response> {
    match Cache::default().get(key, false).await {
        Ok(Some(response)) => {
            let mut response = mutable_response(response).ok()?;
            response
                .headers_mut()
                .set("Cache-Control", "no-store")
                .ok()?;
            response.headers_mut().set("X-Cache", "HIT").ok()?;
            Some(response)
        }
        Ok(None) => None,
        Err(error) => {
            console_warn!("history cache read failed: {error}");
            None
        }
    }
}

async fn store_history_response(key: &str, response: &mut Response) {
    if response.headers_mut().set("X-Cache", "MISS").is_err() {
        return;
    }
    let Ok(mut cached) = response.cloned() else {
        return;
    };
    if cached
        .headers_mut()
        .set(
            "Cache-Control",
            &format!("public, max-age={HISTORY_CACHE_SECONDS}"),
        )
        .is_err()
    {
        return;
    }
    if let Err(error) = Cache::default().put(key, cached).await {
        console_warn!("history cache write failed: {error}");
    }
}

fn request_turnstile_proof(req: &Request) -> Option<String> {
    req.headers()
        .get("X-Turnstile-Verified")
        .ok()
        .flatten()
        .or_else(|| request_cookie(req, "nodeflare_turnstile"))
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

async fn handle_agent_report(
    mut req: Request,
    env: Env,
    ctx: Context,
    database: &D1Database,
) -> Result<Response> {
    let agent_config_hash = req
        .headers()
        .get("X-Agent-Config-Sha256")?
        .unwrap_or_default();
    let agent_config_schema = req
        .headers()
        .get("X-Agent-Config-Schema")?
        .unwrap_or_default();
    let token = match bearer_token(&req) {
        Some(value) => value,
        None => return error("缺少探针凭据", 401),
    };
    let batch: AgentReportBatch = match request_json(&mut req, AGENT_JSON_MAX_BYTES).await {
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
    let Some(row) = db::get_token_hash(database, &batch.server_id).await? else {
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
        let assigned: HashSet<String> = latency::tasks_for_server(database, &batch.server_id)
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
    db::save_reports(database, &batch.server_id, &batch.samples).await?;
    latency::save_results(database, &batch.server_id, &latency_results, received_at).await?;

    let public_dashboard = db::get_setting(database, "public_dashboard")
        .await?
        .is_none_or(|value| value == "true");
    if row.hidden == 0 && public_dashboard {
        let latest = batch
            .samples
            .iter()
            .max_by_key(|report| report.timestamp)
            .expect("batch is non-empty");
        let metrics = serde_json::to_value(latest)?;
        let payload = serde_json::to_string(&serde_json::json!({
            "type": "metrics",
            "server_id": batch.server_id,
            "timestamp": latest.timestamp,
            "metrics": metrics
        }))?;
        let server_id = batch.server_id.clone();
        ctx.wait_until(async move {
            let _ = live::broadcast(&env, &server_id, &payload).await;
        });
    }
    let config = db::agent_config(database, &batch.server_id).await?;
    let config_json = serde_json::to_string(&config)?;
    let config_hash = sha256_hex(&format!("1:{config_json}"));
    if agent_config_schema == "1" && agent_config_hash == config_hash {
        let mut response = Response::empty()?.with_status(204);
        response.headers_mut().set("X-Agent-Config-Schema", "1")?;
        response
            .headers_mut()
            .set("X-Agent-Config-Sha256", &config_hash)?;
        response.headers_mut().set("Cache-Control", "no-store")?;
        return Ok(response);
    }
    let mut response = json(
        &serde_json::json!({ "success": true, "config": config }),
        202,
    )?;
    response.headers_mut().set("X-Agent-Config-Schema", "1")?;
    response
        .headers_mut()
        .set("X-Agent-Config-Sha256", &config_hash)?;
    Ok(response)
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

    let database = env.d1("DB")?;

    if method == Method::Post && path == "/api/agent/report" {
        return handle_agent_report(req, env, ctx, &database).await;
    }

    let default_name = env_text(&env, "SITE_NAME", "NodeFlare");
    let default_threshold = env_number(&env, "OFFLINE_THRESHOLD_SECONDS", 180).clamp(30, 3600);
    let default_retention = env_number(&env, "HISTORY_RETENTION_DAYS", 30).clamp(1, 365);
    let default_username = env_text(&env, "ADMIN_USERNAME", "admin");
    let settings = db::settings(
        &database,
        &default_name,
        default_threshold,
        default_retention,
        &default_username,
    )
    .await?;
    let environment_turnstile_site_key = env_secret_text(&env, "TURNSTILE_SITE_KEY");
    let environment_turnstile_secret_key = env_secret_text(&env, "TURNSTILE_SECRET_KEY");
    let turnstile_site_key = if settings.turnstile_site_key.trim().is_empty() {
        environment_turnstile_site_key.as_str()
    } else {
        settings.turnstile_site_key.as_str()
    };
    let turnstile_secret_key = if settings.turnstile_secret_key.trim().is_empty() {
        environment_turnstile_secret_key.as_str()
    } else {
        settings.turnstile_secret_key.as_str()
    };
    let turnstile_configured =
        !turnstile_site_key.trim().is_empty() && !turnstile_secret_key.trim().is_empty();
    let public_turnstile_enabled = settings.turnstile_enabled && turnstile_configured;
    let login_protection_enabled = settings.turnstile_login_enabled && turnstile_configured;

    if method == Method::Get && path == "/api/config" {
        return json(
            &serde_json::json!({
                "site_name": settings.site_name,
                "site_description": settings.site_description,
                "site_announcement": settings.site_announcement,
                "favicon_url": settings.favicon_url,
                "locale": settings.locale,
                "public_dashboard": settings.public_dashboard,
                "offline_threshold_seconds": settings.offline_threshold_seconds,
                "history_retention_days": settings.history_retention_days,
                "default_theme": settings.default_theme,
                "active_theme_id": settings.active_theme_id,
                "background_url": settings.background_url,
                "theme_options": settings.theme_options,
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
                "turnstile_enabled": public_turnstile_enabled,
                "turnstile_login_enabled": login_protection_enabled || public_turnstile_enabled,
                "turnstile_site_key": turnstile_site_key,
                "password_client_salt": settings.password_client_salt,
                "websocket": true
            }),
            200,
        );
    }

    if method == Method::Post && path == "/api/admin/login" {
        if settings.admin_password_hash.is_empty() && env.secret("ADMIN_PASSWORD").is_err() {
            return error("尚未配置 ADMIN_PASSWORD", 503);
        }
        let input: LoginRequest = match request_json(&mut req, API_JSON_MAX_BYTES).await {
            Ok(value) => value,
            Err(_) => return error("请求格式无效", 400),
        };
        if input.username.trim().is_empty()
            || input.password.is_empty()
            || !valid_password_derived(&input.password_derived)
        {
            return error("请输入用户名和密码", 400);
        }
        let login_turnstile_enabled = public_turnstile_enabled || login_protection_enabled;
        if login_turnstile_enabled {
            let verified = turnstile::verify(
                &input.turnstile_token,
                turnstile_secret_key,
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
            &input.password_derived,
        ) {
            return error("用户名或密码错误", 401);
        }
        let session_password_hash = if settings.admin_password_hash.is_empty() {
            let Some(salt) = random_salt() else {
                return error("运行时随机源不可用，无法初始化管理员密码", 503);
            };
            let password_hash = hash_password(&input.password_derived, &salt);
            db::save_setting(&database, "admin_password_hash", &password_hash).await?;
            password_hash
        } else {
            settings.admin_password_hash.clone()
        };
        let Some(token) = create_admin_jwt(&env, &session_password_hash) else {
            return error("会话密钥未配置", 503);
        };
        return json(&serde_json::json!({ "token": token }), 200);
    }

    if method == Method::Post && path == "/api/turnstile/verify" {
        if !public_turnstile_enabled {
            return error("全站人机验证未启用", 400);
        }
        let input: TurnstileVerifyRequest = match request_json(&mut req, API_JSON_MAX_BYTES).await {
            Ok(value) => value,
            Err(_) => return error("验证请求格式无效", 400),
        };
        let verified = turnstile::verify(
            &input.token,
            turnstile_secret_key,
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
                "nodeflare_turnstile={proof}; Path=/; Max-Age=3600; HttpOnly; SameSite=Strict"
            ),
        )?;
        return Ok(response);
    }

    let admin = is_admin(&req, &env, &settings.admin_password_hash);
    let turnstile_verified = !public_turnstile_enabled
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
        let servers: Vec<_> = raw_servers
            .into_iter()
            .map(|server| {
                let samples = latency_by_server.remove(&server.id).unwrap_or_default();
                public_server(server, samples)
            })
            .collect();
        return json(&serde_json::json!({ "servers": servers }), 200);
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
                json(&public_server(server, samples), 200)
            }
            None => error("节点不存在", 404),
        };
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
        let hours = requested_hours(&req, 24 * 30)?;
        let cache_key = history_cache_key(
            &req.url()?,
            "metrics",
            &id,
            hours,
            settings.history_cache_version,
        );
        if let Some(response) = cached_history_response(&cache_key).await {
            return Ok(response);
        }
        let points = db::history(&database, &id, hours).await?;
        let mut response = json(&serde_json::json!({ "points": points }), 200)?;
        store_history_response(&cache_key, &mut response).await;
        return Ok(response);
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
        let hours = requested_hours(&req, 24 * 365)?;
        let cache_key = history_cache_key(
            &req.url()?,
            "latency",
            &id,
            hours,
            settings.history_cache_version,
        );
        if let Some(response) = cached_history_response(&cache_key).await {
            return Ok(response);
        }
        let tasks = latency::tasks_for_server(&database, &id).await?;
        let points = latency::history(&database, &id, hours).await?;
        let mut response = json(
            &serde_json::json!({ "tasks": tasks, "points": points }),
            200,
        )?;
        store_history_response(&cache_key, &mut response).await;
        return Ok(response);
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
        let input: LatencyTaskInput = match request_json(&mut req, API_JSON_MAX_BYTES).await {
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
        let input: LatencyTaskInput = match request_json(&mut req, API_JSON_MAX_BYTES).await {
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
        let input: AlertRuleInput = match request_json(&mut req, API_JSON_MAX_BYTES).await {
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
        let input: AlertRuleInput = match request_json(&mut req, API_JSON_MAX_BYTES).await {
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
        let input: ServerBatchInput = match request_json(&mut req, API_JSON_MAX_BYTES).await {
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
        let input: ServerOrderInput = match request_json(&mut req, API_JSON_MAX_BYTES).await {
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
        let input: ServerInput = match request_json(&mut req, API_JSON_MAX_BYTES).await {
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
        let input: ServerInput = match request_json(&mut req, API_JSON_MAX_BYTES).await {
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

    if method == Method::Get && path == "/api/admin/themes" {
        let mut themes = vec![ThemeView {
            id: theme::BUILTIN_THEME_ID.to_string(),
            name: theme::BUILTIN_THEME_NAME.to_string(),
            description: "NodeFlare 内置默认主题".to_string(),
            url: String::new(),
            builtin: true,
            active: settings.active_theme_id == theme::BUILTIN_THEME_ID,
        }];
        themes.extend(db::list_themes(&database, &settings.active_theme_id).await?);
        return json(&serde_json::json!({ "themes": themes }), 200);
    }

    if method == Method::Post && path == "/api/admin/themes" {
        if db::list_themes(&database, &settings.active_theme_id)
            .await?
            .len()
            >= 32
        {
            return error("最多可添加 32 个第三方主题", 400);
        }
        let input: ThemeInput = match request_json(&mut req, API_JSON_MAX_BYTES).await {
            Ok(value) => value,
            Err(_) => return error("主题格式无效", 400),
        };
        if let Some(message) = validate_theme(&input) {
            return error(message, 400);
        }
        let resolved = match theme::resolve_url(&input.url) {
            Ok(value) => value,
            Err(err) => {
                console_warn!("theme URL resolution failed: {err}");
                return error("无法解析主题地址，请检查 URL", 422);
            }
        };
        let digest = sha256_hex(&resolved.source_url);
        let id = format!("theme-{}", &digest[..16]);
        if db::theme_exists(&database, &id).await? {
            return error("该主题 URL 已添加", 409);
        }
        if let Err(err) = theme::validate_remote(&resolved.resolved_url).await {
            console_warn!(
                "theme validation failed for {}: {err}",
                resolved.resolved_url
            );
            return error("无法读取主题 index.html，请检查 URL 和主题构建产物", 422);
        }
        db::create_theme(&database, &id, &input, &resolved.source_url, now()).await?;
        return json(&serde_json::json!({ "id": id }), 201);
    }

    if method == Method::Post
        && path.starts_with("/api/admin/themes/")
        && path.ends_with("/preview")
    {
        let id = path
            .strip_prefix("/api/admin/themes/")
            .and_then(|value| value.strip_suffix("/preview"))
            .unwrap_or("")
            .trim_matches('/');
        if id.is_empty() || id.contains('/') || id.len() > 80 {
            return error("主题 ID 无效", 400);
        }
        if id == theme::BUILTIN_THEME_ID || !db::theme_exists(&database, id).await? {
            return error("远程主题不存在", 404);
        }
        let Some(proof) = create_theme_preview_proof(&env, &settings.admin_password_hash, id)
        else {
            return error("无法创建主题预览凭据", 503);
        };
        return json(
            &serde_json::json!({
                "preview_url": format!("/__theme-preview/{proof}/")
            }),
            200,
        );
    }

    if method == Method::Post
        && path.starts_with("/api/admin/themes/")
        && path.ends_with("/activate")
    {
        let id = path
            .strip_prefix("/api/admin/themes/")
            .and_then(|value| value.strip_suffix("/activate"))
            .unwrap_or("")
            .trim_matches('/');
        if id.is_empty() || id.contains('/') || id.len() > 80 {
            return error("主题 ID 无效", 400);
        }
        if id != theme::BUILTIN_THEME_ID {
            let Some(url) = db::theme_url(&database, id).await? else {
                return error("主题不存在", 404);
            };
            if let Err(err) = theme::validate_remote(&url).await {
                console_warn!("theme activation failed for {url}: {err}");
                return error("主题当前不可访问，未切换主题", 422);
            }
        }
        if !db::set_active_theme(&database, id).await? {
            return error("主题不存在", 404);
        }
        return json(&Success { success: true }, 200);
    }

    if method == Method::Delete && path.starts_with("/api/admin/themes/") {
        let Some(id) = server_id(&path, "/api/admin/themes/") else {
            return error("主题 ID 无效", 400);
        };
        if id == theme::BUILTIN_THEME_ID {
            return error("内置主题不能删除", 400);
        }
        return if db::delete_theme(&database, &id).await? {
            json(&Success { success: true }, 200)
        } else {
            error("主题不存在", 404)
        };
    }

    if method == Method::Get && path == "/api/admin/theme-settings" {
        return json(&theme::settings_schema(), 200);
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
            env.secret("CF_USAGE_ACCOUNT_ID")
                .or_else(|_| env.var("CF_USAGE_ACCOUNT_ID"))
                .map(|value| value.to_string())
                .unwrap_or_default()
        } else {
            settings.cloudflare_account_id.clone()
        };
        let token = if settings.cloudflare_api_token.trim().is_empty() {
            env.secret("CF_USAGE_API_TOKEN")
                .or_else(|_| env.var("CF_USAGE_API_TOKEN"))
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
        db::save_setting(&database, "history_cache_version", &now().to_string()).await?;
        return json(&Success { success: true }, 200);
    }

    if method == Method::Post && path == "/api/admin/notifications/test" {
        if settings.notification_endpoint.trim().is_empty() {
            return error("请先填写 Telegram Bot Token 和 Chat ID", 400);
        }
        if let Err(err) = notify::send(&settings, "NodeFlare 测试通知：通知渠道配置成功。").await
        {
            console_error!("test notification failed: {err}");
            return error("测试通知发送失败，请检查 Bot Token 和 Chat ID", 502);
        }
        return json(&Success { success: true }, 200);
    }

    if method == Method::Patch && path == "/api/admin/settings" {
        let input: SettingsInput = match request_json(&mut req, API_JSON_MAX_BYTES).await {
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
        if let Some(id) = input.active_theme_id.as_deref() {
            let valid = id == theme::BUILTIN_THEME_ID || db::theme_exists(&database, id).await?;
            if !valid {
                return error("活动主题不存在", 400);
            }
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
        if input
            .new_password
            .as_ref()
            .is_some_and(|value| !value.is_empty())
            && input
                .new_password_derived
                .as_deref()
                .is_none_or(|value| !valid_password_derived(value))
        {
            return error("新密码派生值无效", 400);
        }
        let enabled = input
            .turnstile_enabled
            .unwrap_or(settings.turnstile_enabled);
        let configured_site_key = input
            .turnstile_site_key
            .as_deref()
            .unwrap_or(&settings.turnstile_site_key)
            .trim();
        let configured_secret_key = submitted_secret(
            input.turnstile_secret_key.as_deref(),
            &settings.turnstile_secret_key,
        );
        let site_key = if configured_site_key.is_empty() {
            environment_turnstile_site_key.trim()
        } else {
            configured_site_key
        };
        let secret_key = if configured_secret_key.is_empty() {
            environment_turnstile_secret_key.trim()
        } else {
            configured_secret_key
        };
        if enabled && (site_key.is_empty() || secret_key.is_empty()) {
            return error("启用 Turnstile 前必须填写站点密钥和私钥", 400);
        }
        if input
            .notification_enabled
            .unwrap_or(settings.notification_enabled)
        {
            let token = submitted_secret(
                input.notification_endpoint.as_deref(),
                &settings.notification_endpoint,
            );
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
        if input.cloudflare_api_token.as_deref().is_some_and(|value| {
            value.trim() != db::SECRET_MASK && !valid_cloudflare_api_token(value)
        }) {
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
        let password_hash = if input
            .new_password
            .as_ref()
            .is_some_and(|value| !value.is_empty())
        {
            let password_derived = input.new_password_derived.as_deref().unwrap_or_default();
            let Some(salt) = random_salt() else {
                return error("运行时随机源不可用，无法保存新密码", 503);
            };
            Some(hash_password(password_derived, &salt))
        } else {
            None
        };
        db::update_settings(&database, &input, password_hash.as_deref()).await?;
        let updated = db::settings(
            &database,
            &default_name,
            default_threshold,
            default_retention,
            &default_username,
        )
        .await?;
        let token = if password_hash.is_some() {
            create_admin_jwt(&env, &updated.admin_password_hash)
        } else {
            None
        };
        return json(
            &serde_json::json!({ "settings": updated, "token": token }),
            200,
        );
    }

    if path.starts_with("/api/") {
        return error("接口不存在", 404);
    }

    if method == Method::Get {
        if let Some(preview_path) = path.strip_prefix("/__theme-preview/") {
            let (proof, relative) = preview_path.split_once('/').unwrap_or((preview_path, ""));
            let Some(theme_id) =
                verify_theme_preview_proof(proof, &env, &settings.admin_password_hash)
            else {
                return secure_public_response(Response::error(
                    "Theme preview link has expired",
                    403,
                )?);
            };
            let Some(url) = db::theme_url(&database, &theme_id).await? else {
                return secure_public_response(Response::error("Theme not found", 404)?);
            };
            let prefix = format!("/__theme-preview/{proof}");
            return remote_theme_preview_response(relative, &url, &prefix).await;
        }
    }

    if settings.active_theme_id != theme::BUILTIN_THEME_ID {
        if let Some(url) = db::theme_url(&database, &settings.active_theme_id).await? {
            if let Some(response) = remote_theme_response(&path, &url).await? {
                return Ok(response);
            }
        }
    }
    let response = env.assets("ASSETS")?.fetch_request(req).await?;
    secure_public_response(mutable_response(response)?)
}

#[event(fetch, respond_with_errors)]
async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    let method = req.method();
    let path = req.path();
    let request_url = req.url().ok();
    let origin = req.headers().get("Origin").ok().flatten();

    if path.starts_with("/api/")
        && origin
            .as_deref()
            .zip(request_url.as_ref())
            .is_some_and(|(origin, url)| !same_origin(origin, url))
    {
        return error("请求来源未获授权", 403);
    }

    if let Some(binding) = rate_limit_binding(method, &path) {
        let key = rate_limit_key(&req);
        if !request_within_rate_limit(&env, binding, key).await {
            return rate_limited();
        }
    }

    match handle(req, env, ctx).await {
        Ok(response) => Ok(response),
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
    let default_name = env_text(&env, "SITE_NAME", "NodeFlare");
    let default_threshold = env_number(&env, "OFFLINE_THRESHOLD_SECONDS", 180).clamp(30, 3600);
    let default_retention = env_number(&env, "HISTORY_RETENTION_DAYS", 30).clamp(1, 365);
    let default_username = env_text(&env, "ADMIN_USERNAME", "admin");
    let settings = db::settings(
        &database,
        &default_name,
        default_threshold,
        default_retention,
        &default_username,
    )
    .await;
    let retention = match &settings {
        Ok(settings) => settings.history_retention_days,
        Err(_) => default_retention,
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
        allow_on_rate_limit_failure, history_cache_key, rate_limit_binding, same_origin,
        submitted_secret, valid_cloudflare_account_id, valid_cloudflare_api_token,
        valid_password_derived, valid_ping_target, validate_latency_task, validate_server,
        ADMIN_HTML, ADMIN_SCRIPT, ADMIN_STYLE,
    };
    use crate::{
        db::SECRET_MASK,
        models::{AgentReport, LatencyTaskInput, ServerInput, SettingsInput},
    };
    use worker::{Method, Url};

    #[test]
    fn assigns_rate_limits_by_route() {
        assert_eq!(
            rate_limit_binding(Method::Post, "/api/admin/login"),
            Some("AUTH_RATE_LIMITER")
        );
        assert_eq!(
            rate_limit_binding(Method::Post, "/api/agent/report"),
            Some("AGENT_RATE_LIMITER")
        );
        assert_eq!(
            rate_limit_binding(Method::Get, "/api/history/node-a"),
            Some("API_RATE_LIMITER")
        );
        assert_eq!(rate_limit_binding(Method::Get, "/"), None);
        assert!(!allow_on_rate_limit_failure("AUTH_RATE_LIMITER"));
        assert!(allow_on_rate_limit_failure("API_RATE_LIMITER"));
    }

    #[test]
    fn builds_versioned_history_cache_keys() {
        let request_url =
            Url::parse("https://status.example/api/history/node-a?hours=1").expect("request URL");
        assert_eq!(
            history_cache_key(&request_url, "metrics", "node-a", 24, 7),
            "https://status.example/__nodeflare-cache/metrics/node-a?hours=24&version=7"
        );
        assert!(same_origin("https://status.example", &request_url));
        assert!(!same_origin("https://other.example", &request_url));
    }

    #[test]
    fn requires_the_current_server_schema() {
        assert!(
            serde_json::from_value::<ServerInput>(serde_json::json!({ "name": "node" })).is_err()
        );
        let mut server: ServerInput = serde_json::from_value(serde_json::json!({
            "name": "node",
            "region": "",
            "group_name": "默认",
            "tags": "",
            "note": "",
            "hidden": false,
            "expires_at": null,
            "traffic_limit": 0,
            "traffic_limit_type": "sum",
            "price": 0,
            "billing_cycle": 30,
            "currency": "CNY",
            "auto_renewal": false,
            "public_remark": "",
            "network_interface": "",
            "reset_day": 1,
            "report_interval": 60,
            "collect_interval": 5,
            "rx_correction": 0,
            "tx_correction": 0,
            "offline_notify_disabled": false,
            "auto_update": true
        }))
        .expect("current server schema");
        assert_eq!(server.price, 0.0);
        assert!(server.auto_update);
        assert_eq!(validate_server(&server), None);

        server.price = -1.0;
        assert_eq!(validate_server(&server), None);
        server.price = -2.0;
        assert_eq!(validate_server(&server), Some("价格无效"));
    }

    #[test]
    fn rejects_incomplete_or_unknown_protocol_fields() {
        assert!(serde_json::from_value::<AgentReport>(serde_json::json!({
            "timestamp": 1,
            "cpu": 1
        }))
        .is_err());
        assert!(serde_json::from_value::<SettingsInput>(serde_json::json!({
            "unexpected": true
        }))
        .is_err());
    }

    #[test]
    fn validates_latency_targets() {
        for target in [
            "gd-ct-dualstack.ip.zstaticcdn.com",
            "1.1.1.1",
            "example.com:443",
        ] {
            assert!(valid_ping_target(target), "expected valid target: {target}");
        }

        for target in [
            "",
            "https://example.com",
            "example.com/path",
            "example.com:0",
            "example.com:65536",
            "example.com:abc",
            "999.1.1.1",
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "localhost",
            "router.local",
            "metadata.google.internal",
            "Metadata.Google.INTERNAL",
            "router_1.example.com",
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
        assert_eq!(submitted_secret(Some(SECRET_MASK), "stored"), "stored");
        assert_eq!(
            submitted_secret(Some("replacement"), "stored"),
            "replacement"
        );
        assert_eq!(submitted_secret(Some(""), "stored"), "");
    }

    #[test]
    fn validates_client_password_derivations() {
        assert!(valid_password_derived(&"a1".repeat(32)));
        assert!(!valid_password_derived(&"A1".repeat(32)));
        assert!(!valid_password_derived(&"a1".repeat(31)));
        assert!(!valid_password_derived(&"zz".repeat(32)));
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
