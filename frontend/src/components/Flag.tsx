import { useMemo } from "react";
import { countryFlag } from "../format";

export function regionDisplayName(region: string | null | undefined, locale = "zh-CN") {
  const code = region?.trim().slice(0, 2).toUpperCase() ?? "";
  if (!/^[A-Z]{2}$/.test(code)) return region?.trim() || (locale === "en" ? "Unknown region" : "未知地区");
  try { return typeof Intl.DisplayNames === "function" ? new Intl.DisplayNames([locale], { type: "region" }).of(code) || code : code; } catch { return code; }
}

export function Flag({ region, size = 20, className = "", locale = "zh-CN" }: { region: string | null | undefined; size?: number; className?: string; locale?: string }) {
  const emoji = countryFlag(region || "");
  const label = useMemo(() => regionDisplayName(region, locale), [locale, region]);
  if (!emoji) return null;
  return <span className={`region-flag ${className}`} style={{ fontSize: size * 0.9 }} title={label} aria-label={label}>{emoji}</span>;
}
