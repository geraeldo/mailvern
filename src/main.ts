import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";

async function start() {
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window && (window as any).__TAURI_INTERNALS__?.metadata)) {
    (await import("./dev-mock")).installMock();
  }
  mount(App, { target: document.getElementById("app")! });
}

start();
