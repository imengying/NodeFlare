import {
  Activity,
  ArrowLeft,
  Box,
  Cpu,
  Network,
  Radio,
  RadioTower,
  ServerCog,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
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
import { displayGpuDevices, formatBytes, formatCpuName, formatSpeed, formatUptime, isOnline, number } from "../format";
import type { HistoryPoint, LatencySample, LatencyTestPoint, LiveLatencyResult, Server } from "../types";
import { Flag, regionDisplayName } from "./Flag";
import { OSIcon } from "./OSIcon";
import { ui } from "../locale";

type ChartType = "load" | "latency";

const LATENCY_COLORS = ["#2563eb", "#db2777", "#ea7b1b", "#0f766e", "#7c3aed", "#0891b2"];
const REALTIME_WINDOW_SECONDS = 60 * 60;
const MAX_REALTIME_POINTS = 720;

function historyPointFromServer(server: Server): HistoryPoint | null {
  if (!Number.isFinite(server.timestamp) || number(server.timestamp) <= 0) return null;
  return {
    timestamp: number(server.timestamp),
    cpu: number(server.cpu),
    load1: number(server.load1),
    load5: number(server.load5),
    load15: number(server.load15),
    mem_used: number(server.mem_used),
    mem_total: number(server.mem_total),
    swap_used: number(server.swap_used),
    swap_total: number(server.swap_total),
    disk_used: number(server.disk_used),
    disk_total: number(server.disk_total),
    net_in: number(server.net_in),
    net_out: number(server.net_out),
    net_rx_total: number(server.net_rx_total),
    net_tx_total: number(server.net_tx_total),
    processes: number(server.processes),
    tcp_connections: number(server.tcp_connections),
    udp_connections: number(server.udp_connections),
    gpu_usage: number(server.gpu_usage),
    disk_read_bps: number(server.disk_read_bps),
    disk_write_bps: number(server.disk_write_bps),
    disk_read_iops: number(server.disk_read_iops),
    disk_write_iops: number(server.disk_write_iops),
    disk_await_ms: number(server.disk_await_ms),
    disk_utilization: number(server.disk_utilization),
  };
}

function appendRealtimePoint(points: HistoryPoint[], point: HistoryPoint | null): HistoryPoint[] {
  if (!point) return points;
  const cutoff = point.timestamp - REALTIME_WINDOW_SECONDS;
  const next = points.filter((current) => current.timestamp >= cutoff && current.timestamp !== point.timestamp);
  next.push(point);
  next.sort((left, right) => left.timestamp - right.timestamp);
  return next.slice(-MAX_REALTIME_POINTS);
}

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

export function NodeDetails({ server, liveLatencyResults, threshold, retentionDays, locale, demo = false, onClose }: {
  server: Server;
  liveLatencyResults: LiveLatencyResult[];
  threshold: number;
  retentionDays: number;
  locale: "zh-CN" | "en";
  demo?: boolean;
  onClose: () => void;
}) {
  const [loadHours, setLoadHours] = useState(0);
  const [latencyHours, setLatencyHours] = useState(1);
  const [chartType, setChartType] = useState<ChartType>("load");
  const [points, setPoints] = useState<HistoryPoint[]>([]);
  const [latencyPoints, setLatencyPoints] = useState<LatencySample[]>([]);
  const [latencyTasks, setLatencyTasks] = useState<LatencyTestPoint[]>([]);
  const [loadLoading, setLoadLoading] = useState(true);
  const [latencyLoading, setLatencyLoading] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [latencyError, setLatencyError] = useState("");
  const latestServerRef = useRef(server);
  latestServerRef.current = server;

  useEffect(() => {
    setLoadLoading(true);
    setLoadError("");
    const requestHours = loadHours === 0 ? 1 : loadHours;
    if (demo) {
      setPoints(demoHistory(server.id, requestHours));
      setLoadLoading(false);
      return;
    }
    let active = true;
    api.history(server.id, requestHours)
      .then((history) => {
        if (active) setPoints(loadHours === 0
          ? appendRealtimePoint(history.points, historyPointFromServer(latestServerRef.current))
          : history.points);
      })
      .catch((reason) => { if (active) { setPoints([]); setLoadError(reason instanceof Error ? reason.message : ui(locale, "历史数据加载失败", "Unable to load history")); } })
      .finally(() => { if (active) setLoadLoading(false); });
    return () => { active = false; };
  }, [server.id, loadHours, demo, locale]);

  useEffect(() => {
    if (demo || loadHours !== 0) return;
    const point = historyPointFromServer(server);
    if (point) setPoints((current) => appendRealtimePoint(current, point));
  }, [demo, loadHours, server]);

  useEffect(() => {
    setLatencyLoading(true);
    setLatencyError("");
    if (demo) {
      const taskIds = new Set(server.latency.map((point) => point.task_id));
      const tasks = demoLatencyTasks.filter((task) => taskIds.has(task.id));
      setLatencyTasks(tasks);
      setLatencyPoints(tasks.length ? demoLatencyHistory(server.id, latencyHours).filter((point) => taskIds.has(point.task_id)) : []);
      setLatencyLoading(false);
      return;
    }
    let active = true;
    api.latencyHistory(server.id, latencyHours)
      .then((latency) => { if (active) { setLatencyTasks(latency.tasks); setLatencyPoints(latency.points); } })
      .catch((reason) => { if (active) { setLatencyPoints([]); setLatencyError(reason instanceof Error ? reason.message : ui(locale, "延迟数据加载失败", "Unable to load latency")); } })
      .finally(() => { if (active) setLatencyLoading(false); });
    return () => { active = false; };
  }, [server.id, latencyHours, demo, locale]);

  useEffect(() => {
    if (demo || !server.latency.length) return;
    const latest = Math.max(...server.latency.map((point) => point.timestamp).filter(Number.isFinite));
    if (!Number.isFinite(latest)) return;
    const cutoff = latest - latencyHours * 3600;
    setLatencyPoints((current) => {
      const next = new Map(current
        .filter((point) => point.timestamp >= cutoff)
        .map((point) => [`${point.task_id}:${point.timestamp}`, point]));
      for (const point of server.latency) {
        if (point.timestamp >= cutoff) {
          next.set(`${point.task_id}:${point.timestamp}`, point);
        }
      }
      return Array.from(next.values()).sort((left, right) => left.timestamp - right.timestamp);
    });
  }, [demo, latencyHours, server.latency]);

  useEffect(() => {
    if (demo || !liveLatencyResults.length || !latencyTasks.length) return;
    const definitions = new Map(latencyTasks.map((task) => [task.id, task]));
    const samples = liveLatencyResults.flatMap((result): LatencySample[] => {
      const task = definitions.get(result.task_id);
      if (!task || !Number.isFinite(result.timestamp)) return [];
      return [{
        task_id: task.id,
        server_id: server.id,
        name: task.name,
        task_type: task.task_type,
        target: task.target,
        port: task.port,
        timestamp: result.timestamp,
        latency_ms: result.latency_ms,
        packet_loss: result.packet_loss,
      }];
    });
    if (!samples.length) return;
    const latest = Math.max(...samples.map((point) => point.timestamp));
    const cutoff = latest - latencyHours * 3600;
    setLatencyPoints((current) => {
      const next = new Map(current
        .filter((point) => point.timestamp >= cutoff)
        .map((point) => [`${point.task_id}:${point.timestamp}`, point]));
      for (const point of samples) next.set(`${point.task_id}:${point.timestamp}`, point);
      return Array.from(next.values()).sort((left, right) => left.timestamp - right.timestamp);
    });
  }, [demo, latencyHours, latencyTasks, liveLatencyResults, server.id]);

  const online = isOnline(server, threshold);
  const gpuDevices = useMemo(() => displayGpuDevices(server.gpus), [server.gpus]);
  const gpuNames = gpuDevices.map((gpu) => gpu.model).join(" · ");
  const loadRanges = [
    { value: 0, label: ui(locale, "实时", "Live") },
    { value: 1, label: ui(locale, "1 小时", "1 hour") },
    { value: 4, label: ui(locale, "4 小时", "4 hours") },
    { value: 24, label: ui(locale, "1 天", "1 day") },
    { value: 168, label: ui(locale, "7 天", "7 days") },
    { value: 720, label: ui(locale, "30 天", "30 days") },
  ].filter((range) => range.value <= Math.max(24, retentionDays * 24));
  const ranges = chartType === "load" ? loadRanges : loadRanges.filter((range) => range.value > 0);
  const hours = chartType === "load" ? loadHours : latencyHours;
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

  return (
    <div className="detail-page">
      <section className="detail-hero glass-panel">
        <button className="icon-btn" onClick={onClose} title={ui(locale, "返回首页", "Back")}><ArrowLeft size={19} /></button>
        <Flag region={server.region} size={24} className="detail-flag" locale={locale} />
        <div className="detail-title">
          <div><h1>{server.name}</h1><span className={`status-chip ${online ? "online" : ""}`}>{online ? ui(locale, "在线", "Online") : ui(locale, "离线", "Offline")}</span></div>
          <p><OSIcon os={server.os} size={14} />{regionDisplayName(server.region, locale)}{server.group_name ? ` · ${server.group_name}` : ""}</p>
        </div>
      </section>

      <div className="info-groups">
        <InfoGroup title={ui(locale, "硬件信息", "Hardware")} icon={<Cpu size={16} />}>
          <InfoItem icon={<Cpu size={14} />} label="CPU" value={formatCpuName(server.cpu_model, server.cpu_cores)} />
          <InfoItem icon={<Box size={14} />} label={ui(locale, "架构", "Architecture")} value={server.arch || "--"} />
          <InfoItem icon={<Cpu size={14} />} label="GPU" value={gpuNames || ui(locale, "未检测到", "Not detected")} />
          <InfoItem icon={<ServerCog size={14} />} label={ui(locale, "虚拟化", "Virtualization")} value={server.virtualization || "--"} />
        </InfoGroup>
        <InfoGroup title={ui(locale, "系统信息", "System")} icon={<ServerCog size={16} />}>
          <InfoItem icon={<ServerCog size={14} />} label={ui(locale, "操作系统", "Operating system")} value={server.os || "--"} />
          <InfoItem icon={<Radio size={14} />} label={ui(locale, "运行时间", "Uptime")} value={formatUptime(server.uptime, locale)} />
          <InfoItem icon={<Cpu size={14} />} label={ui(locale, "内核版本", "Kernel")} value={server.kernel || "--"} />
          <InfoItem icon={<Network size={14} />} label={ui(locale, "进程 / 连接", "Processes / connections")} value={`${number(server.processes)} / ${number(server.tcp_connections) + number(server.udp_connections)}`} />
        </InfoGroup>
      </div>

      <section className="chart-section">
        <div className="chart-controls">
          <div className="segmented" aria-label="图表类型">
            <button className={chartType === "load" ? "active" : ""} onClick={() => setChartType("load")}><Activity size={14} />{ui(locale, "负载", "Load")}</button>
            {pingEnabled ? <button className={chartType === "latency" ? "active" : ""} onClick={() => setChartType("latency")}><RadioTower size={14} />{ui(locale, "延迟", "Latency")}</button> : null}
          </div>
          <div className="segmented range-control" aria-label="时间范围">
            {ranges.map((range) => <button className={hours === range.value ? "active" : ""} key={range.value} onClick={() => chartType === "load" ? setLoadHours(range.value) : setLatencyHours(range.value)}>{range.label}</button>)}
          </div>
        </div>

        {chartType === "load" && loadError ? <div className="error-band">{loadError}</div> : null}
        {chartType === "latency" && latencyError ? <div className="error-band">{latencyError}</div> : null}
        {chartType === "load" && loadLoading ? <div className="chart-loading">{ui(locale, "正在读取历史数据", "Loading history")}</div> : chartType === "latency" && latencyLoading ? <div className="chart-loading">{ui(locale, "正在读取延迟数据", "Loading latency")}</div> : chartType === "load" && !data.length ? <div className="chart-loading">{ui(locale, "暂无历史数据", "No history")}</div> : chartType === "load" ? (
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
              {latencyData.length ? <ResponsiveContainer width="100%" height="100%">
                <LineChart data={latencyData.length ? latencyData : data} margin={{ top: 10, right: 12, left: 4, bottom: 2 }}>
                  <CartesianGrid stroke="var(--chart-grid)" strokeDasharray="3 3" />
                  <XAxis dataKey="time" tick={{ fontSize: 10 }} minTickGap={40} />
                  <YAxis tick={{ fontSize: 10 }} width={48} unit="ms" />
                  <Tooltip contentStyle={tooltipStyle} formatter={(value, name) => [`${Number(value).toFixed(1)} ms`, name]} />
                  <Legend iconType="rect" iconSize={9} wrapperStyle={{ fontSize: 11, paddingTop: 10 }} />
                  {latencySeries.map((series) => <Line key={series.id} type="monotone" dataKey={series.dataKey} name={series.name} stroke={series.color} strokeWidth={2} dot={false} connectNulls={false} isAnimationActive={false} />)}
                </LineChart>
              </ResponsiveContainer> : <div className="latency-empty">{ui(locale, "暂无延迟数据", "No latency data")}</div>}
            </div>
          </section>
        )}
      </section>

    </div>
  );
}
