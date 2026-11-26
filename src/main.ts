import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { bootstrapApp } from "./lib/state.svelte";

async function start() {
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window && (window as any).__TAURI_INTERNALS__?.metadata)) {
    (await import("./dev-mock")).installMock();
  }
  // Hydrate from Rust's prebooted snapshot (computed in parallel with WKWebView spawn) before
  // mounting Svelte, so the very first paint renders populated folders & inbox with no empty flash.
  await Promise.race([
    bootstrapApp().catch(() => {}),
    new Promise((r) => setTimeout(r, 150)),
  ]);
  mount(App, { target: document.getElementById("app")! });
}

start();
