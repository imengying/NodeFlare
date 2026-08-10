import type { Server, TrafficLimitType } from "./types";
import { ui, type UiLocale } from "./locale";

export function number(value: number | null | undefined) {
  return Number.isFinite(value) ? Number(value) : 0;
}

export function percent(used: number | null, total: number | null) {
  const safeTotal = number(total);
  return safeTotal > 0 ? Math.min(100, Math.max(0, (number(used) / safeTotal) * 100)) : 0;
}

export function formatBytes(value: number | null | undefined, decimals = 1) {
  const size = Math.max(0, number(value));
  if (size === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const index = Math.min(units.length - 1, Math.floor(Math.log(size) / Math.log(1024)));
  return `${(size / 1024 ** index).toFixed(index === 0 ? 0 : decimals)} ${units[index]}`;
}

export function formatSpeed(value: number | null | undefined) {
  return `${formatBytes(value)}/s`;
}

export function formatUptime(seconds: number | null | undefined, locale: UiLocale = "zh-CN") {
  const days = Math.floor(number(seconds) / 86400);
  if (days > 0) return ui(locale, `${days} 天`, `${days} days`);
  const hours = Math.floor(number(seconds) / 3600);
  if (hours > 0) return ui(locale, `${hours} 小时`, `${hours} hours`);
  const minutes = Math.floor(number(seconds) / 60);
  return ui(locale, `${minutes} 分钟`, `${minutes} minutes`);
}

export function isOnline(server: Server, threshold: number, at = Date.now() / 1000) {
  return !!server.timestamp && at - server.timestamp <= threshold;
}

export function timeAgo(timestamp: number | null, locale: UiLocale = "zh-CN") {
  if (!timestamp) return ui(locale, "尚未上报", "never reported");
  const seconds = Math.max(0, Math.floor(Date.now() / 1000 - timestamp));
  if (seconds < 60) return ui(locale, `${seconds} 秒前`, `${seconds}s ago`);
  if (seconds < 3600) return ui(locale, `${Math.floor(seconds / 60)} 分钟前`, `${Math.floor(seconds / 60)}m ago`);
  if (seconds < 86400) return ui(locale, `${Math.floor(seconds / 3600)} 小时前`, `${Math.floor(seconds / 3600)}h ago`);
  return ui(locale, `${Math.floor(seconds / 86400)} 天前`, `${Math.floor(seconds / 86400)}d ago`);
}

export function countryFlag(region: string) {
  const code = region.trim().slice(0, 2).toUpperCase();
  if (!/^[A-Z]{2}$/.test(code)) return "";
  return String.fromCodePoint(...[...code].map((char) => 127397 + char.charCodeAt(0)));
}

export function trafficUsed(server: Pick<Server, "net_rx_total" | "net_tx_total" | "traffic_limit_type">) {
  const down = number(server.net_rx_total);
  const up = number(server.net_tx_total);
  const type: TrafficLimitType = server.traffic_limit_type || "sum";
  if (type === "max") return Math.max(up, down);
  if (type === "min") return Math.min(up, down);
  if (type === "up") return up;
  if (type === "down") return down;
  return up + down;
}

const currencySymbols: Record<string, string> = {
  CNY: "¥", USD: "$", CAD: "CA$", HKD: "HK$", EUR: "€", GBP: "£", JPY: "¥",
  RUB: "₽", CHF: "CHF ", INR: "₹", VND: "₫", THB: "฿",
};

export function formatCurrency(value: number, currency = "CNY") {
  const code = currency.toUpperCase();
  const symbol = currencySymbols[code] ?? `${code} `;
  const rounded = (Math.round(number(value) * 100) / 100).toFixed(2).replace(/\.?0+$/, "");
  return `${symbol}${rounded || "0"}`;
}

export function formatPrice(server: Pick<Server, "price" | "billing_cycle" | "currency">, locale: UiLocale = "zh-CN") {
  if (server.price === -1) return ui(locale, "免费", "Free");
  if (server.price === 0) return ui(locale, "未设置", "Not set");
  if (server.price < 0) return "";
  const cycle = server.billing_cycle >= 27 && server.billing_cycle <= 32 ? ui(locale, "月", "month")
    : server.billing_cycle >= 87 && server.billing_cycle <= 95 ? ui(locale, "季", "quarter")
      : server.billing_cycle >= 175 && server.billing_cycle <= 185 ? ui(locale, "半年", "half-year")
        : server.billing_cycle >= 360 && server.billing_cycle <= 370 ? ui(locale, "年", "year")
          : server.billing_cycle === -1 ? ui(locale, "一次", "one-time") : ui(locale, `${server.billing_cycle} 天`, `${server.billing_cycle} days`);
  return `${formatCurrency(server.price, server.currency)} / ${cycle}`;
}

function daysUntil(timestamp: number | null) {
  if (!timestamp) return null;
  return Math.ceil((timestamp * 1000 - Date.now()) / 86_400_000);
}

export function formatExpire(server: Pick<Server, "expires_at" | "price">, locale: UiLocale = "zh-CN") {
  if (server.price === -1) return ui(locale, "长期", "Lifetime");
  const days = daysUntil(server.expires_at);
  if (days === null) return ui(locale, "未设置", "Not set");
  if (days < 0) return ui(locale, "已过期", "Expired");
  return ui(locale, `${days} 天`, `${days} days`);
}

export function remainingAssetValue(price: number, billingCycle: number, expiresAt: number | null) {
  if (price <= 0) return 0;
  const days = daysUntil(expiresAt);
  if (days === null || days <= 0) return 0;
  if (billingCycle <= 0) return price;
  return price * Math.min(days / billingCycle, 1);
}
