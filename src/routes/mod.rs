pub(crate) mod admin;
pub(crate) mod agent;
pub(crate) mod public;
pub(crate) mod site;

use crate::db;
use worker::{D1Database, Env, Request, Response};

/// 每个请求加载一次的共享状态，按引用传给各路由处理函数。
pub(crate) struct RouteContext {
    pub env: Env,
    pub database: D1Database,
    pub settings: db::SettingsView,
    pub admin: bool,
    pub turnstile_verified: bool,
    pub turnstile_site_key: String,
    pub turnstile_secret_key: String,
    pub public_turnstile_enabled: bool,
    pub login_protection_enabled: bool,
    pub environment_turnstile_site_key: String,
    pub environment_turnstile_secret_key: String,
    pub default_name: String,
    pub default_threshold: i64,
    pub default_retention: i64,
    pub default_username: String,
}

/// WebSocket 升级等终端路由会按值消费 Request，未匹配时原样归还给调用方。
pub(crate) enum RouteOutcome {
    Handled(Response),
    Unmatched(Request),
}
