import { setTheme as setAppTheme } from "@tauri-apps/api/app";
import { api } from "./api";

/** "system" follows macOS; the others pin the app to one appearance. */
export type ThemeMode = "system" | "light" | "dark";

export const THEME_MODES: { value: ThemeMode; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

export const theme = $state({ mode: "system" as ThemeMode });

/**
 * Sets the native window appearance, which the web view's `prefers-color-scheme` follows,
 * so every existing dark-mode style switches with it.
 */
async function apply(mode: ThemeMode) {
  try {
    await setAppTheme(mode === "system" ? null : mode);
  } catch {}
}

export async function loadTheme() {
  const v = await api.getSetting("theme");
  theme.mode = v === "light" || v === "dark" ? v : "system";
  await apply(theme.mode);
}

export async function setThemeMode(mode: ThemeMode) {
  theme.mode = mode;
  await apply(mode);
  await api.setSetting("theme", mode);
}
