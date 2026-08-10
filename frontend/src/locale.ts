export type UiLocale = "zh-CN" | "en";

export function ui(locale: UiLocale | string | undefined, chinese: string, english: string) {
  return locale === "en" ? english : chinese;
}
