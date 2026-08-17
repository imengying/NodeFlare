use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
#[cfg(target_os = "windows")]
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use sysinfo::{Disks, Networks, ProcessesToUpdate, System};
use tungstenite::client::IntoClientRequest;
use tungstenite::{connect, Error as WebSocketError, Message};

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
compile_error!("nodeflare-agent supports Linux, Windows, and macOS");

const VERSION: &str = match option_env!("NODEFLARE_VERSION") {
    Some(version) if !version.is_empty() => version,
    _ => env!("CARGO_PKG_VERSION"),
};
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/imengying/NodeFlare/releases/latest";
const PROBE_ATTEMPTS: usize = 4;
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_AGENT_BINARY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LATENCY_TASKS: usize = 128;
const LATENCY_WORKERS: usize = 4;
const MAX_PENDING_LATENCY_RESULTS: usize = 4096;
const MAX_REPORT_AGE_SECONDS: i64 = 7_000;
const REPORT_RETRY_MIN: Duration = Duration::from_secs(5);
const REPORT_RETRY_MAX: Duration = Duration::from_secs(300);
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const UPDATE_RETRY_INTERVAL: Duration = Duration::from_secs(30 * 60);
const LIVE_RECONNECT_DELAY: Duration = Duration::from_secs(3);
const LIVE_QUEUE_CAPACITY: usize = 720;
// Keep each WebSocket frame comfortably below the Worker 1 MiB validation limit;
// the queue preserves the remaining samples for the next frame.
const LIVE_BATCH_CAPACITY: usize = 32;
// D1 persistence can take a few hundred milliseconds; allow the ACK to arrive
// before continuing with the configured live interval.
const LIVE_ACK_READ_TIMEOUT: Duration = Duration::from_millis(1_000);
const LIVE_HINT_READ_TIMEOUT: Duration = Duration::from_millis(10);
const LIVE_REPORT_DIVISOR: u64 = 15;
const BASIC_INFO_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const CLOCK_CALIBRATION_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const CLOCK_CALIBRATION_MIN_CHANGE_MS: i64 = 20_000;

type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
struct RuntimeConfig {
    token: String,
    endpoint: String,
    report_interval: u64,
    collect_interval: u64,
    network_interface: String,
    agent_mirror: String,
    auto_update: bool,
    latency_tasks: Vec<LatencyTask>,
}

#[derive(Debug, Parser)]
#[command(name = "nodeflare", version = VERSION, about = "NodeFlare monitoring agent")]
struct CliOptions {
    /// NodeFlare endpoint
    #[arg(short = 'e', value_name = "URL")]
    endpoint: String,
    /// Agent token
    #[arg(short = 't', value_name = "TOKEN")]
    token: String,
    /// Initial report interval in seconds (15-3600)
    #[arg(short = 'i', value_name = "SECONDS", default_value_t = 60)]
    interval: u64,
    /// Submit one report and exit
    #[arg(long, conflicts_with = "collect")]
    once: bool,
    /// Print one metric sample and exit
    #[arg(long, conflicts_with = "once")]
    collect: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LatencyTask {
    id: String,
    name: String,
    task_type: String,
    target: String,
    port: Option<i64>,
    interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
struct LatencyResult {
    task_id: String,
    timestamp: i64,
    latency_ms: f64,
    packet_loss: f64,
}

struct LatencyExecutor {
    task_tx: mpsc::SyncSender<LatencyTask>,
    result_rx: mpsc::Receiver<LatencyResult>,
    in_flight: HashSet<String>,
}

impl LatencyExecutor {
    fn new() -> Result<Self> {
        let (task_tx, task_rx) = mpsc::sync_channel::<LatencyTask>(MAX_LATENCY_TASKS);
        let (result_tx, result_rx) = mpsc::channel();
        let task_rx = Arc::new(Mutex::new(task_rx));

        for index in 0..LATENCY_WORKERS {
            let task_rx = Arc::clone(&task_rx);
            let result_tx = result_tx.clone();
            thread::Builder::new()
                .name(format!("nodeflare-latency-{index}"))
                .spawn(move || loop {
                    let task = match task_rx.lock() {
                        Ok(receiver) => receiver.try_recv(),
                        Err(_) => return,
                    };
                    match task {
                        Ok(task) => {
                            if result_tx.send(execute_latency_task(task)).is_err() {
                                return;
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            thread::sleep(Duration::from_millis(25));
                        }
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    };
                })?;
        }

        Ok(Self {
            task_tx,
            result_rx,
            in_flight: HashSet::new(),
        })
    }

    fn enqueue(&mut self, task: LatencyTask) -> bool {
        if self.in_flight.contains(&task.id) {
            return false;
        }
        let task_id = task.id.clone();
        match self.task_tx.try_send(task) {
            Ok(()) => {
                self.in_flight.insert(task_id);
                true
            }
            Err(mpsc::TrySendError::Full(_)) => false,
            Err(mpsc::TrySendError::Disconnected(_)) => false,
        }
    }

    fn drain(&mut self) -> Vec<LatencyResult> {
        let results = self.result_rx.try_iter().collect::<Vec<_>>();
        for result in &results {
            self.in_flight.remove(&result.task_id);
        }
        results
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteConfig {
    report_interval: u64,
    collect_interval: u64,
    network_interface: String,
    agent_mirror: String,
    auto_update: bool,
    latency_tasks: Vec<LatencyTask>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

struct SubmitResult {
    config: Option<RemoteConfig>,
    config_hash: String,
    clock_offset_ms: Option<i64>,
}

#[derive(Debug, Default)]
struct ClockCalibration {
    offset_ms: i64,
    calibrated_at: Option<Instant>,
}

impl ClockCalibration {
    fn observe(&mut self, offset_ms: i64) -> bool {
        let should_update = self.calibrated_at.is_none()
            || self
                .calibrated_at
                .is_some_and(|at| at.elapsed() >= CLOCK_CALIBRATION_MAX_AGE)
            || self.offset_ms.abs_diff(offset_ms) >= CLOCK_CALIBRATION_MIN_CHANGE_MS as u64;
        if should_update {
            self.offset_ms = offset_ms;
            self.calibrated_at = Some(Instant::now());
        }
        should_update
    }
}

type SharedClock = Arc<Mutex<ClockCalibration>>;

struct LiveSender {
    pending: Arc<(Mutex<VecDeque<Report>>, Condvar)>,
    send_interval: Arc<Mutex<Duration>>,
    healthy: Arc<AtomicBool>,
}

impl LiveSender {
    fn start(config: &RuntimeConfig, clock: SharedClock) -> Result<Self> {
        let pending = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let send_interval = Arc::new(Mutex::new(live_batch_interval(
            config.report_interval,
            config.collect_interval,
        )));
        let healthy = Arc::new(AtomicBool::new(false));
        let endpoint = live_endpoint(&config.endpoint)?;
        let token = config.token.clone();
        let sender_state = Arc::clone(&pending);
        let sender_interval = Arc::clone(&send_interval);
        let sender_healthy = Arc::clone(&healthy);
        thread::Builder::new()
            .name("nodeflare-live".to_string())
            .spawn(move || {
                live_sender_loop(
                    &endpoint,
                    &token,
                    sender_state,
                    sender_interval,
                    sender_healthy,
                    clock,
                )
            })?;
        Ok(Self {
            pending,
            send_interval,
            healthy,
        })
    }

    fn send(&self, report: &Report) {
        let (pending, ready) = &*self.pending;
        if let Ok(mut pending) = pending.lock() {
            if pending.len() >= LIVE_QUEUE_CAPACITY {
                pending.pop_front();
            }
            pending.push_back(report.clone());
            ready.notify_one();
        }
    }

    fn set_send_interval(&self, report_interval: u64, collect_interval: u64) {
        if let Ok(mut interval) = self.send_interval.lock() {
            *interval = live_batch_interval(report_interval, collect_interval);
        }
        self.pending.1.notify_one();
    }

    fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }
}

fn live_batch_interval(report_interval: u64, collect_interval: u64) -> Duration {
    let seconds = report_interval
        .clamp(15, 3600)
        .div_ceil(LIVE_REPORT_DIVISOR)
        .clamp(1, 60)
        .max(collect_interval.clamp(1, 60));
    Duration::from_secs(seconds)
}

#[derive(Debug, Serialize)]
struct ReportBatch<'a> {
    samples: &'a [Report],
}

#[derive(Debug, Default, Clone, Serialize)]
struct DiskMetric {
    name: String,
    mount_point: String,
    used: i64,
    total: i64,
    read_bps: f64,
    write_bps: f64,
    read_iops: f64,
    write_iops: f64,
    await_ms: f64,
    utilization: f64,
}

#[derive(Debug, Clone, Serialize)]
struct GpuMetric {
    model: String,
    usage: Option<f64>,
    memory_used: i64,
    memory_total: i64,
}

#[derive(Debug, Default, Clone, Serialize)]
struct Report {
    timestamp: i64,
    cpu: f64,
    load1: f64,
    load5: f64,
    load15: f64,
    mem_used: i64,
    mem_total: i64,
    swap_used: i64,
    swap_total: i64,
    disk_used: i64,
    disk_total: i64,
    net_in: f64,
    net_out: f64,
    net_rx_total: i64,
    net_tx_total: i64,
    uptime: i64,
    processes: i64,
    tcp_connections: i64,
    udp_connections: i64,
    cpu_cores: i64,
    cpu_model: String,
    os: String,
    kernel: String,
    arch: String,
    virtualization: String,
    gpu_usage: f64,
    gpu_model: String,
    agent_version: String,
    disk_read_bps: f64,
    disk_write_bps: f64,
    disk_read_iops: f64,
    disk_write_iops: f64,
    disk_await_ms: f64,
    disk_utilization: f64,
    disks: Vec<DiskMetric>,
    gpus: Vec<GpuMetric>,
    latency_results: Vec<LatencyResult>,
}

#[derive(Debug, Default, Clone)]
struct BasicMetrics {
    cpu_cores: i64,
    cpu_model: String,
    os: String,
    kernel: String,
    arch: String,
    virtualization: String,
    gpu_usage: f64,
    gpu_model: String,
    gpus: Vec<GpuMetric>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default, Clone, Copy)]
struct CpuSample {
    total: u64,
    idle: u64,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default, Clone, Copy)]
struct IoSample {
    rx: u64,
    tx: u64,
    read_ops: u64,
    read_sectors: u64,
    write_ops: u64,
    write_sectors: u64,
    read_millis: u64,
    write_millis: u64,
    io_millis: u64,
}

#[cfg(target_os = "linux")]
fn text(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn command(name: &str, args: &[&str]) -> String {
    Command::new(name)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn cpu_sample() -> CpuSample {
    let line = text("/proc/stat").lines().next().unwrap_or("").to_string();
    let values = line
        .split_whitespace()
        .skip(1)
        .filter_map(|value| value.parse::<u64>().ok())
        .collect::<Vec<_>>();
    CpuSample {
        total: values.iter().sum(),
        idle: values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0),
    }
}

fn selected_interface(name: &str, filter: &str) -> bool {
    if filter.trim().is_empty() {
        name != "lo" && !name.to_ascii_lowercase().contains("loopback")
    } else {
        filter.split(',').any(|value| value.trim() == name)
    }
}

#[cfg(target_os = "linux")]
fn disk_device(name: &str) -> bool {
    ((name.starts_with("sd") || name.starts_with("vd") || name.starts_with("xvd"))
        && name
            .chars()
            .last()
            .is_some_and(|value| value.is_ascii_alphabetic()))
        || (name.starts_with("nvme") && name.contains('n') && !name.contains('p'))
        || (name.starts_with("mmcblk") && !name.contains('p'))
}

#[cfg(target_os = "linux")]
fn io_sample(filter: &str) -> IoSample {
    let mut sample = IoSample::default();
    for line in text("/proc/net/dev")
        .lines()
        .filter(|line| line.contains(':'))
    {
        let Some((name, values)) = line.split_once(':') else {
            continue;
        };
        if !selected_interface(name.trim(), filter) {
            continue;
        }
        let fields = values.split_whitespace().collect::<Vec<_>>();
        sample.rx = sample
            .rx
            .saturating_add(fields.first().and_then(|v| v.parse().ok()).unwrap_or(0));
        sample.tx = sample
            .tx
            .saturating_add(fields.get(8).and_then(|v| v.parse().ok()).unwrap_or(0));
    }
    for line in text("/proc/diskstats").lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 13 || !disk_device(fields[2]) {
            continue;
        }
        sample.read_ops = sample
            .read_ops
            .saturating_add(fields[3].parse().unwrap_or(0));
        sample.read_sectors = sample
            .read_sectors
            .saturating_add(fields[5].parse().unwrap_or(0));
        sample.write_ops = sample
            .write_ops
            .saturating_add(fields[7].parse().unwrap_or(0));
        sample.write_sectors = sample
            .write_sectors
            .saturating_add(fields[9].parse().unwrap_or(0));
        sample.read_millis = sample
            .read_millis
            .saturating_add(fields[6].parse().unwrap_or(0));
        sample.write_millis = sample
            .write_millis
            .saturating_add(fields[10].parse().unwrap_or(0));
        sample.io_millis = sample
            .io_millis
            .saturating_add(fields[12].parse().unwrap_or(0));
    }
    sample
}

#[cfg(target_os = "linux")]
fn mem_value(contents: &str, key: &str) -> i64 {
    contents
        .lines()
        .find(|line| line.starts_with(key))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        .saturating_mul(1024)
}

#[cfg(target_os = "linux")]
fn file_line_count(path: &str) -> i64 {
    text(path).lines().skip(1).count() as i64
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn connection_counts_from_netstat(output: &str) -> (i64, i64) {
    output.lines().fold((0_i64, 0_i64), |(tcp, udp), line| {
        let protocol = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if protocol.starts_with("tcp") {
            (tcp.saturating_add(1), udp)
        } else if protocol.starts_with("udp") {
            (tcp, udp.saturating_add(1))
        } else {
            (tcp, udp)
        }
    })
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn connection_counts() -> (i64, i64) {
    Command::new("netstat")
        .args(["-an"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| connection_counts_from_netstat(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn disk_usage() -> Vec<DiskMetric> {
    let output = command(
        "df",
        &[
            "-B1",
            "-l",
            "--output=source,target,size,used",
            "-x",
            "tmpfs",
            "-x",
            "devtmpfs",
        ],
    );
    output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 4 {
                return None;
            }
            let total = fields[2].parse::<i64>().ok()?;
            let used = fields[3].parse::<i64>().ok()?;
            (total > 0).then(|| DiskMetric {
                name: fields[0].to_string(),
                mount_point: fields[1].to_string(),
                used,
                total,
                ..DiskMetric::default()
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn os_name() -> String {
    text("/etc/os-release")
        .lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim_matches('"').to_string())
        .unwrap_or_else(|| command("uname", &["-s"]))
}

#[cfg(target_os = "linux")]
fn cpu_model() -> String {
    text("/proc/cpuinfo")
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            matches!(key.trim(), "model name" | "Hardware").then(|| value.trim().to_string())
        })
        .unwrap_or_default()
}

fn valid_probe_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 50 || host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    let labels = host.split('.').collect::<Vec<_>>();
    let ipv4_like = labels.len() == 4
        && labels
            .iter()
            .all(|label| !label.is_empty() && label.chars().all(|c| c.is_ascii_digit()));
    if ipv4_like {
        return host
            .parse()
            .is_ok_and(|address| is_public_probe_ip(IpAddr::V4(address)));
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

fn is_public_probe_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
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
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_public_probe_ip(IpAddr::V4(mapped));
            }
            address.segments()[0] & 0xe000 == 0x2000
        }
    }
}

fn resolve_public_probe_address(host: &str, port: u16) -> Option<SocketAddr> {
    let addresses = (host, port)
        .to_socket_addrs()
        .ok()?
        .filter(|address| is_public_probe_ip(address.ip()))
        .collect::<Vec<_>>();
    addresses
        .iter()
        .find(|address| address.is_ipv4())
        .or_else(|| addresses.first())
        .copied()
}

fn parse_probe_target(value: &str, port: Option<u16>) -> Option<(String, u16)> {
    let raw = value.trim();
    if raw.is_empty()
        || raw.len() > 60
        || raw.contains("://")
        || raw
            .chars()
            .any(|character| character.is_whitespace() || "/@?#\\[]".contains(character))
        || raw.contains(':')
    {
        return None;
    }
    let port = port.unwrap_or(443);
    (port > 0 && valid_probe_host(raw)).then(|| (raw.to_ascii_lowercase(), port))
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

fn tcp_latency_probe(target: &str, port: Option<i64>) -> (f64, f64) {
    if target.trim().is_empty() {
        return (-1.0, -1.0);
    }
    let Some(port) = port.and_then(|value| u16::try_from(value).ok()) else {
        return (-1.0, 100.0);
    };
    let Some((host, port)) = parse_probe_target(target, Some(port)) else {
        return (-1.0, 100.0);
    };
    let Some(address) = resolve_public_probe_address(&host, port) else {
        return (-1.0, 100.0);
    };
    tcp_latency_probe_address(&address)
}

fn tcp_latency_probe_address(address: &SocketAddr) -> (f64, f64) {
    let mut latencies = Vec::with_capacity(PROBE_ATTEMPTS);
    for _ in 0..PROBE_ATTEMPTS {
        let started = Instant::now();
        if TcpStream::connect_timeout(address, PROBE_TIMEOUT).is_ok() {
            latencies.push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let loss = (PROBE_ATTEMPTS - latencies.len()) as f64 * 100.0 / PROBE_ATTEMPTS as f64;
    if latencies.is_empty() {
        (-1.0, loss)
    } else {
        (median(&mut latencies), loss)
    }
}

fn ping_latency(output: &str) -> Option<f64> {
    let marker = output
        .find("time=")
        .map(|index| (index + 5, false))
        .or_else(|| output.find("time<").map(|index| (index + 5, true)))?;
    if marker.1 {
        return Some(0.5);
    }
    let value = output[marker.0..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect::<String>();
    value.parse().ok()
}

fn icmp_latency_probe(target: &str) -> (f64, f64) {
    let host = target.trim();
    let Some((host, _)) = parse_probe_target(host, None) else {
        return (-1.0, 100.0);
    };
    let Some(address) = resolve_public_probe_address(&host, 443) else {
        return (-1.0, 100.0);
    };
    let destination = address.ip().to_string();
    let mut latencies = Vec::with_capacity(PROBE_ATTEMPTS);
    for _ in 0..PROBE_ATTEMPTS {
        let started = Instant::now();
        let mut ping = Command::new("ping");
        #[cfg(target_os = "linux")]
        ping.args(["-n", "-c", "1", "-W", "1", destination.as_str()]);
        #[cfg(target_os = "macos")]
        ping.args(["-n", "-c", "1", "-W", "1000", destination.as_str()]);
        #[cfg(target_os = "windows")]
        ping.args(["-n", "1", "-w", "1000", destination.as_str()]);
        let output = ping.output();
        if let Ok(output) = output {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                latencies.push(
                    ping_latency(&stdout)
                        .unwrap_or_else(|| started.elapsed().as_secs_f64() * 1000.0),
                );
            }
        }
    }
    let loss = (PROBE_ATTEMPTS - latencies.len()) as f64 * 100.0 / PROBE_ATTEMPTS as f64;
    if latencies.is_empty() {
        (-1.0, loss)
    } else {
        (median(&mut latencies), loss)
    }
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn unix_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn clock_offset_from_http_date(value: &str, started_ms: i64, ended_ms: i64) -> Option<i64> {
    let server_ms: i64 = httpdate::parse_http_date(value)
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()?;
    let midpoint_ms = started_ms.saturating_add(ended_ms.saturating_sub(started_ms) / 2);
    Some(server_ms.saturating_sub(midpoint_ms))
}

fn clock_offset_from_server_seconds(value: &str, started_ms: i64, ended_ms: i64) -> Option<i64> {
    let server_ms = value.trim().parse::<i64>().ok()?.saturating_mul(1_000);
    let midpoint_ms = started_ms.saturating_add(ended_ms.saturating_sub(started_ms) / 2);
    Some(server_ms.saturating_sub(midpoint_ms))
}

fn corrected_timestamp(timestamp: i64, offset_ms: i64) -> i64 {
    timestamp
        .saturating_mul(1_000)
        .saturating_add(offset_ms)
        .div_euclid(1_000)
}

fn shared_clock_offset(clock: &SharedClock) -> i64 {
    clock.lock().map_or(0, |calibration| calibration.offset_ms)
}

fn observe_clock(clock: &SharedClock, offset_ms: Option<i64>) {
    let Some(offset_ms) = offset_ms else {
        return;
    };
    if let Ok(mut calibration) = clock.lock() {
        calibration.observe(offset_ms);
    }
}

fn execute_latency_task(task: LatencyTask) -> LatencyResult {
    let (latency_ms, packet_loss) = match task.task_type.as_str() {
        "tcp" => tcp_latency_probe(&task.target, task.port),
        "icmp" => icmp_latency_probe(&task.target),
        _ => (-1.0, 100.0),
    };
    LatencyResult {
        task_id: task.id,
        timestamp: unix_timestamp(),
        latency_ms,
        packet_loss,
    }
}

fn nvidia_gpu_info() -> Vec<GpuMetric> {
    let gpu = command(
        "nvidia-smi",
        &[
            "--query-gpu=utilization.gpu,name,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ],
    );
    gpu.lines()
        .filter_map(|line| {
            let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
            if fields.len() < 4 {
                return None;
            }
            if fields[1].is_empty() {
                return None;
            }
            Some(GpuMetric {
                usage: fields[0].parse().ok(),
                model: fields[1].to_string(),
                memory_used: fields[2]
                    .parse::<i64>()
                    .unwrap_or(0)
                    .saturating_mul(1024 * 1024),
                memory_total: fields[3]
                    .parse::<i64>()
                    .unwrap_or(0)
                    .saturating_mul(1024 * 1024),
            })
        })
        .collect()
}

fn basic_gpu_metrics(names: impl IntoIterator<Item = String>) -> Vec<GpuMetric> {
    let mut seen = HashSet::new();
    names
        .into_iter()
        .filter_map(|name| {
            let name = name.split(" (rev ").next().unwrap_or(&name).trim();
            let lower = name.to_ascii_lowercase();
            if name.is_empty()
                || [
                    "sensor hub",
                    "management engine",
                    "ethernet",
                    "wireless",
                    "audio controller",
                    "usb controller",
                    "sata controller",
                    "virtio",
                    "vmware",
                    "qxl",
                    "hyper-v",
                    "cirrus",
                    "microsoft basic display",
                ]
                .iter()
                .any(|pattern| lower.contains(pattern))
                || !seen.insert(name.to_string())
            {
                return None;
            }
            Some(GpuMetric {
                model: name.to_string(),
                usage: None,
                memory_used: 0,
                memory_total: 0,
            })
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn parse_lspci_gpu_names(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("vga compatible controller")
                || lower.contains("3d controller")
                || lower.contains("display controller")
        })
        .filter_map(|line| line.split_once(": ").map(|(_, name)| name.to_string()))
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn gpu_name_from_uevent(output: &str) -> Option<String> {
    let driver = output
        .lines()
        .find_map(|line| line.strip_prefix("DRIVER="))?;
    let pci_class = output
        .lines()
        .find_map(|line| line.strip_prefix("PCI_CLASS="))
        .unwrap_or_default();
    if u32::from_str_radix(pci_class, 16)
        .ok()
        .map(|class| class >> 16)
        != Some(3)
    {
        return None;
    }
    match driver {
        "i915" | "xe" => Some("Intel Integrated Graphics".to_string()),
        "amdgpu" | "radeon" => Some("AMD Radeon Graphics".to_string()),
        "nvidia" | "nouveau" => Some("NVIDIA GPU".to_string()),
        "msm" | "msm_drm" => Some("Qualcomm Adreno GPU".to_string()),
        "panfrost" | "lima" => Some("ARM Mali GPU".to_string()),
        "vc4" | "v3d" => Some("Broadcom VideoCore Graphics".to_string()),
        "virtio-pci" | "virtio_gpu" | "bochs-drm" | "qxl" | "vmwgfx" | "cirrus" | "vboxvideo"
        | "hyperv_fb" | "simpledrm" | "simplefb" => None,
        other if !other.is_empty() => Some(format!("GPU ({other})")),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn sysfs_gpu_names() -> Vec<String> {
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix("card").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
            })
        })
        .filter_map(|entry| gpu_name_from_uevent(&text(entry.path().join("device/uevent"))))
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn parse_system_profiler_gpu_names(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("Chipset Model:")
                .map(|name| name.trim().to_string())
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn basic_gpu_info() -> Vec<GpuMetric> {
    let names = parse_lspci_gpu_names(&command("lspci", &[]));
    basic_gpu_metrics(if names.is_empty() {
        sysfs_gpu_names()
    } else {
        names
    })
}

#[cfg(target_os = "windows")]
fn basic_gpu_info() -> Vec<GpuMetric> {
    basic_gpu_metrics(
        command(
            "powershell.exe",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name }",
            ],
        )
        .lines()
        .map(str::to_string),
    )
}

#[cfg(target_os = "macos")]
fn basic_gpu_info() -> Vec<GpuMetric> {
    basic_gpu_metrics(parse_system_profiler_gpu_names(&command(
        "system_profiler",
        &["SPDisplaysDataType"],
    )))
}

fn gpu_info() -> Vec<GpuMetric> {
    let detailed = nvidia_gpu_info();
    if detailed.is_empty() {
        basic_gpu_info()
    } else {
        detailed
    }
}

fn average_gpu_usage(gpus: &[GpuMetric]) -> f64 {
    let usages = gpus.iter().filter_map(|gpu| gpu.usage).collect::<Vec<_>>();
    if usages.is_empty() {
        0.0
    } else {
        usages.iter().sum::<f64>() / usages.len() as f64
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn per_second(value: u64, elapsed_seconds: f64) -> f64 {
    value as f64 / elapsed_seconds.max(0.001)
}

fn valid_sample_schedule(report_interval: u64, collect_interval: u64) -> bool {
    (15..=3600).contains(&report_interval)
        && (1..=60).contains(&collect_interval)
        && collect_interval <= report_interval
        && report_interval.div_ceil(collect_interval) <= LIVE_QUEUE_CAPACITY as u64
}

fn advance_deadline(mut deadline: Instant, interval: Duration, now: Instant) -> Instant {
    while deadline <= now {
        deadline += interval;
    }
    deadline
}

#[cfg(target_os = "linux")]
struct Collector {
    previous_cpu: CpuSample,
    previous_io: IoSample,
    previous_at: Instant,
    network_interface: String,
    basic: BasicMetrics,
    basic_at: Instant,
}

#[cfg(target_os = "linux")]
impl Collector {
    fn new(config: &RuntimeConfig) -> Self {
        let mut collector = Self {
            previous_cpu: cpu_sample(),
            previous_io: io_sample(&config.network_interface),
            previous_at: Instant::now(),
            network_interface: config.network_interface.clone(),
            basic: BasicMetrics::default(),
            basic_at: Instant::now(),
        };
        collector.refresh_basic();
        collector
    }

    fn refresh_basic(&mut self) {
        let gpus = gpu_info();
        self.basic = BasicMetrics {
            cpu_cores: thread::available_parallelism()
                .map(|value| value.get() as i64)
                .unwrap_or(1),
            cpu_model: cpu_model(),
            os: os_name(),
            kernel: command("uname", &["-r"]),
            arch: env::consts::ARCH.to_string(),
            virtualization: command("systemd-detect-virt", &[]),
            gpu_usage: average_gpu_usage(&gpus),
            gpu_model: gpus
                .iter()
                .map(|gpu| gpu.model.as_str())
                .collect::<Vec<_>>()
                .join(" · "),
            gpus,
        };
        self.basic_at = Instant::now();
    }

    fn collect(
        &mut self,
        config: &RuntimeConfig,
        latency_results: Vec<LatencyResult>,
        timestamp: i64,
    ) -> Report {
        let sampled_at = Instant::now();
        let elapsed = sampled_at
            .saturating_duration_since(self.previous_at)
            .as_secs_f64()
            .max(0.001);
        let cpu_now = cpu_sample();
        let io_now = io_sample(&config.network_interface);
        let total = cpu_now.total.saturating_sub(self.previous_cpu.total);
        let idle = cpu_now.idle.saturating_sub(self.previous_cpu.idle);
        let cpu = if total == 0 {
            0.0
        } else {
            (total.saturating_sub(idle)) as f64 * 100.0 / total as f64
        };
        let interface_changed = self.network_interface != config.network_interface;
        let io_before = if interface_changed {
            io_now
        } else {
            self.previous_io
        };
        self.previous_cpu = cpu_now;
        self.previous_io = io_now;
        self.previous_at = sampled_at;
        self.network_interface.clone_from(&config.network_interface);

        if self.basic_at.elapsed() >= BASIC_INFO_REFRESH_INTERVAL {
            self.refresh_basic();
        }

        let mem = text("/proc/meminfo");
        let mem_total = mem_value(&mem, "MemTotal:");
        let mem_used = mem_total.saturating_sub(mem_value(&mem, "MemAvailable:"));
        let swap_total = mem_value(&mem, "SwapTotal:");
        let swap_used = swap_total.saturating_sub(mem_value(&mem, "SwapFree:"));
        let loads = text("/proc/loadavg")
            .split_whitespace()
            .take(3)
            .filter_map(|value| value.parse::<f64>().ok())
            .collect::<Vec<_>>();
        let uptime = text("/proc/uptime")
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.0) as i64;
        let processes = fs::read_dir("/proc")
            .map(|items| {
                items
                    .filter_map(|item| item.ok())
                    .filter(|item| {
                        item.file_name()
                            .to_string_lossy()
                            .chars()
                            .all(|ch| ch.is_ascii_digit())
                    })
                    .count() as i64
            })
            .unwrap_or(0);
        let disks = disk_usage();
        let disk_used = disks.iter().map(|disk| disk.used).sum();
        let disk_total = disks.iter().map(|disk| disk.total).sum();
        let tcp_connections = file_line_count("/proc/net/tcp") + file_line_count("/proc/net/tcp6");
        let udp_connections = file_line_count("/proc/net/udp") + file_line_count("/proc/net/udp6");
        let read_ops = io_now.read_ops.saturating_sub(io_before.read_ops);
        let write_ops = io_now.write_ops.saturating_sub(io_before.write_ops);
        let total_ops = read_ops.saturating_add(write_ops);
        let io_wait = io_now
            .read_millis
            .saturating_sub(io_before.read_millis)
            .saturating_add(io_now.write_millis.saturating_sub(io_before.write_millis));
        let elapsed_ms = elapsed * 1000.0;

        Report {
            timestamp,
            cpu,
            load1: loads.first().copied().unwrap_or(0.0),
            load5: loads.get(1).copied().unwrap_or(0.0),
            load15: loads.get(2).copied().unwrap_or(0.0),
            mem_used,
            mem_total,
            swap_used,
            swap_total,
            disk_used,
            disk_total,
            net_in: per_second(io_now.rx.saturating_sub(io_before.rx), elapsed),
            net_out: per_second(io_now.tx.saturating_sub(io_before.tx), elapsed),
            net_rx_total: io_now.rx.min(i64::MAX as u64) as i64,
            net_tx_total: io_now.tx.min(i64::MAX as u64) as i64,
            uptime,
            processes,
            tcp_connections,
            udp_connections,
            cpu_cores: self.basic.cpu_cores,
            cpu_model: self.basic.cpu_model.clone(),
            os: self.basic.os.clone(),
            kernel: self.basic.kernel.clone(),
            arch: self.basic.arch.clone(),
            virtualization: self.basic.virtualization.clone(),
            gpu_usage: self.basic.gpu_usage,
            gpu_model: self.basic.gpu_model.clone(),
            agent_version: VERSION.to_string(),
            disk_read_bps: per_second(
                io_now
                    .read_sectors
                    .saturating_sub(io_before.read_sectors)
                    .saturating_mul(512),
                elapsed,
            ),
            disk_write_bps: per_second(
                io_now
                    .write_sectors
                    .saturating_sub(io_before.write_sectors)
                    .saturating_mul(512),
                elapsed,
            ),
            disk_read_iops: per_second(read_ops, elapsed),
            disk_write_iops: per_second(write_ops, elapsed),
            disk_await_ms: if total_ops == 0 {
                0.0
            } else {
                io_wait as f64 / total_ops as f64
            },
            disk_utilization: if elapsed_ms <= 0.0 {
                0.0
            } else {
                (io_now.io_millis.saturating_sub(io_before.io_millis) as f64 * 100.0 / elapsed_ms)
                    .clamp(0.0, 100.0)
            },
            disks,
            gpus: self.basic.gpus.clone(),
            latency_results,
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
struct Collector {
    system: System,
    networks: Networks,
    previous_at: Instant,
    basic: BasicMetrics,
    basic_at: Instant,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl Collector {
    fn new(_config: &RuntimeConfig) -> Self {
        let mut collector = Self {
            system: System::new_all(),
            networks: Networks::new_with_refreshed_list(),
            previous_at: Instant::now(),
            basic: BasicMetrics::default(),
            basic_at: Instant::now(),
        };
        collector.refresh_basic();
        collector
    }

    fn refresh_basic(&mut self) {
        let gpus = gpu_info();
        self.basic = BasicMetrics {
            cpu_cores: self.system.cpus().len().max(1) as i64,
            cpu_model: self
                .system
                .cpus()
                .first()
                .map(|cpu| cpu.brand().to_string())
                .unwrap_or_default(),
            os: System::long_os_version().unwrap_or_else(|| env::consts::OS.to_string()),
            kernel: System::kernel_version().unwrap_or_default(),
            arch: env::consts::ARCH.to_string(),
            virtualization: String::new(),
            gpu_usage: average_gpu_usage(&gpus),
            gpu_model: gpus
                .iter()
                .map(|gpu| gpu.model.as_str())
                .collect::<Vec<_>>()
                .join(" · "),
            gpus,
        };
        self.basic_at = Instant::now();
    }

    fn collect(
        &mut self,
        config: &RuntimeConfig,
        latency_results: Vec<LatencyResult>,
        timestamp: i64,
    ) -> Report {
        let sampled_at = Instant::now();
        let elapsed = sampled_at
            .saturating_duration_since(self.previous_at)
            .as_secs_f64()
            .max(0.001);
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.system.refresh_processes(ProcessesToUpdate::All, true);
        self.networks.refresh(true);
        self.previous_at = sampled_at;
        if self.basic_at.elapsed() >= BASIC_INFO_REFRESH_INTERVAL {
            self.refresh_basic();
        }

        let disks = Disks::new_with_refreshed_list();
        #[cfg(target_os = "windows")]
        let root = format!(
            "{}\\",
            env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string())
        );
        #[cfg(target_os = "macos")]
        let root = "/".to_string();
        let mut disk_metrics = disks
            .iter()
            .filter(|disk| !disk.is_removable())
            .map(|disk| {
                let total = disk.total_space();
                DiskMetric {
                    name: disk.name().to_string_lossy().to_string(),
                    mount_point: disk.mount_point().to_string_lossy().to_string(),
                    used: u64_to_i64(total.saturating_sub(disk.available_space())),
                    total: u64_to_i64(total),
                    ..DiskMetric::default()
                }
            })
            .collect::<Vec<_>>();
        disk_metrics.sort_by_key(|disk| (disk.mount_point != root, disk.mount_point.clone()));
        let (tcp_connections, udp_connections) = connection_counts();

        let mut net_in = 0_u64;
        let mut net_out = 0_u64;
        let mut net_rx_total = 0_u64;
        let mut net_tx_total = 0_u64;
        for (name, network) in &self.networks {
            if !selected_interface(name, &config.network_interface) {
                continue;
            }
            net_in = net_in.saturating_add(network.received());
            net_out = net_out.saturating_add(network.transmitted());
            net_rx_total = net_rx_total.saturating_add(network.total_received());
            net_tx_total = net_tx_total.saturating_add(network.total_transmitted());
        }
        let load = System::load_average();
        Report {
            timestamp,
            cpu: self.system.global_cpu_usage() as f64,
            load1: load.one,
            load5: load.five,
            load15: load.fifteen,
            mem_used: u64_to_i64(self.system.used_memory()),
            mem_total: u64_to_i64(self.system.total_memory()),
            swap_used: u64_to_i64(self.system.used_swap()),
            swap_total: u64_to_i64(self.system.total_swap()),
            disk_used: disk_metrics.iter().map(|disk| disk.used).sum(),
            disk_total: disk_metrics.iter().map(|disk| disk.total).sum(),
            net_in: per_second(net_in, elapsed),
            net_out: per_second(net_out, elapsed),
            net_rx_total: u64_to_i64(net_rx_total),
            net_tx_total: u64_to_i64(net_tx_total),
            uptime: u64_to_i64(System::uptime()),
            processes: self.system.processes().len() as i64,
            tcp_connections,
            udp_connections,
            cpu_cores: self.basic.cpu_cores,
            cpu_model: self.basic.cpu_model.clone(),
            os: self.basic.os.clone(),
            kernel: self.basic.kernel.clone(),
            arch: self.basic.arch.clone(),
            virtualization: self.basic.virtualization.clone(),
            gpu_usage: self.basic.gpu_usage,
            gpu_model: self.basic.gpu_model.clone(),
            agent_version: VERSION.to_string(),
            disk_read_bps: 0.0,
            disk_write_bps: 0.0,
            disk_read_iops: 0.0,
            disk_write_iops: 0.0,
            disk_await_ms: 0.0,
            disk_utilization: 0.0,
            disks: disk_metrics,
            gpus: self.basic.gpus.clone(),
            latency_results,
        }
    }
}

fn runtime_config(options: &CliOptions) -> Result<RuntimeConfig> {
    let interval = options.interval;
    if !(15..=3600).contains(&interval) {
        return Err("interval must be between 15 and 3600 seconds".into());
    }
    let token = options.token.clone();
    let endpoint = options.endpoint.clone();
    if token.is_empty() || token.len() > 512 || token.chars().any(char::is_whitespace) {
        return Err("token is invalid".into());
    }
    if !valid_endpoint(&endpoint) {
        return Err(
            "endpoint must use HTTPS; HTTP is only allowed for loopback development".into(),
        );
    }
    Ok(RuntimeConfig {
        token,
        endpoint: endpoint.trim_end_matches('/').to_string(),
        report_interval: interval,
        collect_interval: 1,
        network_interface: String::new(),
        agent_mirror: String::new(),
        auto_update: true,
        latency_tasks: Vec::new(),
    })
}

fn submit(
    agent: &ureq::Agent,
    config: &RuntimeConfig,
    reports: &[Report],
    config_hash: &str,
) -> Result<SubmitResult> {
    let batch = ReportBatch { samples: reports };
    let started_ms = unix_timestamp_millis();
    let mut response = agent
        .post(&format!("{}/api/agent/report", config.endpoint))
        .header("Authorization", format!("Bearer {}", config.token))
        .header("User-Agent", format!("nodeflare-agent/{VERSION}"))
        .header("X-Agent-Config-Sha256", config_hash)
        .send_json(batch)?;
    let ended_ms = unix_timestamp_millis();
    let clock_offset_ms = response
        .headers()
        .get("x-nodeflare-server-time")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| clock_offset_from_server_seconds(value, started_ms, ended_ms))
        .or_else(|| {
            response
                .headers()
                .get("date")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| clock_offset_from_http_date(value, started_ms, ended_ms))
        });
    let next_hash = response
        .headers()
        .get("X-Agent-Config-Sha256")
        .and_then(|value| value.to_str().ok())
        .unwrap_or(config_hash)
        .to_string();
    let remote = if response.status().as_u16() == 204 {
        None
    } else {
        Some(response.body_mut().read_json::<RemoteConfig>()?)
    };
    Ok(SubmitResult {
        config: remote,
        config_hash: next_hash,
        clock_offset_ms,
    })
}

fn fetch_remote_config(
    agent: &ureq::Agent,
    config: &RuntimeConfig,
    config_hash: &str,
) -> Result<SubmitResult> {
    let started_ms = unix_timestamp_millis();
    let mut response = agent
        .get(&format!("{}/api/agent/config", config.endpoint))
        .header("Authorization", format!("Bearer {}", config.token))
        .header("User-Agent", format!("nodeflare-agent/{VERSION}"))
        .header("X-Agent-Config-Sha256", config_hash)
        .call()?;
    let ended_ms = unix_timestamp_millis();
    let clock_offset_ms = response
        .headers()
        .get("x-nodeflare-server-time")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| clock_offset_from_server_seconds(value, started_ms, ended_ms))
        .or_else(|| {
            response
                .headers()
                .get("date")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| clock_offset_from_http_date(value, started_ms, ended_ms))
        });
    let next_hash = response
        .headers()
        .get("X-Agent-Config-Sha256")
        .and_then(|value| value.to_str().ok())
        .unwrap_or(config_hash)
        .to_string();
    let remote = if response.status().as_u16() == 204 {
        None
    } else {
        Some(response.body_mut().read_json::<RemoteConfig>()?)
    };
    Ok(SubmitResult {
        config: remote,
        config_hash: next_hash,
        clock_offset_ms,
    })
}

fn fetch_clock_offset(agent: &ureq::Agent, endpoint: &str) -> Result<Option<i64>> {
    let started_ms = unix_timestamp_millis();
    let response = agent
        .get(&format!("{endpoint}/api/config"))
        .header("User-Agent", format!("nodeflare-agent/{VERSION}"))
        .call()?;
    let ended_ms = unix_timestamp_millis();
    Ok(response
        .headers()
        .get("x-nodeflare-server-time")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| clock_offset_from_server_seconds(value, started_ms, ended_ms))
        .or_else(|| {
            response
                .headers()
                .get("date")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| clock_offset_from_http_date(value, started_ms, ended_ms))
        }))
}

fn live_endpoint(endpoint: &str) -> Result<String> {
    let mut url = url::Url::parse(endpoint)?;
    url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
        .map_err(|_| "unsupported live endpoint scheme")?;
    let path = format!("{}/api/agent/live", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url.to_string())
}

type LiveSocket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

fn connect_live(endpoint: &str, token: &str) -> Result<(LiveSocket, Option<i64>)> {
    let mut request = endpoint.into_client_request()?;
    request
        .headers_mut()
        .insert("Authorization", format!("Bearer {token}").parse()?);
    request
        .headers_mut()
        .insert("User-Agent", format!("nodeflare-agent/{VERSION}").parse()?);
    let started_ms = unix_timestamp_millis();
    let (socket, response) = connect(request)?;
    let ended_ms = unix_timestamp_millis();
    let clock_offset_ms = response
        .headers()
        .get("date")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| clock_offset_from_http_date(value, started_ms, ended_ms));
    Ok((socket, clock_offset_ms))
}

fn set_live_read_timeout(socket: &mut LiveSocket, timeout: Option<Duration>) -> io::Result<()> {
    match socket.get_mut() {
        tungstenite::stream::MaybeTlsStream::Plain(stream) => stream.set_read_timeout(timeout),
        tungstenite::stream::MaybeTlsStream::Rustls(stream) => {
            stream.sock.set_read_timeout(timeout)
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveAck {
    #[serde(rename = "type")]
    message_type: String,
    ts: i64,
    persisted: bool,
    #[serde(rename = "nextD1WriteAfterMs")]
    next_d1_write_after_ms: u64,
    #[serde(rename = "nextWssReportAfterMs")]
    next_wss_report_after_ms: u64,
    #[serde(rename = "realtimeHint")]
    realtime_hint: bool,
}

fn ack_interval(ack: &LiveAck) -> Duration {
    Duration::from_millis(ack.next_wss_report_after_ms)
        .clamp(Duration::from_secs(1), Duration::from_secs(60))
}

fn read_live_ack(socket: &mut LiveSocket) -> Result<Option<Duration>> {
    match socket.read() {
        Ok(Message::Text(text)) => {
            let Ok(ack) = serde_json::from_str::<LiveAck>(text.as_ref()) else {
                return Ok(None);
            };
            if ack.message_type != "ack" || ack.ts <= 0 {
                return Ok(None);
            }
            let _ = (ack.persisted, ack.next_d1_write_after_ms, ack.realtime_hint);
            Ok(Some(ack_interval(&ack)))
        }
        Ok(Message::Ping(payload)) => {
            socket.send(Message::Pong(payload))?;
            Ok(None)
        }
        Ok(Message::Close(_)) => Ok(Some(Duration::ZERO)),
        Ok(_) => Ok(None),
        Err(WebSocketError::Io(error))
            if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

fn live_update_payload(reports: &[Report]) -> Result<String> {
    Ok(serde_json::to_string(&serde_json::json!({
        "type": "update",
        "samples": reports,
    }))?)
}

fn requeue_reports(pending: &Arc<(Mutex<VecDeque<Report>>, Condvar)>, reports: Vec<Report>) {
    if reports.is_empty() {
        return;
    }
    let (queue, ready) = &**pending;
    if let Ok(mut queue) = queue.lock() {
        for report in reports.into_iter().rev() {
            queue.push_front(report);
        }
        while queue.len() > LIVE_QUEUE_CAPACITY {
            queue.pop_back();
        }
        ready.notify_one();
    }
}

fn live_sender_loop(
    endpoint: &str,
    token: &str,
    pending: Arc<(Mutex<VecDeque<Report>>, Condvar)>,
    configured_interval: Arc<Mutex<Duration>>,
    healthy: Arc<AtomicBool>,
    clock: SharedClock,
) {
    let mut socket: Option<LiveSocket> = None;
    let mut send_interval = configured_interval
        .lock()
        .map(|interval| *interval)
        .unwrap_or(Duration::from_secs(1));
    let mut next_send_at = Instant::now() + send_interval;
    loop {
        let (queue_lock, ready) = &*pending;
        let mut queue = match queue_lock.lock() {
            Ok(queue) => queue,
            Err(_) => return,
        };
        while queue.is_empty() || Instant::now() < next_send_at {
            let wait = if queue.is_empty() {
                Duration::from_millis(250)
            } else {
                next_send_at
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(250))
            };
            queue = match ready.wait_timeout(queue, wait) {
                Ok((queue, _)) => queue,
                Err(_) => return,
            };
            drop(queue);

            let mut drop_socket = false;
            if let Some(connected) = socket.as_mut() {
                if set_live_read_timeout(connected, Some(LIVE_HINT_READ_TIMEOUT)).is_err() {
                    drop_socket = true;
                } else {
                    match read_live_ack(connected) {
                        Ok(Some(Duration::ZERO)) => drop_socket = true,
                        Ok(Some(interval)) => {
                            healthy.store(true, Ordering::Release);
                            send_interval = interval;
                            next_send_at = Instant::now() + send_interval;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            eprintln!("live hint read failed: {error}");
                            drop_socket = true;
                        }
                    }
                }
            }
            if drop_socket {
                socket = None;
                healthy.store(false, Ordering::Release);
                next_send_at = Instant::now();
            }
            queue = match queue_lock.lock() {
                Ok(queue) => queue,
                Err(_) => return,
            };
        }
        let count = queue.len().min(LIVE_BATCH_CAPACITY);
        let batch = queue.drain(..count).collect::<Vec<_>>();
        drop(queue);

        if let Ok(interval) = configured_interval.lock() {
            send_interval = (*interval).clamp(Duration::from_secs(1), Duration::from_secs(60));
        }
        if socket.is_none() {
            match connect_live(endpoint, token) {
                Ok((mut connected, offset_ms)) => {
                    observe_clock(&clock, offset_ms);
                    if set_live_read_timeout(&mut connected, Some(LIVE_ACK_READ_TIMEOUT)).is_err() {
                        socket = None;
                        healthy.store(false, Ordering::Release);
                        thread::sleep(LIVE_RECONNECT_DELAY);
                        requeue_reports(&pending, batch);
                        continue;
                    }
                    socket = Some(connected);
                }
                Err(error) => {
                    healthy.store(false, Ordering::Release);
                    eprintln!("live connection failed: {error}");
                    requeue_reports(&pending, batch);
                    thread::sleep(LIVE_RECONNECT_DELAY);
                    continue;
                }
            }
        }

        let payload = match live_update_payload(&batch) {
            Ok(payload) => payload,
            Err(error) => {
                eprintln!("live payload encode failed: {error}");
                requeue_reports(&pending, batch);
                continue;
            }
        };
        let sent = socket
            .as_mut()
            .is_some_and(|socket| socket.send(Message::Text(payload.clone().into())).is_ok());
        if !sent {
            socket = None;
            healthy.store(false, Ordering::Release);
            thread::sleep(LIVE_RECONNECT_DELAY);
            let mut resent = false;
            if let Ok((mut connected, offset_ms)) = connect_live(endpoint, token) {
                observe_clock(&clock, offset_ms);
                if set_live_read_timeout(&mut connected, Some(LIVE_ACK_READ_TIMEOUT)).is_ok()
                    && connected.send(Message::Text(payload.into())).is_ok()
                {
                    socket = Some(connected);
                    resent = true;
                }
            }
            if !resent {
                requeue_reports(&pending, batch);
            }
            continue;
        }

        next_send_at = Instant::now() + send_interval;
        let mut drop_socket = false;
        if let Some(socket) = socket.as_mut() {
            if set_live_read_timeout(socket, Some(LIVE_ACK_READ_TIMEOUT)).is_err() {
                drop_socket = true;
            } else {
                match read_live_ack(socket) {
                    Ok(Some(Duration::ZERO)) => drop_socket = true,
                    Ok(Some(interval)) => {
                        healthy.store(true, Ordering::Release);
                        send_interval = interval;
                        next_send_at = Instant::now() + send_interval;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        eprintln!("live ACK read failed: {error}");
                        drop_socket = true;
                    }
                }
            }
        }
        if drop_socket {
            socket = None;
            healthy.store(false, Ordering::Release);
        }
    }
}

fn apply_remote(config: &mut RuntimeConfig, remote: &RemoteConfig) -> bool {
    if valid_sample_schedule(remote.report_interval, remote.collect_interval) {
        config.report_interval = remote.report_interval;
        config.collect_interval = remote.collect_interval;
    }
    config
        .network_interface
        .clone_from(&remote.network_interface);
    let mirror = remote.agent_mirror.trim().trim_end_matches('/');
    if mirror.is_empty() || valid_endpoint(mirror) {
        config.agent_mirror = mirror.to_string();
    }
    config.auto_update = remote.auto_update;
    let tasks = sanitize_latency_tasks(&remote.latency_tasks);
    let changed = config.latency_tasks != tasks;
    config.latency_tasks = tasks;
    changed
}

fn sanitize_latency_tasks(tasks: &[LatencyTask]) -> Vec<LatencyTask> {
    let mut seen = HashSet::new();
    tasks
        .iter()
        .filter(|task| {
            !task.id.is_empty()
                && task.id.len() <= 80
                && (1..=80).contains(&task.name.trim().chars().count())
                && (30..=3600).contains(&task.interval_seconds)
                && matches!(task.task_type.as_str(), "tcp" | "icmp")
                && ((task.task_type == "tcp"
                    && task.port.is_some_and(|port| (1..=65535).contains(&port)))
                    || (task.task_type == "icmp" && task.port.is_none()))
                && parse_probe_target(&task.target, None).is_some()
                && seen.insert(task.id.clone())
        })
        .take(MAX_LATENCY_TASKS)
        .cloned()
        .collect()
}

fn valid_endpoint(value: &str) -> bool {
    let value = value.trim_end_matches('/');
    if value.is_empty() || value.len() > 2048 || value.chars().any(char::is_whitespace) {
        return false;
    }
    let Ok(parsed) = url::Url::parse(value) else {
        return false;
    };
    if parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return false;
    }
    if parsed.scheme() == "https" {
        return true;
    }
    if parsed.scheme() != "http" {
        return false;
    }
    match parsed.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address == std::net::Ipv4Addr::LOCALHOST,
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn report_retry_delay(failures: u32, report_interval: u64) -> Duration {
    if failures == 0 {
        return Duration::from_secs(report_interval);
    }
    let exponent = failures.saturating_sub(1).min(16);
    REPORT_RETRY_MIN
        .checked_mul(1_u32 << exponent)
        .unwrap_or(REPORT_RETRY_MAX)
        .min(REPORT_RETRY_MAX)
}

fn prune_report_samples(samples: &mut Vec<Report>, now: i64) {
    samples.retain(|report| (report.timestamp - now).abs() <= MAX_REPORT_AGE_SECONDS);
    let mut remaining = MAX_PENDING_LATENCY_RESULTS;
    for report in samples.iter_mut().rev() {
        if report.latency_results.len() > remaining {
            let discard = report.latency_results.len() - remaining;
            report.latency_results.drain(..discard);
            remaining = 0;
        } else {
            remaining -= report.latency_results.len();
        }
    }
}

fn normalized_version(value: &str) -> &str {
    value.trim().strip_prefix('v').unwrap_or(value.trim())
}

fn version_triplet(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = normalized_version(value).split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(version)
}

fn agent_artifact_name() -> Option<&'static str> {
    match (env::consts::OS, env::consts::ARCH) {
        ("linux", "x86_64") => Some("agent-linux-x86_64"),
        ("linux", "aarch64") => Some("agent-linux-aarch64"),
        ("windows", "x86_64") => Some("agent-windows-x86_64.exe"),
        ("macos", "aarch64") => Some("agent-macos-aarch64"),
        _ => None,
    }
}

fn executable_format_valid(path: &Path) -> bool {
    let mut magic = [0_u8; 4];
    if fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_err()
    {
        return false;
    }
    match env::consts::OS {
        "linux" => magic == *b"\x7fELF",
        "windows" => magic.starts_with(b"MZ"),
        "macos" => matches!(
            magic,
            [0xcf, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
        ),
        _ => false,
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn release_asset_sha256(asset: &GithubReleaseAsset) -> Option<String> {
    let digest = asset.digest.as_deref()?.strip_prefix("sha256:")?;
    (digest.len() == 64
        && digest
            .chars()
            .all(|character| character.is_ascii_hexdigit()))
    .then(|| digest.to_ascii_lowercase())
}

fn download_agent(
    agent: &ureq::Agent,
    url: &str,
    destination: &Path,
    expected_version: &str,
    expected_sha256: &str,
) -> Result<bool> {
    let Ok(response) = agent.get(url).call() else {
        return Ok(false);
    };
    let response = response.into_body().into_reader();
    let mut file = fs::File::create(destination)?;
    let copied = io::copy(&mut response.take(MAX_AGENT_BINARY_BYTES + 1), &mut file)?;
    file.flush()?;
    drop(file);
    if copied > MAX_AGENT_BINARY_BYTES
        || !executable_format_valid(destination)
        || sha256_file(destination)? != expected_sha256
    {
        let _ = fs::remove_file(destination);
        return Ok(false);
    }
    #[cfg(unix)]
    fs::set_permissions(destination, fs::Permissions::from_mode(0o755))?;

    let downloaded_version = Command::new(destination)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        });
    let valid = downloaded_version.as_deref().map(normalized_version)
        == Some(normalized_version(expected_version));
    if !valid {
        let _ = fs::remove_file(destination);
    }
    Ok(valid)
}

fn mirrored_download_url(mirror: &str, url: &str) -> String {
    let mirror = mirror.trim().trim_end_matches('/');
    if mirror.is_empty() {
        url.to_string()
    } else {
        format!("{mirror}/{url}")
    }
}

fn update(agent: &ureq::Agent, mirror: &str) -> Result<bool> {
    let mut response = agent
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", format!("nodeflare-agent/{VERSION}"))
        .call()?;
    let release = response.body_mut().read_json::<GithubRelease>()?;
    let Some(remote_version) = version_triplet(&release.tag_name) else {
        return Ok(false);
    };
    let Some(current_version) = version_triplet(VERSION) else {
        return Ok(false);
    };
    if remote_version <= current_version {
        return Ok(false);
    }
    let Some(artifact) = agent_artifact_name() else {
        return Ok(false);
    };
    let current = env::current_exe()?;
    let temporary = current.with_file_name(format!(
        ".nodeflare-agent.{}.download{}",
        std::process::id(),
        if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        }
    ));
    let Some(release_asset) = release.assets.iter().find(|asset| asset.name == artifact) else {
        return Err(format!("latest release does not contain {artifact}").into());
    };
    let Some(expected_sha256) = release_asset_sha256(release_asset) else {
        return Err(
            format!("latest release does not contain a SHA-256 digest for {artifact}").into(),
        );
    };
    if !download_agent(
        agent,
        &mirrored_download_url(mirror, &release_asset.browser_download_url),
        &temporary,
        &release.tag_name,
        &expected_sha256,
    )? {
        return Err("downloaded agent version does not match the configured version".into());
    }

    #[cfg(unix)]
    {
        fs::rename(&temporary, &current)?;
        let error = Command::new(&current).args(env::args_os().skip(1)).exec();
        Err(error.into())
    }

    #[cfg(target_os = "windows")]
    {
        let script = concat!(
            "$ErrorActionPreference='Stop'; ",
            "$targetPid=[int]$env:NODEFLARE_UPDATE_PID; ",
            "Wait-Process -Id $targetPid; ",
            "Move-Item -LiteralPath $env:NODEFLARE_UPDATE_NEW ",
            "-Destination $env:NODEFLARE_UPDATE_CURRENT -Force; ",
            "try { Start-ScheduledTask -TaskName 'NodeFlare Agent' -ErrorAction Stop } ",
            "catch { $restartArgs=@($env:NODEFLARE_UPDATE_ARGS | ConvertFrom-Json); ",
            "Start-Process -FilePath $env:NODEFLARE_UPDATE_CURRENT -ArgumentList $restartArgs }"
        );
        Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .env("NODEFLARE_UPDATE_PID", std::process::id().to_string())
            .env("NODEFLARE_UPDATE_NEW", &temporary)
            .env("NODEFLARE_UPDATE_CURRENT", &current)
            .env(
                "NODEFLARE_UPDATE_ARGS",
                serde_json::to_string(&env::args().skip(1).collect::<Vec<_>>())?,
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(true)
    }
}

fn run(options: &CliOptions, once: bool, print_only: bool) -> Result<()> {
    let mut config = runtime_config(options)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let mut collector = Collector::new(&config);
    let mut next_report = Instant::now();
    let mut next_collect = Instant::now() + Duration::from_secs(config.collect_interval);
    let mut next_latency: HashMap<String, Instant> = HashMap::new();
    let mut latency_executor = LatencyExecutor::new()?;
    let mut pending_results = Vec::new();
    let mut pending_samples = Vec::new();
    let mut report_failures = 0_u32;
    let mut next_update_check = Instant::now();
    let mut config_hash = String::new();
    let mut live_was_healthy = false;
    let clock = Arc::new(Mutex::new(ClockCalibration::default()));
    if !print_only {
        match fetch_clock_offset(&agent, &config.endpoint) {
            Ok(offset_ms) => observe_clock(&clock, offset_ms),
            Err(error) => eprintln!("server time calibration failed: {error}"),
        }
    }
    let live = (!once && !print_only)
        .then(|| LiveSender::start(&config, Arc::clone(&clock)))
        .transpose()?;
    loop {
        let live_healthy = live.as_ref().is_some_and(LiveSender::is_healthy);
        if live_was_healthy && !live_healthy {
            next_report = Instant::now();
        }
        live_was_healthy = live_healthy;
        pending_results.extend(latency_executor.drain());
        let current = Instant::now();
        let due = config
            .latency_tasks
            .iter()
            .filter(|task| {
                next_latency
                    .get(&task.id)
                    .is_none_or(|deadline| *deadline <= current)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !due.is_empty() {
            let scheduled_at = Instant::now();
            for task in due {
                let task_id = task.id.clone();
                let interval = task.interval_seconds.clamp(30, 3600);
                if latency_executor.enqueue(task) {
                    next_latency.insert(task_id, scheduled_at + Duration::from_secs(interval));
                }
            }
        }

        if Instant::now() >= next_collect {
            let offset_ms = shared_clock_offset(&clock);
            let mut latest_results = HashMap::new();
            for mut result in std::mem::take(&mut pending_results) {
                result.timestamp = corrected_timestamp(result.timestamp, offset_ms);
                latest_results.insert(result.task_id.clone(), result);
            }
            let mut report = collector.collect(&config, latest_results.into_values().collect(), 0);
            report.timestamp = corrected_timestamp(unix_timestamp(), offset_ms);
            if print_only {
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            if let Some(live) = &live {
                live.send(&report);
            }
            pending_samples.push(report);
            prune_report_samples(
                &mut pending_samples,
                corrected_timestamp(unix_timestamp(), offset_ms),
            );
            if pending_samples.len() > LIVE_QUEUE_CAPACITY {
                let overflow = pending_samples.len() - LIVE_QUEUE_CAPACITY;
                pending_samples.drain(..overflow);
            }
            next_collect = advance_deadline(
                next_collect,
                Duration::from_secs(config.collect_interval),
                Instant::now(),
            );
        }

        if Instant::now() >= next_report && !pending_samples.is_empty() {
            let sync_result = if live_healthy {
                fetch_remote_config(&agent, &config, &config_hash).map(|result| (result, false))
            } else {
                submit(&agent, &config, &pending_samples, &config_hash).map(|result| (result, true))
            };
            let sync_failed = sync_result.is_err();
            match sync_result {
                Ok((result, submitted_metrics)) => {
                    observe_clock(&clock, result.clock_offset_ms);
                    if submitted_metrics {
                        pending_samples.clear();
                    }
                    report_failures = 0;
                    config_hash = result.config_hash;
                    if let Some(remote) = result.config {
                        let previous_collect_interval = config.collect_interval;
                        let previous_report_interval = config.report_interval;
                        if apply_remote(&mut config, &remote) {
                            next_latency.clear();
                        }
                        if config.collect_interval != previous_collect_interval {
                            next_collect =
                                Instant::now() + Duration::from_secs(config.collect_interval);
                        }
                        if config.collect_interval != previous_collect_interval
                            || config.report_interval != previous_report_interval
                        {
                            if let Some(live) = &live {
                                live.set_send_interval(
                                    config.report_interval,
                                    config.collect_interval,
                                );
                            }
                        }
                    }
                    if !once && config.auto_update && Instant::now() >= next_update_check {
                        match update(&agent, &config.agent_mirror) {
                            Ok(true) => return Ok(()),
                            Ok(false) => {
                                next_update_check = Instant::now() + UPDATE_CHECK_INTERVAL;
                            }
                            Err(error) => {
                                eprintln!("agent update failed: {error}");
                                next_update_check = Instant::now() + UPDATE_RETRY_INTERVAL;
                            }
                        }
                    }
                }
                Err(error) => {
                    if live_healthy {
                        eprintln!("config sync failed: {error}");
                    } else {
                        eprintln!("report failed: {error}");
                        report_failures = report_failures.saturating_add(1);
                    }
                    if once {
                        return Err(error);
                    }
                }
            }
            let retry_delay = if live_healthy && sync_failed {
                REPORT_RETRY_MIN
            } else {
                report_retry_delay(report_failures, config.report_interval)
            };
            next_report = Instant::now() + retry_delay;
            if once {
                return Ok(());
            }
        }

        let wake_at = next_latency
            .values()
            .copied()
            .min()
            .map_or(next_report.min(next_collect), |deadline| {
                deadline.min(next_report).min(next_collect)
            });
        let wait = wake_at
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(1));
        if !wait.is_zero() {
            thread::sleep(wait);
        }
    }
}

fn main() {
    let options = CliOptions::parse();
    let result = run(&options, options.once || options.collect, options.collect);
    if let Err(error) = result {
        eprintln!("nodeflare-agent: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        ack_interval, advance_deadline, agent_artifact_name, clock_offset_from_http_date,
        corrected_timestamp, executable_format_valid, gpu_name_from_uevent, is_public_probe_ip,
        live_batch_interval, live_endpoint, live_update_payload, median, mirrored_download_url,
        normalized_version, parse_lspci_gpu_names, parse_probe_target,
        parse_system_profiler_gpu_names, ping_latency, prune_report_samples, release_asset_sha256,
        report_retry_delay, sanitize_latency_tasks, selected_interface, tcp_latency_probe_address,
        valid_endpoint, valid_sample_schedule, version_triplet, CliOptions, ClockCalibration,
        GithubReleaseAsset, LatencyResult, LatencyTask, LiveAck, Report, CLOCK_CALIBRATION_MAX_AGE,
        MAX_LATENCY_TASKS, MAX_PENDING_LATENCY_RESULTS, PROBE_ATTEMPTS,
    };

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    use super::connection_counts_from_netstat;
    #[cfg(target_os = "linux")]
    use super::disk_device;

    #[test]
    fn selects_network_interfaces() {
        assert!(selected_interface("eth0", ""));
        assert!(!selected_interface("lo", ""));
        assert!(selected_interface("ens3", "eth0, ens3"));
    }

    #[test]
    fn parses_platform_gpu_names() {
        let lspci = "00:02.0 VGA compatible controller: Intel Corporation CometLake-S GT2 [UHD Graphics 630] (rev 05)\n\
                     01:00.0 Audio device: NVIDIA Corporation HDMI Audio\n\
                     02:00.0 3D controller: NVIDIA Corporation GA102 [GeForce RTX 3090] (rev a1)";
        assert_eq!(
            parse_lspci_gpu_names(lspci),
            [
                "Intel Corporation CometLake-S GT2 [UHD Graphics 630] (rev 05)",
                "NVIDIA Corporation GA102 [GeForce RTX 3090] (rev a1)",
            ]
        );

        let profiler = "Graphics/Displays:\n\n    Apple M3 Max:\n\n      Chipset Model: Apple M3 Max\n      Metal Support: Metal 3";
        assert_eq!(parse_system_profiler_gpu_names(profiler), ["Apple M3 Max"]);

        assert_eq!(
            gpu_name_from_uevent("DRIVER=i915\nPCI_CLASS=30000\nPCI_ID=8086:591B"),
            Some("Intel Integrated Graphics".to_string())
        );
        assert_eq!(
            gpu_name_from_uevent("DRIVER=virtio_gpu\nPCI_CLASS=30000"),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn selects_whole_disk_devices() {
        assert!(disk_device("sda"));
        assert!(disk_device("nvme0n1"));
        assert!(!disk_device("sda1"));
        assert!(!disk_device("nvme0n1p1"));
    }

    #[test]
    fn parses_release_versions() {
        assert_eq!(normalized_version("v1.2.3"), "1.2.3");
        assert_eq!(version_triplet("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(version_triplet("1.2"), None);
        assert_eq!(version_triplet("1.2.3.4"), None);
        assert_eq!(version_triplet("rust-3"), None);
    }

    #[test]
    fn validates_release_asset_digests() {
        let hash = "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF";
        let mut asset = GithubReleaseAsset {
            name: "agent-linux-x86_64".to_string(),
            browser_download_url: "https://example.com/agent".to_string(),
            digest: Some(format!("sha256:{hash}")),
        };
        let expected = hash.to_ascii_lowercase();
        assert_eq!(
            release_asset_sha256(&asset).as_deref(),
            Some(expected.as_str())
        );
        asset.digest = Some("sha512:0123".to_string());
        assert_eq!(release_asset_sha256(&asset), None);
        asset.digest = None;
        assert_eq!(release_asset_sha256(&asset), None);
    }

    #[test]
    fn validates_endpoints() {
        assert!(valid_endpoint("https://monitor.example.com/"));
        assert!(valid_endpoint("http://127.0.0.1:8787"));
        assert!(valid_endpoint("http://localhost:8787"));
        assert!(valid_endpoint("http://[::1]:8787"));
        assert!(!valid_endpoint("http://monitor.example.com"));
        assert!(!valid_endpoint("http://127.0.0.2:8787"));
        assert!(!valid_endpoint("monitor.example.com"));
        assert!(!valid_endpoint("https://"));
        assert!(!valid_endpoint("https://user@example.com"));
        assert!(!valid_endpoint("https://monitor.example.com/?token=abc"));
        assert!(!valid_endpoint("https://bad host.example"));
    }

    #[test]
    fn builds_mirrored_download_urls() {
        let release =
            "https://github.com/imengying/NodeFlare/releases/download/v1.2.3/agent-linux-x86_64";
        assert_eq!(mirrored_download_url("", release), release);
        assert_eq!(
            mirrored_download_url("https://mirror.example.com/", release),
            format!("https://mirror.example.com/{release}")
        );
    }

    #[test]
    fn builds_live_websocket_endpoints() {
        assert_eq!(
            live_endpoint("https://monitor.example.com").unwrap(),
            "wss://monitor.example.com/api/agent/live"
        );
        assert_eq!(
            live_endpoint("https://monitor.example.com/base/").unwrap(),
            "wss://monitor.example.com/base/api/agent/live"
        );
        assert_eq!(
            live_endpoint("http://127.0.0.1:8787").unwrap(),
            "ws://127.0.0.1:8787/api/agent/live"
        );
    }

    #[test]
    fn accepts_server_realtime_ack_interval() {
        let ack: LiveAck = serde_json::from_str(
            r#"{"type":"ack","ts":100,"persisted":false,"nextD1WriteAfterMs":60000,"nextWssReportAfterMs":5000,"realtimeHint":false}"#,
        )
        .unwrap();
        assert_eq!(ack_interval(&ack), Duration::from_secs(5));
        let slow: LiveAck = serde_json::from_str(
            r#"{"type":"ack","ts":100,"persisted":true,"nextD1WriteAfterMs":60000,"nextWssReportAfterMs":1,"realtimeHint":true}"#,
        )
        .unwrap();
        assert_eq!(ack_interval(&slow), Duration::from_secs(1));
        assert!(slow.realtime_hint);
    }

    #[test]
    fn encodes_realtime_samples_as_a_batch() {
        let payload = live_update_payload(&[
            Report {
                timestamp: 10,
                cpu: 20.0,
                ..Report::default()
            },
            Report {
                timestamp: 15,
                cpu: 30.0,
                ..Report::default()
            },
        ])
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["type"], "update");
        assert_eq!(value["samples"].as_array().unwrap().len(), 2);
        assert_eq!(value["samples"][1]["cpu"], 30.0);
    }

    #[test]
    fn calibrates_timestamps_from_http_date() {
        let server_ms = 784_111_777_000_i64;
        assert_eq!(
            clock_offset_from_http_date(
                "Sun, 06 Nov 1994 08:49:37 GMT",
                server_ms - 1_000,
                server_ms + 1_000,
            ),
            Some(0),
        );
        assert_eq!(corrected_timestamp(100, 2_500), 102);
        assert_eq!(corrected_timestamp(100, -2_500), 97);
    }

    #[test]
    fn ignores_small_clock_jitter_until_the_calibration_expires() {
        let mut calibration = ClockCalibration::default();
        assert!(calibration.observe(1_000));
        assert!(!calibration.observe(2_000));
        assert_eq!(calibration.offset_ms, 1_000);
        assert!(calibration.observe(25_000));
        calibration.calibrated_at = Some(Instant::now() - CLOCK_CALIBRATION_MAX_AGE);
        assert!(calibration.observe(25_001));
    }

    #[test]
    fn parses_cli_options() {
        let parsed = CliOptions::try_parse_from([
            "nodeflare",
            "-e",
            "https://monitor.example.com",
            "-t",
            "secret",
            "-i",
            "60",
            "--once",
        ])
        .unwrap();
        assert_eq!(parsed.endpoint, "https://monitor.example.com");
        assert_eq!(parsed.token, "secret");
        assert_eq!(parsed.interval, 60);
        assert!(parsed.once);
        assert!(CliOptions::try_parse_from(["nodeflare", "-t"]).is_err());
        assert!(CliOptions::try_parse_from(["nodeflare", "-t", "first", "-t", "second"]).is_err());
        assert!(CliOptions::try_parse_from([
            "nodeflare",
            "-e",
            "https://monitor.example.com",
            "-t",
            "secret",
            "-s",
            "server-a",
        ])
        .is_err());
        assert!(CliOptions::try_parse_from(["nodeflare", "--once", "--collect"]).is_err());
    }

    #[test]
    fn bounds_and_filters_remote_latency_tasks() {
        let valid = LatencyTask {
            id: "task-1".to_string(),
            name: "Cloudflare".to_string(),
            task_type: "tcp".to_string(),
            target: "1.1.1.1".to_string(),
            port: Some(443),
            interval_seconds: 60,
        };
        let mut tasks = vec![valid.clone(), valid];
        tasks.push(LatencyTask {
            id: "bad".to_string(),
            name: "Bad".to_string(),
            task_type: "http".to_string(),
            target: "https://example.com".to_string(),
            port: Some(443),
            interval_seconds: 1,
        });
        for index in 2..=MAX_LATENCY_TASKS + 10 {
            tasks.push(LatencyTask {
                id: format!("task-{index}"),
                name: format!("Task {index}"),
                task_type: "icmp".to_string(),
                target: "1.1.1.1".to_string(),
                port: None,
                interval_seconds: 60,
            });
        }
        let sanitized = sanitize_latency_tasks(&tasks);
        assert_eq!(sanitized.len(), MAX_LATENCY_TASKS);
        assert_eq!(sanitized[0].id, "task-1");
        assert!(!sanitized.iter().any(|task| task.id == "bad"));
    }

    #[test]
    fn backs_off_failed_reports() {
        assert_eq!(report_retry_delay(0, 60), Duration::from_secs(60));
        assert_eq!(report_retry_delay(1, 60), Duration::from_secs(5));
        assert_eq!(report_retry_delay(4, 60), Duration::from_secs(40));
        assert_eq!(report_retry_delay(20, 60), Duration::from_secs(300));
    }

    #[test]
    fn validates_sample_schedules() {
        assert!(valid_sample_schedule(3600, 5));
        assert!(!valid_sample_schedule(3600, 4));
        assert!(!valid_sample_schedule(60, 0));
        assert!(!valid_sample_schedule(10, 1));
    }

    #[test]
    fn batches_live_samples_at_one_fifteenth_of_the_history_interval() {
        assert_eq!(live_batch_interval(60, 1), Duration::from_secs(4));
        assert_eq!(live_batch_interval(60, 5), Duration::from_secs(5));
        assert_eq!(live_batch_interval(120, 1), Duration::from_secs(8));
        assert_eq!(live_batch_interval(3_600, 1), Duration::from_secs(60));
    }

    #[test]
    fn advances_collection_deadlines_without_drift() {
        let start = Instant::now();
        let interval = Duration::from_secs(1);
        assert_eq!(
            advance_deadline(start, interval, start + Duration::from_millis(2500)),
            start + Duration::from_secs(3)
        );
    }

    #[test]
    fn drops_samples_the_worker_would_reject_as_expired() {
        let mut samples = vec![
            Report {
                timestamp: 1_000,
                ..Report::default()
            },
            Report {
                timestamp: 8_500,
                ..Report::default()
            },
        ];
        prune_report_samples(&mut samples, 9_000);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].timestamp, 8_500);
    }

    #[test]
    fn bounds_pending_latency_results() {
        let mut samples = (0..MAX_PENDING_LATENCY_RESULTS + 10)
            .map(|index| Report {
                timestamp: index as i64 + 1,
                latency_results: vec![LatencyResult {
                    task_id: format!("task-{index}"),
                    timestamp: index as i64 + 1,
                    latency_ms: 10.0,
                    packet_loss: 0.0,
                }],
                ..Report::default()
            })
            .collect::<Vec<_>>();
        prune_report_samples(&mut samples, MAX_PENDING_LATENCY_RESULTS as i64 + 10);
        assert_eq!(
            samples
                .iter()
                .map(|sample| sample.latency_results.len())
                .sum::<usize>(),
            MAX_PENDING_LATENCY_RESULTS
        );
        assert!(samples.first().unwrap().latency_results.is_empty());
        assert_eq!(samples.last().unwrap().latency_results.len(), 1);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn parses_netstat_connection_counts() {
        let output = "tcp4 0 0 host.443 peer.1 ESTABLISHED\nudp4 0 0 *.5353 *.*\nTCP host peer ESTABLISHED\n";
        assert_eq!(connection_counts_from_netstat(output), (2, 1));
    }

    #[test]
    fn selects_current_platform_artifact() {
        #[cfg(target_os = "linux")]
        assert!(agent_artifact_name().is_some_and(|name| name.starts_with("agent-linux-")));
        #[cfg(target_os = "windows")]
        assert_eq!(agent_artifact_name(), Some("agent-windows-x86_64.exe"));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(agent_artifact_name(), Some("agent-macos-aarch64"));
    }

    #[test]
    fn recognizes_current_executable_format() {
        assert!(executable_format_valid(
            &std::env::current_exe().expect("current executable")
        ));
    }

    #[test]
    fn parses_probe_targets() {
        assert_eq!(
            parse_probe_target("Example.COM", None),
            Some(("example.com".to_string(), 443))
        );
        assert_eq!(
            parse_probe_target("1.1.1.1", Some(8080)),
            Some(("1.1.1.1".to_string(), 8080))
        );
        for target in [
            "",
            "https://example.com",
            "example.com:0",
            "example.com:65536",
            "999.1.1.1",
            "127.0.0.1:8080",
            "169.254.169.254",
            "192.168.1.1",
            "router.local",
            "localhost",
            "[::1]:443",
            "bad host",
        ] {
            assert_eq!(parse_probe_target(target, None), None, "target: {target}");
        }
    }

    #[test]
    fn accepts_only_public_probe_addresses() {
        for address in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(is_public_probe_ip(address.parse().expect("IP address")));
        }
        for address in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fd00:ec2::254",
        ] {
            assert!(!is_public_probe_ip(address.parse().expect("IP address")));
        }
    }

    #[test]
    fn calculates_median() {
        assert_eq!(median(&mut [4.0, 1.0, 3.0]), 3.0);
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), 2.5);
    }

    #[test]
    fn measures_local_tcp_latency() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let address = listener.local_addr().expect("listener address");
        let accepter = thread::spawn(move || {
            for _ in 0..PROBE_ATTEMPTS {
                listener.accept().expect("accept probe connection");
            }
        });
        let (latency, loss) = tcp_latency_probe_address(&address);
        accepter.join().expect("join listener");
        assert!(latency >= 0.0);
        assert_eq!(loss, 0.0);
    }

    #[test]
    fn parses_ping_latency() {
        assert_eq!(ping_latency("64 bytes time=12.34 ms"), Some(12.34));
        assert_eq!(ping_latency("64 bytes time<1 ms"), Some(0.5));
        assert_eq!(ping_latency("unreachable"), None);
    }
}
