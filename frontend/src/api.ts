import type { AlertRule, AlertRuleInput, CloudflareUsage, Config, DatabaseStats, ExchangeRates, HistoryPoint, LatencySample, LatencyTask, LatencyTaskInput, LatencyTestPoint, Server, ServerInput, Settings, ThemeSettingsSchema, ThemeStore } from "./types";

const TOKEN_KEY = "cf-monitor-admin-token";
const TURNSTILE_KEY = "cf-monitor-turnstile-verified";

export class ApiError extends Error {
  constructor(message: string, public status: number) {
    super(message);
  }
}

export function getToken() {
  return sessionStorage.getItem(TOKEN_KEY) ?? "";
}

export function setToken(token: string) {
  if (token) sessionStorage.setItem(TOKEN_KEY, token);
  else sessionStorage.removeItem(TOKEN_KEY);
}

export function getTurnstileProof() {
  return localStorage.getItem(TURNSTILE_KEY) ?? "";
}

export function setTurnstileProof(value: string) {
  if (value) localStorage.setItem(TURNSTILE_KEY, value);
  else localStorage.removeItem(TURNSTILE_KEY);
}

async function request<T>(path: string, init: RequestInit = {}, admin = false, baseUrl = ""): Promise<T> {
  const headers = new Headers(init.headers);
  if (init.body) headers.set("Content-Type", "application/json");
  if (!baseUrl && getToken()) headers.set("Authorization", `Bearer ${getToken()}`);
  if (!baseUrl && getTurnstileProof()) headers.set("X-Turnstile-Verified", getTurnstileProof());
  const response = await fetch(`${baseUrl.replace(/\/$/, "")}${path}`, { ...init, headers });
  if (!response.ok) {
    const payload = await response.json().catch(() => ({ error: response.statusText }));
    if (response.status === 401 && admin) setToken("");
    throw new ApiError(payload.error ?? "请求失败", response.status);
  }
  return response.json() as Promise<T>;
}

export const api = {
  config: (baseUrl = "") => request<Config>("/api/config", {}, false, baseUrl),
  exchangeRates: (baseUrl = "") => request<ExchangeRates>("/api/exchange-rates", {}, false, baseUrl),
  refreshExchangeRates: () => request<ExchangeRates>("/api/admin/exchange-rates/refresh", { method: "POST" }, true),
  servers: (admin = false, baseUrl = "") =>
    request<{ servers: Server[]; server_time?: number }>(admin ? "/api/admin/servers" : "/api/servers", {}, admin, baseUrl),
  server: (id: string, baseUrl = "") => request<Server>(`/api/servers/${encodeURIComponent(id)}`, {}, false, baseUrl),
  history: (id: string, hours: number, baseUrl = "") =>
    request<{ points: HistoryPoint[] }>(`/api/history/${encodeURIComponent(id)}?hours=${hours}`, {}, false, baseUrl),
  latencyHistory: (id: string, hours: number, baseUrl = "") =>
    request<{ tasks: LatencyTestPoint[]; points: LatencySample[] }>(`/api/latency/${encodeURIComponent(id)}?hours=${hours}`, {}, false, baseUrl),
  verifyTurnstile: (token: string) =>
    request<{ verification: string }>("/api/turnstile/verify", { method: "POST", body: JSON.stringify({ token }) }),
  login: (username: string, password: string, turnstileToken: string) =>
    request<{ token: string }>("/api/admin/login", {
      method: "POST",
      body: JSON.stringify({ username, password, turnstile_token: turnstileToken }),
    }),
  settings: () => request<Settings>("/api/admin/settings", {}, true),
  latencyTasks: () => request<{ tasks: LatencyTask[] }>("/api/admin/latency-tasks", {}, true),
  createLatencyTask: (input: LatencyTaskInput) =>
    request<{ id: string }>("/api/admin/latency-tasks", { method: "POST", body: JSON.stringify(input) }, true),
  updateLatencyTask: (id: string, input: LatencyTaskInput) =>
    request<{ success: boolean }>(`/api/admin/latency-tasks/${encodeURIComponent(id)}`, { method: "PATCH", body: JSON.stringify(input) }, true),
  deleteLatencyTask: (id: string) =>
    request<{ success: boolean }>(`/api/admin/latency-tasks/${encodeURIComponent(id)}`, { method: "DELETE" }, true),
  alertRules: () => request<{ rules: AlertRule[] }>("/api/admin/alert-rules", {}, true),
  createAlertRule: (input: AlertRuleInput) =>
    request<{ id: string }>("/api/admin/alert-rules", { method: "POST", body: JSON.stringify(input) }, true),
  updateAlertRule: (id: string, input: AlertRuleInput) =>
    request<{ success: boolean }>(`/api/admin/alert-rules/${encodeURIComponent(id)}`, { method: "PATCH", body: JSON.stringify(input) }, true),
  deleteAlertRule: (id: string) =>
    request<{ success: boolean }>(`/api/admin/alert-rules/${encodeURIComponent(id)}`, { method: "DELETE" }, true),
  themes: () => request<ThemeStore>("/api/themes"),
  themeSettings: () => request<ThemeSettingsSchema>("/api/admin/theme-settings", {}, true),
  previewTheme: (themeUrl: string) =>
    request<{ theme_url: string; proof: string }>("/api/admin/themes/preview", {
      method: "POST",
      body: JSON.stringify({ theme_url: themeUrl }),
    }, true),
  saveSettings: (input: Partial<Settings>) =>
    request<{ settings: Settings; token: string | null }>("/api/admin/settings", { method: "PATCH", body: JSON.stringify(input) }, true),
  createServer: (input: ServerInput) =>
    request<{ id: string; agent_token: string }>(
      "/api/admin/servers",
      { method: "POST", body: JSON.stringify(input) },
      true,
    ),
  updateServer: (id: string, input: ServerInput) =>
    request<{ success: boolean }>(
      `/api/admin/servers/${encodeURIComponent(id)}`,
      { method: "PATCH", body: JSON.stringify(input) },
      true,
    ),
  deleteServer: (id: string) =>
    request<{ success: boolean }>(
      `/api/admin/servers/${encodeURIComponent(id)}`,
      { method: "DELETE" },
      true,
    ),
  deleteServers: (ids: string[]) =>
    request<{ success: boolean }>("/api/admin/servers", {
      method: "DELETE",
      body: JSON.stringify({ ids }),
    }, true),
  rotateToken: (id: string) =>
    request<{ agent_token: string }>(
      `/api/admin/servers/${encodeURIComponent(id)}/token`,
      { method: "POST" },
      true,
    ),
  reorderServers: (ids: string[]) =>
    request<{ success: boolean }>(
      "/api/admin/servers/order",
      { method: "PATCH", body: JSON.stringify({ ids }) },
      true,
    ),
  databaseStats: () => request<DatabaseStats>("/api/admin/database", {}, true),
  cloudflareUsage: () => request<CloudflareUsage>("/api/admin/cloudflare-usage", {}, true),
  clearHistory: () => request<{ success: boolean }>("/api/admin/history", { method: "DELETE" }, true),
  testNotification: () => request<{ success: boolean }>("/api/admin/notifications/test", { method: "POST" }, true),
};
