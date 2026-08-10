import {
  Activity,
  ArrowLeft,
  Box,
  Cpu,
  HardDrive,
  Network,
  Radio,
  RadioTower,
  ServerCog,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import {
  Area,
  AreaChart,
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { api } from "../api";
import { demoHistory, demoLatencyHistory, demoLatencyTasks } from "../demo";
import { formatBytes, formatSpeed, formatUptime, isOnline, number } from "../format";
import type { HistoryPoint, LatencySample, LatencyTestPoint, Server } from "../types";
import { Flag, regionDisplayName } from "./Flag";
import { OSIcon } from "./OSIcon";
import { ui } from "../locale";

type ChartType = "load" | "latency";

const LATENCY_COLORS = ["#2563eb", "#db2777", "#ea7b1b", "#0f766e", "#7c3aed", "#0891b2"];

function InfoItem({ icon, label, value }: { icon: React.ReactNode; label: string; value: React.ReactNode }) {
  return <div className="info-item"><span>{icon}</span><div><small>{label}</small><strong>{value}</strong></div></div>;
}

function InfoGroup({ icon, title, children }: { icon: React.ReactNode; title: string; children: React.ReactNode }) {
  return <section className="info-group glass-panel"><h2>{icon}{title}</h2><div className="info-group-grid">{children}</div></section>;
}

function ChartCard({ title, value, children }: { title: string; value: string; children: React.ReactNode }) {
  return <div className="detail-chart glass-panel"><header><h3>{title}</h3><span>{value}</span></header><div>{children}</div></div>;
}

function resourceTicks(total: number): number[] {
  return Array.from({ length: 5 }, (_, index) => total * index / 4);
}

function average(values: number[]): number | null {
  const valid = values.filter((value) => Number.isFinite(value) && value >= 0);
  return valid.length ? valid.reduce((sum, value) => sum + value, 0) / valid.length : null;
}

const tooltipStyle = {
  borderRadius: 8,
  border: "1px solid var(--border)",
  background: "var(--card-solid)",
  fontSize: 12,
};

export function NodeDetails({ server, threshold, retentionDays, locale, demo = false, onClose }: {
  server: Server;
  threshold: number;
  retentionDays: number;
  locale: "zh-CN" | "en";
  demo?: boolean;
  onClose: () => void;
}) {
  const [hours, setHours] = useState(0);
  const [chartType, setChartType] = useState<ChartType>("load");
  const [points, setPoints] = useState<HistoryPoint[]>([]);
  const [latencyPoints, setLatencyPoints] = useState<LatencySample[]>([]);
  const [latencyTasks, setLatencyTasks] = useState<LatencyTestPoint[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    setLoading(true);
    setError("");
    const requestHours = hours === 0 ? 1 : hours;
    if (demo) {
      const taskIds = new Set((server.latency ?? []).map((point) => point.task_id));
      const tasks = demoLatencyTasks.filter((task) => taskIds.has(task.id));
      setPoints(demoHistory(server.id, requestHours));
      setLatencyTasks(tasks);
      setLatencyPoints(tasks.length ? demoLatencyHistory(server.id, requestHours).filter((point) => taskIds.has(point.task_id)) : []);
      setLoading(false);
      return;
    }
    let active = true;
    const serverId = server.source_id || server.id;
    const baseUrl = server.source_url || "";
    Promise.all([api.history(serverId, requestHours, baseUrl), api.latencyHistory(serverId, requestHours, baseUrl)])
      .then(([history, latency]) => { if (active) { setPoints(history.points); setLatencyTasks(latency.tasks); setLatencyPoints(latency.points); } })
      .catch((reason) => { if (active) { setPoints([]); setLatencyTasks([]); setLatencyPoints([]); setError(reason instanceof Error ? reason.message : ui(locale, "历史数据加载失败", "Unable to load history")); } })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [server.id, server.source_id, server.source_url, hours, demo, locale]);

  const online = isOnline(server, threshold);
  const loadRanges = [
    { value: 0, label: ui(locale, "实时", "Live") },
    { value: 1, label: ui(locale, "1 小时", "1 hour") },
    { value: 4, label: ui(locale, "4 小时", "4 hours") },
    { value: 24, label: ui(locale, "1 天", "1 day") },
    { value: 168, label: ui(locale, "7 天", "7 days") },
    { value: 720, label: ui(locale, "30 天", "30 days") },
  ].filter((range) => range.value <= Math.max(24, retentionDays * 24));
  const ranges = chartType === "load" ? loadRanges : loadRanges.filter((range) => range.value > 0);
  const data = useMemo(() => points.map((point) => ({
    ...point,
    time: new Date(point.timestamp * 1000).toLocaleString(locale, hours >= 24
      ? { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }
      : { hour: "2-digit", minute: "2-digit", hour12: false }),
  })), [points, hours, locale]);
  const latencyData = useMemo(() => {
    const grouped = new Map<number, Record<string, number | string | null>>();
    for (const point of latencyPoints) {
      const row = grouped.get(point.timestamp) ?? {
        timestamp: point.timestamp,
        time: new Date(point.timestamp * 1000).toLocaleString(locale, hours >= 24
          ? { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }
          : { hour: "2-digit", minute: "2-digit", hour12: false }),
      };
      row[`latency_${point.task_id}`] = point.latency_ms >= 0 ? point.latency_ms : null;
      row[`loss_${point.task_id}`] = point.packet_loss;
      grouped.set(point.timestamp, row);
    }
    return Array.from(grouped.values()).sort((left, right) => Number(left.timestamp) - Number(right.timestamp));
  }, [hours, latencyPoints, locale]);
  const latencySeries = useMemo(() => {
    return latencyTasks.map((task, index) => {
      const samples = latencyPoints.filter((point) => point.task_id === task.id);
      return {
        id: task.id,
        name: task.name,
        dataKey: `latency_${task.id}`,
        color: LATENCY_COLORS[index % LATENCY_COLORS.length],
        latency: average(samples.map((sample) => sample.latency_ms)),
        loss: average(samples.map((sample) => sample.packet_loss)),
      };
    });
  }, [latencyPoints, latencyTasks]);
  const last = data[data.length - 1];
  const pingEnabled = latencyTasks.length > 0;

  function selectChart(next: ChartType) {
    setChartType(next);
    if (next === "latency" && hours === 0) setHours(1);
  }

  return (
    <div className="detail-page">
      <section className="detail-hero glass-panel">
        <button className="icon-btn" onClick={onClose} title={ui(locale, "返回首页", "Back")}><ArrowLeft size={19} /></button>
        <Flag region={server.region} size={24} className="detail-flag" locale={locale} />
        <div className="detail-title">
          <div><h1>{server.name}</h1><span className={`status-chip ${online ? "online" : ""}`}>{online ? ui(locale, "在线", "Online") : ui(locale, "离线", "Offline")}</span></div>
          <p><OSIcon os={server.os} size={14} />{regionDisplayName(server.region, locale)}{server.group_name ? ` · ${server.group_name}` : ""}{server.source_name && server.source_url ? ` · ${server.source_name}` : ""}</p>
        </div>
      </section>

      <div className="info-groups">
        <InfoGroup title={ui(locale, "硬件信息", "Hardware")} icon={<Cpu size={16} />}>
          <InfoItem icon={<Cpu size={14} />} label={ui(locale, "处理器", "Processor")} value={`${server.cpu_model || "--"} (x${server.cpu_cores || 0})`} />
          <InfoItem icon={<Box size={14} />} label={ui(locale, "架构", "Architecture")} value={server.arch || "--"} />
          <InfoItem icon={<Cpu size={14} />} label={ui(locale, "图形设备", "Graphics")} value={server.gpus?.length ? server.gpus.map((gpu) => `${gpu.model} ${gpu.usage.toFixed(1)}%`).join(" · ") : server.gpu_model || ui(locale, "未检测到", "Not detected")} />
          <InfoItem icon={<HardDrive size={14} />} label={ui(locale, "存储设备", "Storage")} value={server.disks?.length ? server.disks.map((disk) => `${disk.mount_point} ${formatBytes(disk.used)} / ${formatBytes(disk.total)}`).join(" · ") : `${formatBytes(server.disk_used)} / ${formatBytes(server.disk_total)}`} />
          <InfoItem icon={<ServerCog size={14} />} label={ui(locale, "虚拟化", "Virtualization")} value={server.virtualization || "--"} />
        </InfoGroup>
        <InfoGroup title={ui(locale, "系统信息", "System")} icon={<ServerCog size={16} />}>
          <InfoItem icon={<ServerCog size={14} />} label={ui(locale, "操作系统", "Operating system")} value={server.os || "--"} />
          <InfoItem icon={<Radio size={14} />} label={ui(locale, "运行时间", "Uptime")} value={online ? formatUptime(server.uptime, locale) : "--"} />
          <InfoItem icon={<Cpu size={14} />} label={ui(locale, "内核版本", "Kernel")} value={server.kernel || "--"} />
          <InfoItem icon={<Network size={14} />} label={ui(locale, "进程 / 连接", "Processes / connections")} value={online ? `${server.processes || 0} / ${number(server.tcp_connections) + number(server.udp_connections)}` : "--"} />
          <InfoItem icon={<HardDrive size={14} />} label={ui(locale, "磁盘 IO", "Disk IO")} value={`${number(server.disk_await_ms).toFixed(1)} ms · ${number(server.disk_utilization).toFixed(1)}%`} />
        </InfoGroup>
      </div>

      <section className="chart-section">
        <div className="chart-controls">
          <div className="segmented" aria-label="图表类型">
            <button className={chartType === "load" ? "active" : ""} onClick={() => selectChart("load")}><Activity size={14} />{ui(locale, "负载", "Load")}</button>
            {pingEnabled ? <button className={chartType === "latency" ? "active" : ""} onClick={() => selectChart("latency")}><RadioTower size={14} />{ui(locale, "延迟", "Latency")}</button> : null}
          </div>
          <div className="segmented range-control" aria-label="时间范围">
            {ranges.map((range) => <button className={hours === range.value ? "active" : ""} key={range.value} onClick={() => setHours(range.value)}>{range.label}</button>)}
          </div>
        </div>

        {error ? <div className="error-band">{error}</div> : null}
        {loading ? <div className="chart-loading">{ui(locale, "正在读取历史数据", "Loading history")}</div> : chartType === "load" && !data.length ? <div className="chart-loading">{ui(locale, "暂无历史数据", "No history")}</div> : chartType === "latency" && !(latencyData.length || data.length) ? <div className="chart-loading">{ui(locale, "暂无延迟数据", "No latency data")}</div> : chartType === "load" ? (
          <div className="detail-charts-grid">
            <ChartCard title="CPU" value={`${number(last.cpu).toFixed(1)}%`}>
              <ResponsiveContainer width="100%" height="100%"><AreaChart data={data}><CartesianGrid stroke="var(--chart-grid)" strokeDasharray="3 3" /><XAxis dataKey="time" tick={{ fontSize: 10 }} minTickGap={30} /><YAxis domain={[0, 100]} tick={{ fontSize: 10 }} width={36} /><Tooltip contentStyle={tooltipStyle} formatter={(value) => [`${Number(value).toFixed(1)}%`, "CPU"]} /><Area type="monotone" dataKey="cpu" stroke="var(--danger)" fill="var(--danger)" fillOpacity={0.15} strokeWidth={2} dot={false} isAnimationActive={false} /></AreaChart></ResponsiveContainer>
            </ChartCard>
            <ChartCard title={ui(locale, "内存", "Memory")} value={`${formatBytes(last.mem_used)} / ${formatBytes(last.mem_total)}`}>
              <ResponsiveContainer width="100%" height="100%"><AreaChart data={data}><CartesianGrid stroke="var(--chart-grid)" strokeDasharray="3 3" /><XAxis dataKey="time" tick={{ fontSize: 10 }} minTickGap={30} /><YAxis domain={[0, last.mem_total]} ticks={resourceTicks(last.mem_total)} tick={{ fontSize: 10 }} width={56} tickFormatter={(value) => formatBytes(Number(value), 0)} /><Tooltip contentStyle={tooltipStyle} formatter={(value) => [formatBytes(Number(value)), "内存"]} /><Area type="monotone" dataKey="mem_used" stroke="var(--primary)" fill="var(--primary)" fillOpacity={0.15} strokeWidth={2} dot={false} isAnimationActive={false} /></AreaChart></ResponsiveContainer>
            </ChartCard>
            <ChartCard title={ui(locale, "硬盘", "Disk")} value={`${formatBytes(last.disk_used)} / ${formatBytes(last.disk_total)}`}>
              <ResponsiveContainer width="100%" height="100%"><AreaChart data={data}><CartesianGrid stroke="var(--chart-grid)" strokeDasharray="3 3" /><XAxis dataKey="time" tick={{ fontSize: 10 }} minTickGap={30} /><YAxis domain={[0, last.disk_total]} ticks={resourceTicks(last.disk_total)} tick={{ fontSize: 10 }} width={56} tickFormatter={(value) => formatBytes(Number(value), 0)} /><Tooltip contentStyle={tooltipStyle} formatter={(value) => [formatBytes(Number(value)), "硬盘"]} /><Area type="monotone" dataKey="disk_used" stroke="var(--warning)" fill="var(--warning)" fillOpacity={0.15} strokeWidth={2} dot={false} isAnimationActive={false} /></AreaChart></ResponsiveContainer>
            </ChartCard>
            <ChartCard title={ui(locale, "网络", "Network")} value={`↑ ${formatSpeed(last.net_out)}  ↓ ${formatSpeed(last.net_in)}`}>
              <ResponsiveContainer width="100%" height="100%"><AreaChart data={data}><CartesianGrid stroke="var(--chart-grid)" strokeDasharray="3 3" /><XAxis dataKey="time" tick={{ fontSize: 10 }} minTickGap={30} /><YAxis tick={{ fontSize: 10 }} width={48} tickFormatter={(value) => formatBytes(Number(value), 0)} /><Tooltip contentStyle={tooltipStyle} formatter={(value, name) => [formatSpeed(Number(value)), name === "net_out" ? "上行" : "下行"]} /><Area type="monotone" dataKey="net_out" stroke="var(--success)" fill="var(--success)" fillOpacity={0.12} strokeWidth={2} dot={false} isAnimationActive={false} /><Area type="monotone" dataKey="net_in" stroke="var(--info)" fill="var(--info)" fillOpacity={0.1} strokeWidth={2} dot={false} isAnimationActive={false} /></AreaChart></ResponsiveContainer>
            </ChartCard>
          </div>
        ) : (
          <section className="latency-overview glass-panel">
            <div className="latency-summary-grid" style={{ gridTemplateColumns: `repeat(${Math.max(1, latencySeries.length)}, minmax(180px, 1fr))` }}>
              {latencySeries.map((series) => <div className="latency-summary" key={series.id}>
                <span><i style={{ backgroundColor: series.color }} />{series.name}</span>
                <strong>{series.latency == null ? "--" : `${series.latency.toFixed(2)} ms`}</strong>
                <small>{series.loss == null ? "--" : `${series.loss.toFixed(2)}%`} 平均丢包</small>
              </div>)}
            </div>
            <div className="latency-line-chart">
              <ResponsiveContainer width="100%" height="100%">
                <LineChart data={latencyData.length ? latencyData : data} margin={{ top: 10, right: 12, left: 4, bottom: 2 }}>
                  <CartesianGrid stroke="var(--chart-grid)" strokeDasharray="3 3" />
                  <XAxis dataKey="time" tick={{ fontSize: 10 }} minTickGap={40} />
                  <YAxis tick={{ fontSize: 10 }} width={48} unit="ms" />
                  <Tooltip contentStyle={tooltipStyle} formatter={(value, name) => [`${Number(value).toFixed(1)} ms`, name]} />
                  <Legend iconType="rect" iconSize={9} wrapperStyle={{ fontSize: 11, paddingTop: 10 }} />
                  {latencySeries.map((series) => <Line key={series.id} type="monotone" dataKey={series.dataKey} name={series.name} stroke={series.color} strokeWidth={2} dot={false} connectNulls={false} isAnimationActive={false} />)}
                </LineChart>
              </ResponsiveContainer>
            </div>
          </section>
        )}
      </section>

    </div>
  );
}
