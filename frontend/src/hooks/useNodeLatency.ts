import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { LatencySample, Server } from "../types";

export interface LatencyBar {
  key: string;
  tone: "good" | "fair" | "warning" | "poor" | "danger" | "empty";
  tooltip: string;
}

const cache = new Map<string, { at: number; points: LatencySample[] }>();
const BAR_COUNT = 20;
const CACHE_TTL = 60_000;
const REFRESH_INTERVAL = 60_000;

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

function mergePoints(...sources: LatencySample[][]): LatencySample[] {
  const points = new Map<string, LatencySample>();
  for (const source of sources) {
    for (const point of source) {
      if (!Number.isFinite(point.timestamp) || point.timestamp <= 0) continue;
      points.set(`${point.task_id}:${point.timestamp}`, point);
    }
  }
  return Array.from(points.values()).sort((left, right) => left.timestamp - right.timestamp);
}

interface LatencyHistoryPoint {
  timestamp: number;
  latency: number | null;
  loss: number | null;
}

function validValue(value: number): number | null {
  return Number.isFinite(value) && value >= 0 ? value : null;
}

/**
 * Keep the same semantics as Komari-Glass: samples are grouped by elapsed
 * time, not array position, and each bucket represents the mean of the
 * configured test points that produced a usable result.
 */
function buildHistory(points: LatencySample[]): LatencyHistoryPoint[] {
  const samples = points
    .filter((point) => Number.isFinite(point.timestamp) && point.timestamp > 0)
    .map((point) => ({
      timestamp: point.timestamp * 1000,
      latency: validValue(point.latency_ms),
      loss: validValue(point.packet_loss),
    }))
    .filter((point) => point.latency !== null || point.loss !== null)
    .sort((left, right) => left.timestamp - right.timestamp);
  if (!samples.length) return [];

  const first = samples[0].timestamp;
  const last = samples[samples.length - 1].timestamp;
  const count = Math.min(BAR_COUNT, Math.max(samples.length, 1));
  const bucketSize = Math.max(1, (last - first) / count);
  const history: LatencyHistoryPoint[] = [];
  let cursor = 0;

  for (let index = 0; index < count; index += 1) {
    const start = first + bucketSize * index;
    const end = index === count - 1 ? last + 1 : start + bucketSize;
    let latencySum = 0;
    let latencyCount = 0;
    let lossSum = 0;
    let lossCount = 0;
    while (cursor < samples.length) {
      const sample = samples[cursor];
      if (sample.timestamp >= end) break;
      if (sample.timestamp >= start) {
        if (sample.latency !== null) {
          latencySum += sample.latency;
          latencyCount += 1;
        }
        if (sample.loss !== null) {
          lossSum += sample.loss;
          lossCount += 1;
        }
      }
      cursor += 1;
    }
    history.push({
      timestamp: Math.round(start / 1000),
      latency: latencyCount ? latencySum / latencyCount : null,
      loss: lossCount ? lossSum / lossCount : null,
    });
  }
  return history;
}

function summarize(points: LatencySample[]) {
  // Komari-Glass excludes tasks with no valid latency, then weights the
  // remaining summary by their actual sample counts.
  const byTask = new Map<string, LatencySample[]>();
  for (const point of points) {
    const samples = byTask.get(point.task_id) ?? [];
    samples.push(point);
    byTask.set(point.task_id, samples);
  }
  const includedTaskIds = new Set([...byTask.entries()].filter(([, samples]) =>
    samples.some((sample) => validValue(sample.latency_ms) !== null),
  ).map(([taskId]) => taskId));
  const included = points.filter((point) => includedTaskIds.has(point.task_id));
  const latencies = included.map((point) => validValue(point.latency_ms)).filter((value): value is number => value !== null);
  const losses = included.map((point) => validValue(point.packet_loss)).filter((value): value is number => value !== null);
  const history = buildHistory(included);
  return {
    averageLatency: latencies.length
      ? latencies.reduce((sum, value) => sum + value, 0) / latencies.length
      : -1,
    averageLoss: losses.length
      ? losses.reduce((sum, value) => sum + value, 0) / losses.length
      : -1,
    history,
  };
}

export function useNodeLatency(server: Server, enabled: boolean) {
  const [fetched, setFetched] = useState<LatencySample[]>(server.latency);
  const [loading, setLoading] = useState(enabled);

  const taskSignature = [...new Set(server.latency.map((point) => point.task_id))].sort().join(",");

  useEffect(() => {
    setFetched(server.latency);
  }, [server.id, taskSignature]);

  useEffect(() => {
    if (!enabled) { setLoading(false); return; }
    let active = true;
    const load = (force: boolean) => {
      const hit = cache.get(server.id);
      if (!force && hit && Date.now() - hit.at < CACHE_TTL) {
        setFetched(hit.points);
        setLoading(false);
        return;
      }
      if (!hit) setLoading(true);
      void api.latencyHistory(server.id, 1).then((result) => {
        cache.set(server.id, { at: Date.now(), points: result.points });
        if (active) setFetched(result.points);
      }).catch(() => {
        // Keep the last successful samples visible during transient failures.
      }).finally(() => { if (active) setLoading(false); });
    };
    load(false);
    const timer = window.setInterval(() => load(true), REFRESH_INTERVAL);
    return () => { active = false; window.clearInterval(timer); };
  }, [enabled, server.id]);

  // 保留历史柱，同时用实时推送覆盖同一任务的最新样本。
  const points = useMemo(() => mergePoints(fetched, server.latency), [server.latency, fetched]);

  return useMemo(() => {
    const summary = summarize(points);
    const latencyBars = summary.history.length
      ? summary.history.map((point, index) => ({
        key: `latency-${point.timestamp}-${index}`,
        tone: point.latency === null ? "empty" as const : latencyTone(point.latency),
        tooltip: point.latency === null
          ? `${new Date(point.timestamp * 1000).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })}\n无采样数据`
          : `${new Date(point.timestamp * 1000).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })}\n${Math.round(point.latency)} ms`,
      }))
      : emptyBars(loading ? "加载中" : "无采样数据");
    const lossBars = summary.history.length
      ? summary.history.map((point, index) => ({
        key: `loss-${point.timestamp}-${index}`,
        tone: point.loss === null ? "empty" as const : lossTone(point.loss),
        tooltip: point.loss === null
          ? `${new Date(point.timestamp * 1000).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })}\n无采样数据`
          : `${new Date(point.timestamp * 1000).toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" })}\n${point.loss.toFixed(1)}%`,
      }))
      : emptyBars(loading ? "加载中" : "无采样数据");
    return {
      latencyDisplay: summary.averageLatency >= 0 ? `${Math.round(summary.averageLatency)} ms` : loading ? "加载中" : "-",
      lossDisplay: summary.averageLoss >= 0 ? `${summary.averageLoss.toFixed(1)}%` : loading ? "加载中" : "-",
      latencyBars,
      lossBars,
    };
  }, [loading, points]);
}
