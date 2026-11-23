// Times offered for snoozing and sending later.

export type Preset = { label: string; at: Date };

function at(base: Date, days: number, hour: number): Date {
  const d = new Date(base);
  d.setDate(d.getDate() + days);
  d.setHours(hour, 0, 0, 0);
  return d;
}

/** Days from `now` until the next given weekday (0 = Sunday), never 0. */
function daysUntil(now: Date, weekday: number): number {
  return ((weekday - now.getDay() + 7) % 7) || 7;
}

export function snoozePresets(now = new Date()): Preset[] {
  const list: Preset[] = [];
  const later = new Date(now);
  later.setHours(now.getHours() + 3, 0, 0, 0);
  if (later.getDate() === now.getDate() && later.getHours() < 21) list.push({ label: "Later today", at: later });
  if (now.getHours() < 17) list.push({ label: "This evening", at: at(now, 0, 18) });
  list.push({ label: "Tomorrow", at: at(now, 1, 8) });
  const weekend = now.getDay() === 6 || now.getDay() === 0;
  if (!weekend && now.getDay() !== 5) list.push({ label: "This weekend", at: at(now, daysUntil(now, 6), 9) });
  list.push({ label: "Next week", at: at(now, daysUntil(now, 1), 8) });
  return list;
}

export function sendPresets(now = new Date()): Preset[] {
  const list: Preset[] = [];
  const hour = new Date(now.getTime() + 60 * 60 * 1000);
  list.push({ label: "In an hour", at: hour });
  list.push({ label: "Tomorrow morning", at: at(now, 1, 8) });
  list.push({ label: "Tomorrow afternoon", at: at(now, 1, 13) });
  if (now.getDay() !== 1 || now.getHours() >= 8) list.push({ label: "Monday morning", at: at(now, daysUntil(now, 1), 8) });
  return list;
}

const fmt = new Intl.DateTimeFormat(undefined, { weekday: "short", hour: "numeric", minute: "2-digit" });
const fmtDate = new Intl.DateTimeFormat(undefined, { weekday: "short", day: "numeric", month: "short", hour: "numeric", minute: "2-digit" });

/** "Tue 8:00" within a week, else with the date. */
export function whenLabel(d: Date, now = new Date()): string {
  const days = (d.getTime() - now.getTime()) / 86400000;
  return days < 6 && days > -1 ? fmt.format(d) : fmtDate.format(d);
}

/** Value for <input type="datetime-local">. */
export function toLocalInput(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
}

export const toUnix = (d: Date) => Math.floor(d.getTime() / 1000);
