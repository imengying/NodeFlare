use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::net::UdpSocket;
use std::net::{TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
#[cfg(target_os = "windows")]
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use sysinfo::{Disks, Networks, System};

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
compile_error!("cf-monitor-agent supports Linux, Windows, and macOS");

const VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASE_DOWNLOAD_BASE: &str =
    "https://github.com/imengying/CF-Monitor/releases/latest/download";
const PROBE_ATTEMPTS: usize = 4;
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_AGENT_BINARY_BYTES: u64 = 64 * 1024 * 1024;

type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
struct RuntimeConfig {
    server_id: String,
    token: String,
    worker_url: String,
    report_interval: u64,
    collect_interval: u64,
    network_interface: String,
    latency_tasks: Vec<LatencyTask>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct LatencyTask {
    id: String,
    name: String,
    task_type: String,
    target: String,
    interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
struct LatencyResult {
    task_id: String,
    timestamp: i64,
    latency_ms: f64,
    packet_loss: f64,
}

#[derive(Debug, Default, Deserialize)]
struct RemoteConfig {
    #[serde(default)]
    report_interval: u64,
    #[serde(default)]
    collect_interval: u64,
    #[serde(default)]
    network_interface: String,
    auto_update: i64,
    #[serde(default)]
    latest_agent_version: String,
    #[serde(default)]
    latency_tasks: Vec<LatencyTask>,
}

#[derive(Debug, Deserialize)]
struct ReportResponse {
    config: Option<RemoteConfig>,
}

#[derive(Debug, Serialize)]
struct ReportBatch<'a> {
    server_id: &'a str,
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

#[derive(Debug, Default, Clone, Serialize)]
struct GpuMetric {
    model: String,
    usage: f64,
    memory_used: i64,
    memory_total: i64,
}

#[derive(Debug, Default, Serialize)]
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
    ipv4: String,
    ipv6: String,
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
    message: String,
    latency_results: Vec<LatencyResult>,
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

#[cfg(target_os = "linux")]
fn ip_address(family: &str) -> String {
    command("ip", &["-o", family, "addr", "show", "scope", "global"])
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().find(|value| value.contains('/')))
        .and_then(|value| value.split('/').next())
        .unwrap_or("")
        .to_string()
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
        return labels.iter().all(|label| label.parse::<u8>().is_ok());
    }
    labels.iter().all(|label| {
        !label.is_empty()
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

fn parse_probe_target(value: &str) -> Option<(String, u16)> {
    let raw = value.trim();
    if raw.is_empty()
        || raw.len() > 60
        || raw.contains("://")
        || raw
            .chars()
            .any(|character| character.is_whitespace() || "/@?#\\[]".contains(character))
        || raw.matches(':').count() > 1
    {
        return None;
    }
    let (host, port) = raw.split_once(':').map_or((raw, 443), |(host, port)| {
        (host, port.parse::<u16>().unwrap_or(0))
    });
    (port > 0 && valid_probe_host(host)).then(|| (host.to_ascii_lowercase(), port))
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

fn tcp_latency_probe(target: &str) -> (f64, f64) {
    if target.trim().is_empty() {
        return (-1.0, -1.0);
    }
    let Some((host, port)) = parse_probe_target(target) else {
        return (-1.0, 100.0);
    };
    let Ok(addresses) = (host.as_str(), port).to_socket_addrs() else {
        return (-1.0, 100.0);
    };
    let addresses = addresses.collect::<Vec<_>>();
    let Some(address) = addresses
        .iter()
        .find(|address| address.is_ipv4())
        .or_else(|| addresses.first())
        .copied()
    else {
        return (-1.0, 100.0);
    };

    let mut latencies = Vec::with_capacity(PROBE_ATTEMPTS);
    for _ in 0..PROBE_ATTEMPTS {
        let started = Instant::now();
        if TcpStream::connect_timeout(&address, PROBE_TIMEOUT).is_ok() {
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
    if host.contains(':') || !valid_probe_host(host) {
        return (-1.0, 100.0);
    }
    let mut latencies = Vec::with_capacity(PROBE_ATTEMPTS);
    for _ in 0..PROBE_ATTEMPTS {
        let started = Instant::now();
        let mut ping = Command::new("ping");
        #[cfg(target_os = "linux")]
        ping.args(["-n", "-c", "1", "-W", "1", host]);
        #[cfg(target_os = "macos")]
        ping.args(["-n", "-c", "1", "-W", "1000", host]);
        #[cfg(target_os = "windows")]
        ping.args(["-n", "1", "-w", "1000", host]);
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

fn execute_latency_tasks(tasks: &[LatencyTask]) -> Vec<LatencyResult> {
    let mut results = Vec::with_capacity(tasks.len());
    thread::scope(|scope| {
        let handles = tasks
            .iter()
            .cloned()
            .map(|task| {
                scope.spawn(move || {
                    let (latency_ms, packet_loss) = match task.task_type.as_str() {
                        "tcp" => tcp_latency_probe(&task.target),
                        "icmp" => icmp_latency_probe(&task.target),
                        _ => (-1.0, 100.0),
                    };
                    LatencyResult {
                        task_id: task.id,
                        timestamp: unix_timestamp(),
                        latency_ms,
                        packet_loss,
                    }
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            if let Ok(result) = handle.join() {
                results.push(result);
            }
        }
    });
    results
}

fn gpu_info() -> Vec<GpuMetric> {
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
            Some(GpuMetric {
                usage: fields[0].parse().unwrap_or(0.0),
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

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn local_ip(bind: &str, destination: &str) -> String {
    UdpSocket::bind(bind)
        .and_then(|socket| {
            socket.connect(destination)?;
            socket.local_addr()
        })
        .map(|address| address.ip().to_string())
        .unwrap_or_default()
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

#[cfg(target_os = "linux")]
fn collect(config: &RuntimeConfig, latency_results: Vec<LatencyResult>) -> Report {
    let cpu_before = cpu_sample();
    let io_before = io_sample(&config.network_interface);
    thread::sleep(Duration::from_secs(1));
    let cpu_after = cpu_sample();
    let io_after = io_sample(&config.network_interface);
    let total = cpu_after.total.saturating_sub(cpu_before.total);
    let idle = cpu_after.idle.saturating_sub(cpu_before.idle);
    let cpu = if total == 0 {
        0.0
    } else {
        (total.saturating_sub(idle)) as f64 * 100.0 / total as f64
    };

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
    let gpus = gpu_info();
    let gpu_usage = if gpus.is_empty() {
        0.0
    } else {
        gpus.iter().map(|gpu| gpu.usage).sum::<f64>() / gpus.len() as f64
    };
    let gpu_model = gpus
        .iter()
        .map(|gpu| gpu.model.as_str())
        .collect::<Vec<_>>()
        .join(" · ");
    let read_ops = io_after.read_ops.saturating_sub(io_before.read_ops);
    let write_ops = io_after.write_ops.saturating_sub(io_before.write_ops);
    let total_ops = read_ops.saturating_add(write_ops);
    let io_wait = io_after
        .read_millis
        .saturating_sub(io_before.read_millis)
        .saturating_add(io_after.write_millis.saturating_sub(io_before.write_millis));

    Report {
        timestamp: unix_timestamp(),
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
        net_in: io_after.rx.saturating_sub(io_before.rx) as f64,
        net_out: io_after.tx.saturating_sub(io_before.tx) as f64,
        net_rx_total: io_after.rx.min(i64::MAX as u64) as i64,
        net_tx_total: io_after.tx.min(i64::MAX as u64) as i64,
        uptime,
        processes,
        tcp_connections: file_line_count("/proc/net/tcp") + file_line_count("/proc/net/tcp6"),
        udp_connections: file_line_count("/proc/net/udp") + file_line_count("/proc/net/udp6"),
        cpu_cores: thread::available_parallelism()
            .map(|value| value.get() as i64)
            .unwrap_or(1),
        cpu_model: cpu_model(),
        os: os_name(),
        kernel: command("uname", &["-r"]),
        arch: env::consts::ARCH.to_string(),
        virtualization: command("systemd-detect-virt", &[]),
        ipv4: ip_address("-4"),
        ipv6: ip_address("-6"),
        gpu_usage,
        gpu_model,
        agent_version: VERSION.to_string(),
        message: env::var("AGENT_MESSAGE").unwrap_or_default(),
        disk_read_bps: io_after
            .read_sectors
            .saturating_sub(io_before.read_sectors)
            .saturating_mul(512) as f64,
        disk_write_bps: io_after
            .write_sectors
            .saturating_sub(io_before.write_sectors)
            .saturating_mul(512) as f64,
        disk_read_iops: io_after.read_ops.saturating_sub(io_before.read_ops) as f64,
        disk_write_iops: io_after.write_ops.saturating_sub(io_before.write_ops) as f64,
        disk_await_ms: if total_ops == 0 {
            0.0
        } else {
            io_wait as f64 / total_ops as f64
        },
        disk_utilization: (io_after.io_millis.saturating_sub(io_before.io_millis) as f64 / 10.0)
            .clamp(0.0, 100.0),
        disks,
        gpus,
        latency_results,
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn collect(config: &RuntimeConfig, latency_results: Vec<LatencyResult>) -> Report {
    let mut system = System::new_all();
    let mut networks = Networks::new_with_refreshed_list();
    thread::sleep(Duration::from_secs(1));
    system.refresh_all();
    networks.refresh(true);

    let mut net_in = 0_u64;
    let mut net_out = 0_u64;
    let mut net_rx_total = 0_u64;
    let mut net_tx_total = 0_u64;
    for (name, network) in &networks {
        if !selected_interface(name, &config.network_interface) {
            continue;
        }
        net_in = net_in.saturating_add(network.received());
        net_out = net_out.saturating_add(network.transmitted());
        net_rx_total = net_rx_total.saturating_add(network.total_received());
        net_tx_total = net_tx_total.saturating_add(network.total_transmitted());
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
    let disk_used = disk_metrics.iter().map(|disk| disk.used).sum();
    let disk_total = disk_metrics.iter().map(|disk| disk.total).sum();
    let load = System::load_average();
    let gpus = gpu_info();
    let gpu_usage = if gpus.is_empty() {
        0.0
    } else {
        gpus.iter().map(|gpu| gpu.usage).sum::<f64>() / gpus.len() as f64
    };
    let gpu_model = gpus
        .iter()
        .map(|gpu| gpu.model.as_str())
        .collect::<Vec<_>>()
        .join(" · ");

    Report {
        timestamp: unix_timestamp(),
        cpu: system.global_cpu_usage() as f64,
        load1: load.one,
        load5: load.five,
        load15: load.fifteen,
        mem_used: u64_to_i64(system.used_memory()),
        mem_total: u64_to_i64(system.total_memory()),
        swap_used: u64_to_i64(system.used_swap()),
        swap_total: u64_to_i64(system.total_swap()),
        disk_used,
        disk_total,
        net_in: net_in as f64,
        net_out: net_out as f64,
        net_rx_total: u64_to_i64(net_rx_total),
        net_tx_total: u64_to_i64(net_tx_total),
        uptime: u64_to_i64(System::uptime()),
        processes: system.processes().len() as i64,
        tcp_connections: 0,
        udp_connections: 0,
        cpu_cores: system.cpus().len().max(1) as i64,
        cpu_model: system
            .cpus()
            .first()
            .map(|cpu| cpu.brand().to_string())
            .unwrap_or_default(),
        os: System::long_os_version().unwrap_or_else(|| env::consts::OS.to_string()),
        kernel: System::kernel_version().unwrap_or_default(),
        arch: env::consts::ARCH.to_string(),
        virtualization: String::new(),
        ipv4: local_ip("0.0.0.0:0", "1.1.1.1:80"),
        ipv6: local_ip("[::]:0", "[2606:4700:4700::1111]:80"),
        gpu_usage,
        gpu_model,
        agent_version: VERSION.to_string(),
        disk_read_bps: 0.0,
        disk_write_bps: 0.0,
        disk_read_iops: 0.0,
        disk_write_iops: 0.0,
        disk_await_ms: 0.0,
        disk_utilization: 0.0,
        disks: disk_metrics,
        gpus,
        message: env::var("AGENT_MESSAGE").unwrap_or_default(),
        latency_results,
    }
}

fn runtime_config() -> Result<RuntimeConfig> {
    let required = |key: &str| -> Result<String> {
        env::var(key).map_err(|_| format!("missing environment variable {key}").into())
    };
    let interval = env::var("REPORT_INTERVAL")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60)
        .clamp(15, 3600);
    let collect_interval = env::var("COLLECT_INTERVAL")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(5)
        .clamp(2, 60)
        .min(interval);
    Ok(RuntimeConfig {
        server_id: required("SERVER_ID")?,
        token: required("AGENT_TOKEN")?,
        worker_url: required("WORKER_URL")?.trim_end_matches('/').to_string(),
        report_interval: interval,
        collect_interval,
        network_interface: env::var("NETWORK_INTERFACE").unwrap_or_default(),
        latency_tasks: Vec::new(),
    })
}

fn submit(
    agent: &ureq::Agent,
    config: &RuntimeConfig,
    reports: &[Report],
) -> Result<Option<RemoteConfig>> {
    let batch = ReportBatch {
        server_id: &config.server_id,
        samples: reports,
    };
    let response = agent
        .post(&format!("{}/api/agent/report", config.worker_url))
        .set("Authorization", &format!("Bearer {}", config.token))
        .send_json(batch)?;
    Ok(response.into_json::<ReportResponse>()?.config)
}

fn apply_remote(config: &mut RuntimeConfig, remote: &RemoteConfig) -> bool {
    if (15..=3600).contains(&remote.report_interval) {
        config.report_interval = remote.report_interval;
    }
    if (2..=60).contains(&remote.collect_interval) {
        config.collect_interval = remote.collect_interval.min(config.report_interval);
    }
    config
        .network_interface
        .clone_from(&remote.network_interface);
    let changed = config.latency_tasks != remote.latency_tasks;
    config.latency_tasks.clone_from(&remote.latency_tasks);
    changed
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

fn download_agent(
    agent: &ureq::Agent,
    url: &str,
    destination: &Path,
    expected_version: &str,
) -> Result<bool> {
    let Ok(response) = agent.get(url).call() else {
        return Ok(false);
    };
    let response = response.into_reader();
    let mut file = fs::File::create(destination)?;
    let copied = io::copy(&mut response.take(MAX_AGENT_BINARY_BYTES + 1), &mut file)?;
    file.flush()?;
    drop(file);
    if copied > MAX_AGENT_BINARY_BYTES || !executable_format_valid(destination) {
        let _ = fs::remove_file(destination);
        return Ok(false);
    }
    #[cfg(unix)]
    fs::set_permissions(destination, fs::Permissions::from_mode(0o755))?;

    let downloaded_version = Command::new(destination)
        .arg("version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    let valid = downloaded_version.as_deref().map(normalized_version)
        == Some(normalized_version(expected_version));
    if !valid {
        let _ = fs::remove_file(destination);
    }
    Ok(valid)
}

fn update(agent: &ureq::Agent, config: &RuntimeConfig, remote: &RemoteConfig) -> Result<bool> {
    let Some(remote_version) = version_triplet(&remote.latest_agent_version) else {
        return Ok(false);
    };
    let Some(current_version) = version_triplet(VERSION) else {
        return Ok(false);
    };
    if remote.auto_update != 1 || remote_version <= current_version {
        return Ok(false);
    }
    let Some(artifact) = agent_artifact_name() else {
        return Ok(false);
    };
    let current = env::current_exe()?;
    #[cfg(target_os = "windows")]
    let temporary = current.with_extension("update.exe");
    #[cfg(not(target_os = "windows"))]
    let temporary = current.with_extension("update");
    let primary = format!("{}/{}", config.worker_url, artifact);
    let fallback = format!("{RELEASE_DOWNLOAD_BASE}/{artifact}");
    if !download_agent(agent, &primary, &temporary, &remote.latest_agent_version)?
        && !download_agent(agent, &fallback, &temporary, &remote.latest_agent_version)?
    {
        return Err("downloaded agent version does not match the Worker version".into());
    }

    #[cfg(unix)]
    {
        fs::rename(&temporary, &current)?;
        let error = Command::new(&current).arg("run").exec();
        Err(error.into())
    }

    #[cfg(target_os = "windows")]
    {
        let script = concat!(
            "$ErrorActionPreference='Stop'; ",
            "$targetPid=[int]$env:CF_MONITOR_UPDATE_PID; ",
            "Wait-Process -Id $targetPid; ",
            "Move-Item -LiteralPath $env:CF_MONITOR_UPDATE_NEW ",
            "-Destination $env:CF_MONITOR_UPDATE_CURRENT -Force; ",
            "Start-Process -FilePath $env:CF_MONITOR_UPDATE_CURRENT -ArgumentList 'run'"
        );
        Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .env("CF_MONITOR_UPDATE_PID", std::process::id().to_string())
            .env("CF_MONITOR_UPDATE_NEW", &temporary)
            .env("CF_MONITOR_UPDATE_CURRENT", &current)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(true)
    }
}

fn run(once: bool, print_only: bool) -> Result<()> {
    let mut config = runtime_config()?;
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .build();
    let mut next_report = Instant::now();
    let mut next_collect = Instant::now();
    let mut next_latency: HashMap<String, Instant> = HashMap::new();
    let mut pending_results = Vec::new();
    let mut pending_samples = Vec::new();
    loop {
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
            pending_results.extend(execute_latency_tasks(&due));
            let scheduled_at = Instant::now();
            for task in due {
                next_latency.insert(
                    task.id,
                    scheduled_at + Duration::from_secs(task.interval_seconds.clamp(30, 3600)),
                );
            }
        }

        if Instant::now() >= next_collect {
            let mut latest_results = HashMap::new();
            for result in std::mem::take(&mut pending_results) {
                latest_results.insert(result.task_id.clone(), result);
            }
            let report = collect(&config, latest_results.into_values().collect());
            if print_only {
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            pending_samples.push(report);
            if pending_samples.len() > 720 {
                let overflow = pending_samples.len() - 720;
                pending_samples.drain(..overflow);
            }
            next_collect = Instant::now() + Duration::from_secs(config.collect_interval);
        }

        if Instant::now() >= next_report && !pending_samples.is_empty() {
            match submit(&agent, &config, &pending_samples) {
                Ok(remote) => {
                    pending_samples.clear();
                    if let Some(remote) = remote {
                        if update(&agent, &config, &remote)? {
                            return Ok(());
                        }
                        if apply_remote(&mut config, &remote) {
                            next_latency.clear();
                        }
                    }
                }
                Err(error) => {
                    eprintln!("report failed: {error}");
                }
            }
            next_report = Instant::now() + Duration::from_secs(config.report_interval);
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
    let result = match env::args().nth(1).as_deref() {
        Some("run") => run(false, false),
        Some("once") => run(true, false),
        Some("collect") => run(true, true),
        Some("version") | Some("--version") => {
            println!("{VERSION}");
            Ok(())
        }
        _ => Err("usage: cf-monitor-agent <run|once|collect|version>".into()),
    };
    if let Err(error) = result {
        eprintln!("cf-monitor-agent: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;

    use super::{
        agent_artifact_name, executable_format_valid, median, normalized_version,
        parse_probe_target, ping_latency, selected_interface, tcp_latency_probe, version_triplet,
        PROBE_ATTEMPTS,
    };

    #[cfg(target_os = "linux")]
    use super::disk_device;

    #[test]
    fn selects_network_interfaces() {
        assert!(selected_interface("eth0", ""));
        assert!(!selected_interface("lo", ""));
        assert!(selected_interface("ens3", "eth0, ens3"));
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
            parse_probe_target("Example.COM"),
            Some(("example.com".to_string(), 443))
        );
        assert_eq!(
            parse_probe_target("127.0.0.1:8080"),
            Some(("127.0.0.1".to_string(), 8080))
        );
        for target in [
            "",
            "https://example.com",
            "example.com:0",
            "example.com:65536",
            "999.1.1.1",
            "[::1]:443",
            "bad host",
        ] {
            assert_eq!(parse_probe_target(target), None, "target: {target}");
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
        let (latency, loss) = tcp_latency_probe(&address.to_string());
        accepter.join().expect("join listener");
        assert!(latency >= 0.0);
        assert_eq!(loss, 0.0);
        assert_eq!(tcp_latency_probe(""), (-1.0, -1.0));
    }

    #[test]
    fn parses_ping_latency() {
        assert_eq!(ping_latency("64 bytes time=12.34 ms"), Some(12.34));
        assert_eq!(ping_latency("64 bytes time<1 ms"), Some(0.5));
        assert_eq!(ping_latency("unreachable"), None);
    }
}
