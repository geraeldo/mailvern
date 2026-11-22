const time = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const weekday = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const dayMonth = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short" });
const full = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", year: "numeric" });
const long = new Intl.DateTimeFormat(undefined, {
  weekday: "short", day: "numeric", month: "short", year: "numeric", hour: "numeric", minute: "2-digit",
});

export function shortDate(secs: number): string {
  if (!secs) return "";
  const d = new Date(secs * 1000);
  const now = new Date();
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  if (d.getTime() >= startOfToday) return time.format(d);
  if (d.getTime() >= startOfToday - 86400000) return "Yesterday";
  if (d.getTime() >= startOfToday - 6 * 86400000) return weekday.format(d);
  if (d.getFullYear() === now.getFullYear()) return dayMonth.format(d);
  return full.format(d);
}

export function longDate(secs: number): string {
  return secs ? long.format(new Date(secs * 1000)) : "";
}

export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

export function displayName(name: string | null | undefined, email: string): string {
  return name && name.trim() ? name : email;
}

export function initials(name: string, email: string): string {
  const src = (name || email.split("@")[0] || "?").replace(/[^\p{L}\p{N} ]/gu, " ").trim();
  const parts = src.split(/\s+/).filter(Boolean);
  if (parts.length >= 2) return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
  return (parts[0] || "?").slice(0, 2).toUpperCase();
}

const palette = ["#E8603C", "#D9A21B", "#4FA05A", "#2F9C95", "#2F6BFF", "#7A5AF8", "#C2417A", "#8A7A63"];

export function colorFor(key: string): string {
  let h = 0;
  for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) >>> 0;
  return palette[h % palette.length];
}

export const accountColors = palette;

export function formatAddr(a: { name: string | null; email: string }): string {
  return a.name ? `${a.name} <${a.email}>` : a.email;
}
