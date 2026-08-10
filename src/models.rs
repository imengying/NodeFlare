use serde::{Deserialize, Serialize};

fn default_group() -> String {
    "默认".to_string()
}

fn default_price() -> f64 {
    -2.0
}

fn default_billing_cycle() -> i64 {
    30
}

fn default_currency() -> String {
    "CNY".to_string()
}

fn default_traffic_limit_type() -> String {
    "sum".to_string()
}

fn default_reset_day() -> i64 {
    1
}

fn default_report_interval() -> i64 {
    60
}

fn default_collect_interval() -> i64 {
    5
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub turnstile_token: String,
}

#[derive(Debug, Deserialize)]
pub struct TurnstileVerifyRequest {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct ThemePreviewInput {
    pub theme_url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerInput {
    pub name: String,
    #[serde(default)]
    pub region: String,
    #[serde(default = "default_group")]
    pub group_name: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub traffic_limit: i64,
    #[serde(default = "default_traffic_limit_type")]
    pub traffic_limit_type: String,
    #[serde(default = "default_price")]
    pub price: f64,
    #[serde(default = "default_billing_cycle")]
    pub billing_cycle: i64,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub auto_renewal: bool,
    #[serde(default)]
    pub public_remark: String,
    #[serde(default)]
    pub network_interface: String,
    #[serde(default = "default_reset_day")]
    pub reset_day: i64,
    #[serde(default = "default_report_interval")]
    pub report_interval: i64,
    #[serde(default = "default_collect_interval")]
    pub collect_interval: i64,
    #[serde(default)]
    pub rx_correction: i64,
    #[serde(default)]
    pub tx_correction: i64,
    #[serde(default)]
    pub offline_notify_disabled: bool,
    #[serde(default)]
    pub auto_update: bool,
}

#[derive(Debug, Deserialize)]
pub struct SettingsInput {
    pub site_name: Option<String>,
    pub site_description: Option<String>,
    pub site_announcement: Option<String>,
    pub favicon_url: Option<String>,
    pub locale: Option<String>,
    pub public_dashboard: Option<bool>,
    pub offline_threshold_seconds: Option<i64>,
    pub history_retention_days: Option<i64>,
    pub default_theme: Option<String>,
    pub background_url: Option<String>,
    pub theme_url: Option<String>,
    pub theme_options: Option<serde_json::Value>,
    pub show_search: Option<bool>,
    pub show_groups: Option<bool>,
    pub show_stats: Option<bool>,
    pub show_assets: Option<bool>,
    pub show_traffic: Option<bool>,
    pub show_speed: Option<bool>,
    pub show_price: Option<bool>,
    pub show_expiry: Option<bool>,
    pub show_latency: Option<bool>,
    pub show_uptime: Option<bool>,
    pub admin_username: Option<String>,
    pub new_password: Option<String>,
    pub turnstile_enabled: Option<bool>,
    pub turnstile_login_enabled: Option<bool>,
    pub turnstile_site_key: Option<String>,
    pub turnstile_secret_key: Option<String>,
    pub notification_enabled: Option<bool>,
    pub notification_endpoint: Option<String>,
    pub notification_target: Option<String>,
    pub offline_alert_minutes: Option<i64>,
    pub expiry_alert_days: Option<i64>,
    pub cloudflare_account_id: Option<String>,
    pub cloudflare_api_token: Option<String>,
    pub cors_allowed_origins: Option<String>,
    pub csp_asset_origins: Option<String>,
    pub federation_sites: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ServerOrderInput {
    pub ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ServerBatchInput {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LatencyTaskInput {
    pub name: String,
    pub task_type: String,
    pub target: String,
    pub interval_seconds: i64,
    #[serde(default)]
    pub default_enabled: bool,
    #[serde(default)]
    pub server_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AgentLatencyTask {
    pub id: String,
    pub name: String,
    pub task_type: String,
    pub target: String,
    pub interval_seconds: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentLatencyResult {
    pub task_id: String,
    pub timestamp: i64,
    pub latency_ms: f64,
    pub packet_loss: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AgentDiskMetric {
    pub name: String,
    pub mount_point: String,
    pub used: i64,
    pub total: i64,
    pub read_bps: f64,
    pub write_bps: f64,
    pub read_iops: f64,
    pub write_iops: f64,
    pub await_ms: f64,
    pub utilization: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AgentGpuMetric {
    pub model: String,
    pub usage: f64,
    pub memory_used: i64,
    pub memory_total: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentReportBatch {
    pub server_id: String,
    pub samples: Vec<AgentReport>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentReport {
    #[serde(default)]
    pub timestamp: i64,
    #[serde(default)]
    pub cpu: f64,
    #[serde(default)]
    pub load1: f64,
    #[serde(default)]
    pub load5: f64,
    #[serde(default)]
    pub load15: f64,
    #[serde(default)]
    pub mem_used: i64,
    #[serde(default)]
    pub mem_total: i64,
    #[serde(default)]
    pub swap_used: i64,
    #[serde(default)]
    pub swap_total: i64,
    #[serde(default)]
    pub disk_used: i64,
    #[serde(default)]
    pub disk_total: i64,
    #[serde(default)]
    pub net_in: f64,
    #[serde(default)]
    pub net_out: f64,
    #[serde(default)]
    pub net_rx_total: i64,
    #[serde(default)]
    pub net_tx_total: i64,
    #[serde(default)]
    pub uptime: i64,
    #[serde(default)]
    pub processes: i64,
    #[serde(default)]
    pub tcp_connections: i64,
    #[serde(default)]
    pub udp_connections: i64,
    #[serde(default)]
    pub cpu_cores: i64,
    #[serde(default)]
    pub cpu_model: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub kernel: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default)]
    pub virtualization: String,
    #[serde(default)]
    pub ipv4: String,
    #[serde(default)]
    pub ipv6: String,
    #[serde(default)]
    pub gpu_usage: f64,
    #[serde(default)]
    pub gpu_model: String,
    #[serde(default)]
    pub agent_version: String,
    #[serde(default)]
    pub disk_read_bps: f64,
    #[serde(default)]
    pub disk_write_bps: f64,
    #[serde(default)]
    pub disk_read_iops: f64,
    #[serde(default)]
    pub disk_write_iops: f64,
    #[serde(default)]
    pub disk_await_ms: f64,
    #[serde(default)]
    pub disk_utilization: f64,
    #[serde(default)]
    pub disks: Vec<AgentDiskMetric>,
    #[serde(default)]
    pub gpus: Vec<AgentGpuMetric>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub latency_results: Vec<AgentLatencyResult>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ServerView {
    pub id: String,
    pub name: String,
    pub region: String,
    pub group_name: String,
    pub tags: String,
    pub note: String,
    pub hidden: i64,
    pub sort_order: i64,
    pub expires_at: Option<i64>,
    pub traffic_limit: i64,
    pub traffic_limit_type: String,
    pub price: f64,
    pub billing_cycle: i64,
    pub currency: String,
    pub auto_renewal: i64,
    pub public_remark: String,
    pub network_interface: String,
    pub reset_day: i64,
    pub report_interval: i64,
    pub collect_interval: i64,
    pub rx_correction: i64,
    pub tx_correction: i64,
    pub offline_notify_disabled: i64,
    pub auto_update: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub timestamp: Option<i64>,
    pub cpu: Option<f64>,
    pub load1: Option<f64>,
    pub load5: Option<f64>,
    pub load15: Option<f64>,
    pub mem_used: Option<i64>,
    pub mem_total: Option<i64>,
    pub swap_used: Option<i64>,
    pub swap_total: Option<i64>,
    pub disk_used: Option<i64>,
    pub disk_total: Option<i64>,
    pub net_in: Option<f64>,
    pub net_out: Option<f64>,
    pub net_rx_total: Option<i64>,
    pub net_tx_total: Option<i64>,
    pub uptime: Option<i64>,
    pub processes: Option<i64>,
    pub tcp_connections: Option<i64>,
    pub udp_connections: Option<i64>,
    pub cpu_cores: Option<i64>,
    pub cpu_model: Option<String>,
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub arch: Option<String>,
    pub virtualization: Option<String>,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub gpu_usage: Option<f64>,
    pub gpu_model: Option<String>,
    pub agent_version: Option<String>,
    pub disk_read_bps: Option<f64>,
    pub disk_write_bps: Option<f64>,
    pub disk_read_iops: Option<f64>,
    pub disk_write_iops: Option<f64>,
    pub disk_await_ms: Option<f64>,
    pub disk_utilization: Option<f64>,
    pub disk_info: Option<String>,
    pub gpu_info: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HistoryPoint {
    pub timestamp: i64,
    pub cpu: f64,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub mem_used: i64,
    pub mem_total: i64,
    pub swap_used: i64,
    pub swap_total: i64,
    pub disk_used: i64,
    pub disk_total: i64,
    pub net_in: f64,
    pub net_out: f64,
    pub net_rx_total: i64,
    pub net_tx_total: i64,
    pub processes: i64,
    pub tcp_connections: i64,
    pub udp_connections: i64,
    pub gpu_usage: f64,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub disk_read_iops: f64,
    pub disk_write_iops: f64,
    pub disk_await_ms: f64,
    pub disk_utilization: f64,
}

#[derive(Debug, Deserialize)]
pub struct TokenHashRow {
    pub token_hash: String,
    pub hidden: i64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AgentConfigView {
    pub report_interval: i64,
    pub collect_interval: i64,
    pub network_interface: String,
    pub auto_update: i64,
    pub latest_agent_version: String,
    #[serde(default)]
    pub latency_tasks: Vec<AgentLatencyTask>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlertRuleInput {
    pub name: String,
    pub metric: String,
    pub threshold: f64,
    pub duration_minutes: i64,
    pub aggregation: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub server_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AlertRuleView {
    pub id: String,
    pub name: String,
    pub metric: String,
    pub threshold: f64,
    pub duration_minutes: i64,
    pub aggregation: String,
    pub enabled: i64,
    #[serde(default)]
    pub server_ids: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
pub struct ApiError<'a> {
    pub error: &'a str,
}
