import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { LatencySample, LatencyTestPoint, Server } from "../types";

export interface LatencyBar {
  key: string;
  tone: "good" | "fair" | "warning" | "poor" | "danger" | "empty";
  tooltip: string;
}

const cache = new Map<string, { at: number; tasks: LatencyTestPoint[]; points: LatencySample[] }>();
const BAR_COUNT = 20;

function latencyTone(value: number): LatencyBar["tone"] {
  if (value <= 60) return "good";
  if (value <= 100) return "fair";
  if (value <= 160) return "warning";
  if (value <= 200) return "poor";
  return "danger";
}

function lossTone(value: number): LatencyBar["tone"] {
  if (value <= 1) return "good";
  if (value <= 3) return "fair";
  if (value <= 6) return "warning";
  if (value <= 9) return "poor";
  return "danger";
}

function emptyBars(label: string): LatencyBar[] {
  return Array.from({ length: BAR_COUNT }, (_, index) => ({ key: `empty-${index}`, tone: "empty", tooltip: label }));
}

function summarize(points: LatencySample[], field: "latency_ms" | "packet_loss") {
  const valid = points.filter((point) => Number.isFinite(point[field]) && point[field] >= 0);
  if (!valid.length) return { average: -1, bars: emptyBars("无采样数据") };
  const buckets = Array.from({ length: BAR_COUNT }, () => [] as LatencySample[]);
  valid.forEach((point, index) => buckets[Math.min(buckets.length - 1, Math.floor(index * buckets.length / valid.length))].push(point));
  const tone = field === "latency_ms" ? latencyTone : lossTone;
  const bars = buckets.map((bucket, index) => {
    if (!bucket.length) return { key: `${field}-empty-${index}`, tone: "empty" as const, tooltip: "无采样数据" };
    const value = bucket.reduce((sum, point) => sum + point[field], 0) / bucket.length;
    const timestamp = bucket[bucket.length - 1].timestamp;
    return {
      key: `${field}-${timestamp}-${index}`,
      tone: tone(value),
      tooltip: `${new Date(timestamp * 1000).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })}\n${field === "latency_ms" ? `${Math.round(value)} ms` : `${value.toFixed(1)}%`}`,
    };
  });
  return { average: valid.reduce((sum, point) => sum + point[field], 0) / valid.length, bars };
}

function tasksFromSamples(points: LatencySample[]): LatencyTestPoint[] {
  return Array.from(new Map(points.map((point) => [point.task_id, {
    id: point.task_id,
    name: point.name,
    task_type: point.task_type,
    target: point.target,
    interval_seconds: 60,
  }])).values());
}

export function useNodeLatency(server: Server, enabled: boolean) {
  const initial = server.latency ?? [];
  const [tasks, setTasks] = useState<LatencyTestPoint[]>(() => tasksFromSamples(initial));
  const [points, setPoints] = useState<LatencySample[]>(initial);
  const [loading, setLoading] = useState(enabled);

  useEffect(() => {
    if (!server.latency?.length) return;
    setTasks(tasksFromSamples(server.latency));
    setPoints(server.latency);
  }, [server.latency]);

  useEffect(() => {
    if (!enabled) { setLoading(false); return; }
    let active = true;
    const hit = cache.get(server.id);
    if (hit && Date.now() - hit.at < 60_000) {
      setTasks(hit.tasks);
      setPoints(hit.points);
      setLoading(false);
      return;
    }
    setLoading(true);
    api.latencyHistory(server.id, 1).then((result) => {
      cache.set(server.id, { at: Date.now(), tasks: result.tasks, points: result.points });
      if (active) { setTasks(result.tasks); setPoints(result.points); }
    }).catch(() => {}).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [enabled, server.id]);

  return useMemo(() => {
    const latency = summarize(points, "latency_ms");
    const loss = summarize(points, "packet_loss");
    return {
      configured: tasks.length > 0,
      latencyDisplay: latency.average >= 0 ? `${Math.round(latency.average)} ms` : loading ? "加载中" : "-",
      lossDisplay: loss.average >= 0 ? `${loss.average.toFixed(1)}%` : loading ? "加载中" : "-",
      latencyBars: latency.bars,
      lossBars: loss.bars,
    };
  }, [loading, points, tasks.length]);
}
