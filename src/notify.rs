use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use worker::{wasm_bindgen::JsValue, D1Database, Fetch, Method, Request, RequestInit, Result};

use crate::db::{self, SettingsView};

#[derive(Default, Deserialize, Serialize, PartialEq, Eq)]
struct AlertState {
    offline: HashSet<String>,
    expiry: HashSet<String>,
}

pub fn valid_config(token: &str, chat_id: &str) -> bool {
    let token = token.trim();
    !chat_id.trim().is_empty()
        && token.split_once(':').is_some_and(|(bot_id, secret)| {
            !bot_id.is_empty()
                && bot_id.chars().all(|value| value.is_ascii_digit())
                && secret.len() >= 20
                && secret
                    .chars()
                    .all(|value| value.is_ascii_alphanumeric() || matches!(value, '_' | '-'))
        })
}

fn notification_request(settings: &SettingsView, message: &str) -> Result<(String, String)> {
    let token = settings.notification_endpoint.trim();
    let chat_id = settings.notification_target.trim();
    if !valid_config(token, chat_id) {
        return Err(worker::Error::RustError(
            "请填写有效的 Telegram Bot Token 和 Chat ID".to_string(),
        ));
    }

    Ok((
        format!("https://api.telegram.org/bot{token}/sendMessage"),
        serde_json::json!({ "chat_id": chat_id, "text": message }).to_string(),
    ))
}

pub async fn send(settings: &SettingsView, message: &str) -> Result<()> {
    let (url, body) = notification_request(settings, message)?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_body(Some(JsValue::from_str(&body)));
    let request = Request::new_with_init(&url, &init)?;
    request.headers().set("Content-Type", "application/json")?;
    let response = Fetch::Request(request).send().await?;
    if !(200..300).contains(&response.status_code()) {
        return Err(worker::Error::RustError(format!(
            "通知服务返回 HTTP {}",
            response.status_code()
        )));
    }
    Ok(())
}

pub async fn check_alerts(db_conn: &D1Database, settings: &SettingsView) -> Result<()> {
    if !settings.notification_enabled || settings.notification_endpoint.trim().is_empty() {
        return Ok(());
    }

    let servers = db::list_servers(db_conn, true).await?;
    let stored = db::get_setting(db_conn, "alert_state").await?;
    let previous: AlertState = stored
        .as_deref()
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default();
    let mut current = AlertState::default();
    let current_time = worker::Date::now().as_millis() as i64 / 1000;
    let offline_after = settings.offline_alert_minutes.clamp(2, 1440) * 60;
    let mut messages = Vec::new();

    for server in &servers {
        let server_id = server.id.clone();
        let is_offline = server
            .timestamp
            .map(|timestamp| current_time - timestamp > offline_after)
            .unwrap_or(current_time - server.created_at > offline_after);
        if server.offline_notify_disabled == 0 && is_offline {
            current.offline.insert(server_id.clone());
            if !previous.offline.contains(&server_id) {
                messages.push(format!(
                    "[离线] {} 已超过 {} 分钟未上报",
                    server.name, settings.offline_alert_minutes
                ));
            }
        } else if previous.offline.contains(&server_id) {
            messages.push(format!("[恢复] {} 已恢复上报", server.name));
        }

        let mut expires_at = server.expires_at;
        if server.auto_renewal != 0 {
            if let Some(mut value) = expires_at {
                let cycle = server.billing_cycle.clamp(1, 3650) * 86_400;
                if value <= current_time + 86_400 {
                    while value <= current_time + 86_400 {
                        value += cycle;
                    }
                    db::update_expiry(db_conn, &server_id, value).await?;
                    expires_at = Some(value);
                }
            }
        }
        if settings.expiry_alert_days > 0 {
            if let Some(value) = expires_at {
                let days = (value - current_time + 86_399) / 86_400;
                if days >= 0 && days <= settings.expiry_alert_days {
                    let key = format!("{server_id}:{value}");
                    current.expiry.insert(key.clone());
                    if !previous.expiry.contains(&key) {
                        messages.push(format!("[到期] {} 剩余 {} 天", server.name, days));
                    }
                }
            }
        }
    }

    let previous_resources = db::active_alert_states(db_conn).await?;
    let mut current_resources = HashSet::new();
    let mut evaluated_resources = HashSet::new();
    let mut eligible_resources = HashSet::new();
    for rule in db::list_alert_rules(db_conn).await? {
        if rule.enabled == 0 {
            continue;
        }
        if rule.server_ids.is_empty() {
            eligible_resources.extend(
                servers
                    .iter()
                    .filter(|server| server.hidden == 0)
                    .map(|server| format!("{}:{}", rule.id, server.id)),
            );
        } else {
            eligible_resources.extend(
                rule.server_ids
                    .iter()
                    .map(|server_id| format!("{}:{server_id}", rule.id)),
            );
        }
        let metric_label = match rule.metric.as_str() {
            "cpu" => "CPU",
            "memory" => "内存",
            "disk" => "磁盘",
            "net_in" => "下行",
            "net_out" => "上行",
            _ => continue,
        };
        let unit = if matches!(rule.metric.as_str(), "net_in" | "net_out") {
            "MiB/s"
        } else {
            "%"
        };
        for value in db::alert_metric_values(db_conn, &rule, current_time).await? {
            let key = format!("{}:{}", rule.id, value.server_id);
            evaluated_resources.insert(key.clone());
            if value.value >= rule.threshold {
                current_resources.insert(key.clone());
                if !previous_resources.contains(&key) {
                    messages.push(format!(
                        "[资源] {} · {}：{} {:.1}{} >= {:.1}{}（{} 分钟{}）",
                        value.name,
                        rule.name,
                        metric_label,
                        value.value,
                        unit,
                        rule.threshold,
                        unit,
                        rule.duration_minutes,
                        if rule.aggregation == "continuous" {
                            "持续"
                        } else {
                            "平均"
                        },
                    ));
                }
            } else if previous_resources.contains(&key) {
                messages.push(format!("[恢复] {} · {} 已恢复正常", value.name, rule.name));
            }
        }
    }
    for key in &previous_resources {
        if eligible_resources.contains(key) && !evaluated_resources.contains(key) {
            current_resources.insert(key.clone());
        }
    }

    if !messages.is_empty() {
        let body = format!("NodeFlare 告警\n\n{}", messages.join("\n"));
        send(settings, &body).await?;
    }
    if previous != current {
        db::save_setting(db_conn, "alert_state", &serde_json::to_string(&current)?).await?;
    }
    db::sync_active_alert_states(db_conn, &previous_resources, &current_resources).await
}
