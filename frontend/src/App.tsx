import { KeyRound, Megaphone, Moon, Search, Sun, UserCircle } from "lucide-react";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { api, ApiError, setToken } from "./api";
import { NodeCard } from "./components/NodeCard";
import { SiteLogo } from "./components/SiteLogo";
import { StatsBar } from "./components/StatsBar";
import { TurnstileWidget } from "./components/TurnstileWidget";
import { demoConfig, demoExchangeRates, demoServers } from "./demo";
import type { Config, ExchangeRates, LatencySample, LiveLatencyResult, Server } from "./types";
import { resolveBackground, themeToggle } from "./theme";
import { ui } from "./locale";
import { derivePassword } from "./password";

const defaultConfig: Config = { ...demoConfig, site_description: "", site_name: "" };
const demoMode = import.meta.env.DEV && new URLSearchParams(window.location.search).has("demo");
const MAX_PLAYBACK_SAMPLES_PER_SERVER = 600;
const NodeDetails = lazy(() => import("./components/NodeDetails").then((module) => ({ default: module.NodeDetails })));

interface LiveMetrics {
  timestamp: number;
  displayTimestamp: number;
  metrics: Partial<Server>;
  latencyResults?: LiveLatencyResult[];
}

interface LiveSample {
  ts: number;
  data: Partial<Server> & { latency_results?: LiveLatencyResult[] };
}

function mergeLiveResults(previous: LiveLatencyResult[] | undefined, incoming: LiveLatencyResult[]): LiveLatencyResult[] {
  const results = new Map<string, LiveLatencyResult>();
  for (const result of [...(previous ?? []), ...incoming]) {
    if (!result.task_id || !Number.isFinite(result.timestamp) || result.timestamp <= 0) continue;
    results.set(`${result.task_id}:${result.timestamp}`, result);
  }
  return Array.from(results.values())
    .sort((left, right) => left.timestamp - right.timestamp)
    .slice(-4096);
}

function mergeLiveLatency(server: Server, results: LiveLatencyResult[]): LatencySample[] {
  const latest = new Map<string, LiveLatencyResult>();
  for (const result of results) {
    const current = latest.get(result.task_id);
    if (!current || result.timestamp >= current.timestamp) latest.set(result.task_id, result);
  }
  let changed = false;
  const merged = server.latency.map((definition) => {
    const result = latest.get(definition.task_id);
    if (!result || result.timestamp <= definition.timestamp) return definition;
    changed = true;
    return {
      ...definition,
      server_id: server.id,
      timestamp: result.timestamp,
      latency_ms: result.latency_ms,
      packet_loss: result.packet_loss,
    };
  });
  return changed ? merged : server.latency;
}

function routeServerId() {
  const match = window.location.pathname.match(/^\/instance\/([^/]+)\/?$/);
  if (!match) return null;
  try { return decodeURIComponent(match[1]); } catch { return null; }
}

export default function App() {
  const [config, setConfig] = useState(defaultConfig);
  const [configReady, setConfigReady] = useState(demoMode);
  const [servers, setServers] = useState<Server[]>([]);
  const [liveMetrics, setLiveMetrics] = useState<Record<string, LiveMetrics>>({});
  const [clockNow, setClockNow] = useState(() => Date.now());
  const [exchangeRates, setExchangeRates] = useState<ExchangeRates | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [group, setGroup] = useState("__all__");
  const [selectedId, setSelectedId] = useState<string | null>(routeServerId);
  const [needsVerification, setNeedsVerification] = useState(false);
  const [verificationBusy, setVerificationBusy] = useState(false);
  const [needsLogin, setNeedsLogin] = useState(false);
  const [loginUsername, setLoginUsername] = useState("");
  const [loginPassword, setLoginPassword] = useState("");
  const [loginTurnstileToken, setLoginTurnstileToken] = useState("");
  const [loginTurnstileReset, setLoginTurnstileReset] = useState(0);
  const [loginBusy, setLoginBusy] = useState(false);
  const [appearance, setAppearance] = useState<"light" | "dark" | null>(() => {
    const stored = localStorage.getItem("nodeflare-theme");
    return stored === "light" || stored === "dark" ? stored : null;
  });
  const [systemDark, setSystemDark] = useState(() => matchMedia("(prefers-color-scheme: dark)").matches);
  const wsRef = useRef<WebSocket[]>([]);
  const liveConnectedRef = useRef(false);
  const playbackRef = useRef<Map<string, LiveSample[]>>(new Map());
  const serversRef = useRef<Server[]>([]);
  const dark = appearance ? appearance === "dark" : config.default_theme === "system" ? systemDark : config.default_theme === "dark";
  const background = resolveBackground(config.background_url, dark);
  const blur = themeToggle(config, "enableBlur");
  const liveServers = useMemo(() => servers.map((server) => {
    const live = liveMetrics[server.id];
    const merged = !live ? server : (() => {
      const latency = live.latencyResults?.length ? mergeLiveLatency(server, live.latencyResults) : server.latency;
      if (live.timestamp <= (server.timestamp ?? 0)) {
        return latency === server.latency ? server : { ...server, latency };
      }
      return { ...server, ...live.metrics, latency, timestamp: live.timestamp };
    })();
    // Keep the card clock moving between Agent samples, while preserving the
    // actual sample timestamp for freshness/online checks.
    if (!merged.timestamp || !merged.uptime || !Number.isFinite(merged.uptime)) return merged;
    const elapsed = Math.max(0, Math.floor(clockNow / 1000 - merged.timestamp));
    return elapsed > 0 && elapsed <= config.offline_threshold_seconds
      ? { ...merged, uptime: merged.uptime + elapsed }
      : merged;
  }), [clockNow, config.offline_threshold_seconds, liveMetrics, servers]);

  const load = useCallback(async (quiet = false) => {
    if (!quiet) setLoading(true);
    if (demoMode) {
      setConfig(demoConfig);
      setConfigReady(true);
      setServers(demoServers);
      setExchangeRates(demoExchangeRates);
      setError("");
      setLoading(false);
      return;
    }
    try {
      const result = await api.bootstrap();
      setConfig(result.config);
      setConfigReady(true);
      setServers(result.servers);
      setExchangeRates(result.exchange_rates);
      setNeedsVerification(result.access === "turnstile");
      setNeedsLogin(result.access === "login");
      if (result.access !== "ok") {
        playbackRef.current.clear();
        setLiveMetrics({});
      }
      setError("");
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 401) {
        setNeedsLogin(true);
        setNeedsVerification(false);
        setServers([]);
        playbackRef.current.clear();
        setLiveMetrics({});
        setError("");
      } else if (reason instanceof ApiError && reason.status === 403) {
        setNeedsVerification(true);
        setNeedsLogin(false);
        setServers([]);
        playbackRef.current.clear();
        setLiveMetrics({});
        setError(reason.message);
      } else {
        setError(reason instanceof Error ? reason.message : ui(config.locale, "无法加载节点状态", "Unable to load server status"));
      }
    } finally {
      setLoading(false);
    }
  }, []);

  async function loginDashboard(event: FormEvent) {
    event.preventDefault();
    if (loginBusy) return;
    setLoginBusy(true);
    setError("");
    try {
      const passwordDerived = await derivePassword(loginPassword, config.password_client_salt);
      const result = await api.login(loginUsername.trim(), loginPassword, passwordDerived, loginTurnstileToken);
      setToken(result.token);
      setLoginPassword("");
      setLoginTurnstileToken("");
      setNeedsLogin(false);
      await load();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : ui(config.locale, "登录失败", "Unable to sign in"));
      setLoginTurnstileToken("");
      setLoginTurnstileReset((value) => value + 1);
    } finally {
      setLoginBusy(false);
    }
  }

  const verifyDashboard = useCallback(async (token: string) => {
    if (!token || verificationBusy) return;
    setVerificationBusy(true);
    setError("");
    try {
      await api.verifyTurnstile(token);
      setNeedsVerification(false);
      await load();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : ui(config.locale, "Cloudflare 验证失败", "Cloudflare verification failed"));
    } finally {
      setVerificationBusy(false);
    }
  }, [load, verificationBusy]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    serversRef.current = servers;
    for (const server of servers) {
      const samples = playbackRef.current.get(server.id);
      if (!samples || !server.timestamp) continue;
      const fresh = samples.filter((sample) => sample.ts > server.timestamp!);
      if (fresh.length) playbackRef.current.set(server.id, fresh);
      else playbackRef.current.delete(server.id);
    }
  }, [servers]);

  useEffect(() => {
    if (needsLogin || needsVerification) return;
    let ticks = 0;
    const timer = window.setInterval(() => {
      ticks += 1;
      if (!liveConnectedRef.current || ticks % 20 === 0) void load(true);
    }, 15_000);
    return () => clearInterval(timer);
  }, [load, needsLogin, needsVerification]);

  useEffect(() => {
    let previous = Date.now();
    const timer = window.setInterval(() => {
      const current = Date.now();
      const elapsed = Math.max(0, Math.min(5_000, current - previous));
      previous = current;
      setClockNow(current);
      if (elapsed === 0) return;
      setLiveMetrics((currentMetrics) => {
        if (!playbackRef.current.size) return currentMetrics;
        const next = { ...currentMetrics };
        for (const [serverId, samples] of playbackRef.current) {
          const state = next[serverId];
          if (!state || !samples.length) continue;
          const displayTimestamp = state.displayTimestamp + elapsed / 1000;
          let selected: LiveSample | undefined;
          while (samples.length && samples[0].ts <= displayTimestamp) selected = samples.shift();
          if (selected) {
            const metrics = { ...selected.data };
            const latencyResults = Array.isArray(metrics.latency_results) ? metrics.latency_results : [];
            delete metrics.latency_results;
            next[serverId] = {
              timestamp: selected.ts,
              displayTimestamp,
              metrics,
              latencyResults: latencyResults.length
                ? mergeLiveResults(state.latencyResults, latencyResults)
                : state.latencyResults,
            };
          } else {
            next[serverId] = { ...state, displayTimestamp };
          }
          if (!samples.length) playbackRef.current.delete(serverId);
        }
        return next;
      });
    }, 1_000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    document.documentElement.style.colorScheme = dark ? "dark" : "light";
    document.documentElement.dataset.blur = blur ? "on" : "off";
    document.documentElement.lang = config.locale;
  }, [blur, config.locale, dark]);

  useEffect(() => {
    let link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (!link) {
      link = document.createElement("link");
      link.rel = "icon";
      document.head.append(link);
    }
    link.href = config.favicon_url || config.logo_url || "/logo.svg";
  }, [config.favicon_url, config.logo_url]);

  useEffect(() => {
    const media = matchMedia("(prefers-color-scheme: dark)");
    const update = () => setSystemDark(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    const onPop = () => setSelectedId(routeServerId());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  useEffect(() => {
    if (!configReady) return;
    const selected = liveServers.find((server) => server.id === selectedId);
    document.title = selected ? `${selected.name} · ${config.site_name}` : config.site_name;
  }, [config.site_name, configReady, liveServers, selectedId]);

  useEffect(() => {
    if (loading || needsLogin || needsVerification || demoMode) return;
    const reconnects: number[] = [];
    let cancelled = false;
    const connect = () => {
      if (cancelled) return;
      const endpoint = new URL(location.origin);
      endpoint.protocol = endpoint.protocol === "https:" ? "wss:" : "ws:";
      endpoint.pathname = "/api/ws";
      endpoint.search = "";
      if (selectedId) endpoint.searchParams.set("server_id", selectedId);
      const socket = new WebSocket(endpoint);
      wsRef.current.push(socket);
      socket.onopen = () => {
        liveConnectedRef.current = true;
      };
      socket.onclose = () => {
        wsRef.current = wsRef.current.filter((current) => current !== socket);
        liveConnectedRef.current = wsRef.current.some((current) => current.readyState === WebSocket.OPEN);
        if (!cancelled) reconnects.push(window.setTimeout(connect, 3000));
      };
      socket.onerror = () => {
        liveConnectedRef.current = false;
      };
      socket.onmessage = (event) => {
        if (event.data === "pong") return;
        try {
          const message = JSON.parse(event.data);
          if (message.type === "server" && message.server?.id) {
            setServers((current) => current.map((server) => server.id === message.server.id
              ? message.server
              : server));
            return;
          }
          if (message.type === "batchUpdate" && Array.isArray(message.updates)) {
            const replayCached = message.cached === true;
            setLiveMetrics((current) => {
              const next = { ...current };
              for (const update of message.updates as Array<{
                serverId?: string;
                samples?: LiveSample[];
                reportAgeMs?: number;
              }>) {
                if (!update.serverId || !Array.isArray(update.samples) || !update.samples.length) continue;
                const samples = update.samples
                  .filter((sample) => Number.isFinite(sample.ts) && sample.data && typeof sample.data === "object")
                  .sort((left, right) => left.ts - right.ts);
                const previous = next[update.serverId];
                const pending = playbackRef.current.get(update.serverId) ?? [];
                const persistedTimestamp = serversRef.current.find((server) => server.id === update.serverId)?.timestamp ?? 0;
                const appliedTimestamp = Math.max(previous?.timestamp ?? 0, persistedTimestamp);
                const seen = new Set<number>([
                  ...pending.map((sample) => sample.ts),
                ]);
                const incoming = samples.filter((sample) => sample.ts > appliedTimestamp && !seen.has(sample.ts));
                if (!incoming.length) continue;
                const reportAgeSeconds = replayCached && Number.isFinite(update.reportAgeMs)
                  ? Math.max(0, update.reportAgeMs! / 1000)
                  : 0;
                const cursor = replayCached
                  ? Math.max(previous?.displayTimestamp ?? 0, incoming[incoming.length - 1].ts + reportAgeSeconds)
                  : previous?.displayTimestamp ?? incoming[0].ts;
                const all = [...pending, ...incoming]
                  .sort((left, right) => left.ts - right.ts)
                  .slice(-MAX_PLAYBACK_SAMPLES_PER_SERVER);
                let selected: LiveSample | undefined;
                while (all.length && all[0].ts <= cursor) selected = all.shift();
                if (selected) {
                  const metrics = { ...selected.data };
                  const latencyResults = Array.isArray(metrics.latency_results) ? metrics.latency_results : [];
                  delete metrics.latency_results;
                  next[update.serverId] = {
                    timestamp: selected.ts,
                    displayTimestamp: cursor,
                    metrics,
                    latencyResults: latencyResults.length
                      ? mergeLiveResults(previous?.latencyResults, latencyResults)
                      : previous?.latencyResults,
                  };
                } else if (!previous) {
                  continue;
                }
                if (all.length) playbackRef.current.set(update.serverId, all);
                else playbackRef.current.delete(update.serverId);
              }
              return next;
            });
            return;
          }
        } catch { /* Ignore non-protocol messages. */ }
      };
    };
    connect();
    const heartbeat = window.setInterval(() => wsRef.current.forEach((socket) => socket.readyState === WebSocket.OPEN && socket.send("ping")), 30_000);
    return () => {
      cancelled = true;
      liveConnectedRef.current = false;
      clearInterval(heartbeat);
      reconnects.forEach(clearTimeout);
      wsRef.current.forEach((socket) => socket.close());
      wsRef.current = [];
    };
  }, [load, loading, needsLogin, needsVerification, selectedId]);

  const groups = useMemo(() => ["__all__", ...Array.from(new Set(liveServers.map((server) => server.group_name || "默认")))], [liveServers]);
  const visible = useMemo(() => liveServers.filter((server) => {
    const text = `${server.name} ${server.region} ${server.tags} ${server.group_name}`.toLowerCase();
    const groupMatches = !config.show_groups || group === "__all__" || (server.group_name || "默认") === group;
    const queryMatches = !config.show_search || text.includes(query.trim().toLowerCase());
    return groupMatches && queryMatches;
  }), [liveServers, query, group, config.show_groups, config.show_search]);
  const selected = liveServers.find((server) => server.id === selectedId) ?? null;

  function goHome() {
    window.history.pushState({}, "", demoMode ? "/?demo=1" : "/");
    setSelectedId(null);
    window.scrollTo({ top: 0, behavior: "auto" });
  }

  function openServer(server: Server) {
    const suffix = demoMode ? "?demo=1" : "";
    window.history.pushState({}, "", `/instance/${encodeURIComponent(server.id)}${suffix}`);
    setSelectedId(server.id);
    window.scrollTo({ top: 0, behavior: "auto" });
  }

  function openAdmin() {
    window.location.assign("/admin");
  }

  function toggleTheme() {
    const next = dark ? "light" : "dark";
    localStorage.setItem("nodeflare-theme", next);
    setAppearance(next);
  }

  if (!configReady) {
    return <div className="app-bootstrap" aria-busy={loading}>{error ? <div className="error-band"><span>{error}</span><button onClick={() => void load()}>重试</button></div> : null}</div>;
  }

  return (
    <div className="app-shell">
      <div className="theme-background" style={background ? { backgroundImage: `url(${JSON.stringify(background)})`, opacity: 1 } : undefined} />
      <header className="site-header">
        <div className="container header-inner">
          <div className="brand"><SiteLogo src={config.logo_url} alt="" width="36" height="36" /><strong>{config.site_name}</strong></div>
          <div className="header-actions">
            <button className="icon-btn" onClick={toggleTheme} title={dark ? ui(config.locale, "浅色主题", "Light theme") : ui(config.locale, "深色主题", "Dark theme")}>{dark ? <Sun size={18} /> : <Moon size={18} />}</button>
            <button className="icon-btn" onClick={openAdmin} title={ui(config.locale, "进入后台", "Administration")}><UserCircle size={18} /></button>
          </div>
        </div>
      </header>

      <main className="container main-content">
        {config.site_announcement ? <div className="site-announcement"><Megaphone size={16} /><span>{config.site_announcement}</span></div> : null}
        {needsLogin ? (
          <section className="dashboard-login-gate glass-panel">
            <SiteLogo src={config.logo_url} alt="" width="46" height="46" />
            <div className="dashboard-login-copy"><h1>{ui(config.locale, "登录仪表盘", "Sign in to dashboard")}</h1><p>{ui(config.locale, "此仪表盘仅限登录后访问", "This dashboard requires an administrator sign-in")}</p></div>
            <form className="dashboard-login-form" onSubmit={(event) => void loginDashboard(event)}>
              <label><span>{ui(config.locale, "用户名", "Username")}</span><input autoFocus autoComplete="username" value={loginUsername} onChange={(event) => setLoginUsername(event.target.value)} required /></label>
              <label><span>{ui(config.locale, "密码", "Password")}</span><input type="password" autoComplete="current-password" value={loginPassword} onChange={(event) => setLoginPassword(event.target.value)} required /></label>
              {config.turnstile_login_enabled ? <div className="dashboard-login-turnstile"><TurnstileWidget siteKey={config.turnstile_site_key} action="admin-login" theme={dark ? "dark" : "light"} resetKey={loginTurnstileReset} onVerify={setLoginTurnstileToken} onError={setError} /></div> : null}
              {error ? <p className="form-error">{error}</p> : null}
              <button className="primary-btn dashboard-login-submit" disabled={loginBusy || (config.turnstile_login_enabled && !loginTurnstileToken)} type="submit"><KeyRound size={16} />{loginBusy ? ui(config.locale, "登录中", "Signing in") : ui(config.locale, "登录", "Sign in")}</button>
            </form>
          </section>
        ) : needsVerification ? (
          <section className="verification-gate">
            <SiteLogo src={config.logo_url} alt="" width="52" height="52" />
            <div><h1>{ui(config.locale, "访问验证", "Access verification")}</h1><p>{config.site_name}</p></div>
            {config.turnstile_site_key ? <TurnstileWidget siteKey={config.turnstile_site_key} action="public-dashboard" theme={dark ? "dark" : "light"} onVerify={(token) => void verifyDashboard(token)} onError={setError} /> : <p className="form-error">{ui(config.locale, "Turnstile 尚未正确配置", "Turnstile is not configured")}</p>}
            {verificationBusy ? <span className="verification-status">{ui(config.locale, "正在验证", "Verifying")}</span> : null}
            {error ? <p className="form-error">{error}</p> : null}
          </section>
        ) : selectedId ? selected ? (
          <Suspense fallback={<div className="chart-loading">{ui(config.locale, "正在加载节点", "Loading server")}</div>}><NodeDetails server={selected} liveLatencyResults={liveMetrics[selected.id]?.latencyResults ?? []} threshold={config.offline_threshold_seconds} retentionDays={config.history_retention_days} locale={config.locale} demo={demoMode} onClose={goHome} /></Suspense>
        ) : loading ? <div className="chart-loading">{ui(config.locale, "正在加载节点", "Loading server")}</div> : <div className="empty-state"><strong>{ui(config.locale, "节点不存在或已隐藏", "Server not found or hidden")}</strong><button className="primary-btn" onClick={goHome}>{ui(config.locale, "返回首页", "Back")}</button></div> : <div className="home-content">
          <StatsBar servers={liveServers} config={config} exchangeRates={exchangeRates} />
          {config.show_search || config.show_groups ? <div className="toolbar">
            {config.show_search ? <div className="search-box"><Search size={16} /><input aria-label={ui(config.locale, "搜索节点", "Search servers")} placeholder={ui(config.locale, "搜索节点", "Search servers")} value={query} onChange={(event) => setQuery(event.target.value)} /></div> : null}
            {config.show_groups ? <div className="group-tabs" aria-label={ui(config.locale, "节点分组", "Server groups")}>{groups.map((item) => <button className={group === item ? "active" : ""} key={item} onClick={() => setGroup(item)}>{item === "__all__" ? ui(config.locale, "全部", "All") : item}</button>)}</div> : null}
            <span className="result-count">{ui(config.locale, `${visible.length} 个节点`, `${visible.length} servers`)}</span>
          </div> : null}
          {error ? <div className="error-band"><span>{error}</span><button onClick={() => void load()}>{ui(config.locale, "重试", "Retry")}</button></div> : null}
          {loading && !liveServers.length ? <div className="loading-grid">{Array.from({ length: 8 }).map((_, index) => <span key={index} />)}</div> : visible.length ? (
            <section className="node-grid">{visible.map((server) => <NodeCard key={server.id} server={server} config={config} onOpen={() => openServer(server)} />)}</section>
          ) : !error ? <div className="empty-state"><strong>{liveServers.length ? ui(config.locale, "没有匹配的节点", "No matching servers") : ui(config.locale, "尚未添加节点", "No servers added")}</strong></div> : null}
        </div>}
      </main>

    </div>
  );
}
