import { Megaphone, Moon, Search, Sun, UserCircle } from "lucide-react";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, ApiError } from "./api";
import { NodeCard } from "./components/NodeCard";
import { StatsBar } from "./components/StatsBar";
import { TurnstileWidget } from "./components/TurnstileWidget";
import { demoConfig, demoExchangeRates, demoServers } from "./demo";
import type { Config, ExchangeRates, Server } from "./types";
import { resolveBackground, themeToggle } from "./theme";
import { ui } from "./locale";

const defaultConfig: Config = { ...demoConfig, site_description: "", site_name: "" };
const demoMode = import.meta.env.DEV && new URLSearchParams(window.location.search).has("demo");
const NodeDetails = lazy(() => import("./components/NodeDetails").then((module) => ({ default: module.NodeDetails })));

function routeServerId() {
  const match = window.location.pathname.match(/^\/instance\/([^/]+)\/?$/);
  if (!match) return null;
  try { return decodeURIComponent(match[1]); } catch { return null; }
}

export default function App() {
  const [config, setConfig] = useState(defaultConfig);
  const [configReady, setConfigReady] = useState(demoMode);
  const [servers, setServers] = useState<Server[]>([]);
  const [exchangeRates, setExchangeRates] = useState<ExchangeRates | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [group, setGroup] = useState("__all__");
  const [selectedId, setSelectedId] = useState<string | null>(routeServerId);
  const [needsVerification, setNeedsVerification] = useState(false);
  const [verificationBusy, setVerificationBusy] = useState(false);
  const [appearance, setAppearance] = useState<"light" | "dark" | null>(() => {
    const stored = localStorage.getItem("nodeflare-theme");
    return stored === "light" || stored === "dark" ? stored : null;
  });
  const [systemDark, setSystemDark] = useState(() => matchMedia("(prefers-color-scheme: dark)").matches);
  const wsRef = useRef<WebSocket[]>([]);
  const dark = appearance ? appearance === "dark" : config.default_theme === "system" ? systemDark : config.default_theme === "dark";
  const background = resolveBackground(config.background_url, dark);
  const blur = themeToggle(config, "enableBlur");

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
      const nextConfig = await api.config();
      setConfig(nextConfig);
      setConfigReady(true);
      const [serverResult, ratesResult] = await Promise.all([api.servers(), api.exchangeRates()]);
      setServers(serverResult.servers);
      setExchangeRates(ratesResult);
      setNeedsVerification(false);
      setError("");
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 403) {
        setNeedsVerification(true);
        setServers([]);
      }
      setError(reason instanceof Error ? reason.message : ui(config.locale, "无法加载节点状态", "Unable to load server status"));
    } finally {
      setLoading(false);
    }
  }, []);

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
    const timer = window.setInterval(() => void load(true), 60_000);
    return () => clearInterval(timer);
  }, [load]);

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
    link.href = config.favicon_url || "/logo.svg";
  }, [config.favicon_url]);

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
    const selected = servers.find((server) => server.id === selectedId);
    document.title = selected ? `${selected.name} · ${config.site_name}` : config.site_name;
  }, [config.site_name, configReady, selectedId, servers]);

  useEffect(() => {
    if (loading || needsVerification || !config.public_dashboard || demoMode) return;
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
      socket.onopen = () => void load(true);
      socket.onclose = () => {
        wsRef.current = wsRef.current.filter((current) => current !== socket);
        if (!cancelled) reconnects.push(window.setTimeout(connect, 3000));
      };
      socket.onmessage = (event) => {
        if (event.data === "pong") return;
        try {
          const message = JSON.parse(event.data);
          if (message.type !== "server" || !message.server?.id) return;
          setServers((current) => current.map((server) => server.id === message.server.id
            ? message.server
            : server));
        } catch { /* Ignore non-protocol messages. */ }
      };
    };
    connect();
    const heartbeat = window.setInterval(() => wsRef.current.forEach((socket) => socket.readyState === WebSocket.OPEN && socket.send("ping")), 30_000);
    return () => {
      cancelled = true;
      clearInterval(heartbeat);
      reconnects.forEach(clearTimeout);
      wsRef.current.forEach((socket) => socket.close());
      wsRef.current = [];
    };
  }, [config.public_dashboard, load, loading, needsVerification, selectedId]);

  const groups = useMemo(() => ["__all__", ...Array.from(new Set(servers.map((server) => server.group_name || "默认")))], [servers]);
  const visible = useMemo(() => servers.filter((server) => {
    const text = `${server.name} ${server.region} ${server.tags} ${server.group_name}`.toLowerCase();
    const groupMatches = !config.show_groups || group === "__all__" || (server.group_name || "默认") === group;
    const queryMatches = !config.show_search || text.includes(query.trim().toLowerCase());
    return groupMatches && queryMatches;
  }), [servers, query, group, config.show_groups, config.show_search]);
  const selected = servers.find((server) => server.id === selectedId) ?? null;

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
          <div className="brand"><img src="/logo.svg" alt="" width="36" height="36" /><strong>{config.site_name}</strong></div>
          <div className="header-actions">
            <button className="icon-btn" onClick={toggleTheme} title={dark ? ui(config.locale, "浅色主题", "Light theme") : ui(config.locale, "深色主题", "Dark theme")}>{dark ? <Sun size={18} /> : <Moon size={18} />}</button>
            <button className="icon-btn" onClick={openAdmin} title={ui(config.locale, "进入后台", "Administration")}><UserCircle size={18} /></button>
          </div>
        </div>
      </header>

      <main className="container main-content">
        {config.site_announcement ? <div className="site-announcement"><Megaphone size={16} /><span>{config.site_announcement}</span></div> : null}
        {needsVerification ? (
          <section className="verification-gate">
            <img src="/logo.svg" alt="" width="52" height="52" />
            <div><h1>{ui(config.locale, "访问验证", "Access verification")}</h1><p>{config.site_name}</p></div>
            {config.turnstile_site_key ? <TurnstileWidget siteKey={config.turnstile_site_key} theme={dark ? "dark" : "light"} onVerify={(token) => void verifyDashboard(token)} onError={setError} /> : <p className="form-error">{ui(config.locale, "Turnstile 尚未正确配置", "Turnstile is not configured")}</p>}
            {verificationBusy ? <span className="verification-status">{ui(config.locale, "正在验证", "Verifying")}</span> : null}
            {error ? <p className="form-error">{error}</p> : null}
          </section>
        ) : selectedId ? selected ? (
          <Suspense fallback={<div className="chart-loading">{ui(config.locale, "正在加载节点", "Loading server")}</div>}><NodeDetails server={selected} threshold={config.offline_threshold_seconds} retentionDays={config.history_retention_days} locale={config.locale} demo={demoMode} onClose={goHome} /></Suspense>
        ) : loading ? <div className="chart-loading">{ui(config.locale, "正在加载节点", "Loading server")}</div> : <div className="empty-state"><strong>{ui(config.locale, "节点不存在或已隐藏", "Server not found or hidden")}</strong><button className="primary-btn" onClick={goHome}>{ui(config.locale, "返回首页", "Back")}</button></div> : <div className="home-content">
          <StatsBar servers={servers} config={config} exchangeRates={exchangeRates} />
          {config.show_search || config.show_groups ? <div className="toolbar">
            {config.show_search ? <div className="search-box"><Search size={16} /><input aria-label={ui(config.locale, "搜索节点", "Search servers")} placeholder={ui(config.locale, "搜索节点", "Search servers")} value={query} onChange={(event) => setQuery(event.target.value)} /></div> : null}
            {config.show_groups ? <div className="group-tabs" aria-label={ui(config.locale, "节点分组", "Server groups")}>{groups.map((item) => <button className={group === item ? "active" : ""} key={item} onClick={() => setGroup(item)}>{item === "__all__" ? ui(config.locale, "全部", "All") : item}</button>)}</div> : null}
            <span className="result-count">{ui(config.locale, `${visible.length} 个节点`, `${visible.length} servers`)}</span>
          </div> : null}
          {error ? <div className="error-band"><span>{error}</span><button onClick={() => void load()}>{ui(config.locale, "重试", "Retry")}</button></div> : null}
          {loading && !servers.length ? <div className="loading-grid">{Array.from({ length: 8 }).map((_, index) => <span key={index} />)}</div> : visible.length ? (
            <section className="node-grid">{visible.map((server) => <NodeCard key={server.id} server={server} config={config} onOpen={() => openServer(server)} />)}</section>
          ) : !error ? <div className="empty-state"><strong>{servers.length ? ui(config.locale, "没有匹配的节点", "No matching servers") : ui(config.locale, "尚未添加节点", "No servers added")}</strong></div> : null}
        </div>}
      </main>

    </div>
  );
}
