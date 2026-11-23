import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "./api";

/** What the Dock badge counts. "new" leaves out mail you've already seen in the app, even if you mark it unread again. */
export type BadgeMode = "new" | "all" | "off";

export const BADGE_MODES: { id: BadgeMode; label: string; detail: string }[] = [
  { id: "new", label: "New messages", detail: "Unread mail since you last opened Mailvern" },
  { id: "all", label: "All unread messages", detail: "Every unread message in your inboxes" },
  { id: "off", label: "Off", detail: "" },
];

export const badge = $state({ mode: "new" as BadgeMode });

export async function loadBadgeMode() {
  const v = await api.getSetting("badge_mode");
  if (v === "all" || v === "off" || v === "new") badge.mode = v;
}

export async function setBadgeMode(mode: BadgeMode) {
  badge.mode = mode;
  await api.setSetting("badge_mode", mode);
  await refreshBadge();
}

/** Updates the Dock badge. While the window has focus you're looking at the mail, so nothing in it is "new". */
export async function refreshBadge() {
  try {
    const win = getCurrentWindow();
    if (await win.isFocused()) await api.markBadgeSeen();
    const n = await api.badgeCount();
    await win.setBadgeCount(n > 0 ? n : undefined);
  } catch {}
}
