# Mailvern — Agent & Contributor Guide

Mailvern is a local-first macOS email client built with Tauri v2, Rust, and Svelte 5 (runes).

## Workspace Layout

- `crates/mail-core/` — Pure Rust mail engine with zero Tauri dependency:
  - `db/` — SQLite persistence split by domain (`mod.rs`, `migrations.rs`, `folders.rs`, `messages.rs`, `search.rs`, `outbox.rs`, `rules.rs`, `agents.rs`, `tests.rs`).
  - `engine.rs`, `worker.rs`, `imap.rs`, `smtp.rs`, `parse.rs`, `model.rs` — Sync engine, IMAP/SMTP clients, MIME parsing, shared domain models.
  - `rules.rs`, `rule_runner.rs`, `agent_actions.rs`, `search.rs`, `security.rs`, `extract.rs`, `autoconfig.rs`, `ics.rs`, `icons.rs`, `text.rs`, `secrets.rs`.
- `crates/mail-mcp/` — Local MCP server built directly on `mail-core`, with HTTP integration tests (`tests/http.rs`) using `Engine::seed`.
- `src-tauri/` — Thin desktop shell:
  - `src/lib.rs` — App runtime, native menu, custom URI protocols (`mailvern-cid://`, `mailvern-icon://`), external link guard.
  - `src/commands/` — Tauri IPC commands split by domain (`mod.rs`, `accounts.rs`, `messages.rs`, `rules.rs`, `files.rs`).
  - `src/agents.rs`, `src/ai_http.rs`, `src/context_menu.rs`.
- `src/` — Svelte 5 + TypeScript frontend:
  - `src/lib/api.ts` — Typed Tauri `invoke()` wrappers and `Event` union.
  - `src/dev-mock.ts` — Browser dev mock for running `npm run dev` outside Tauri.
  - `src/lib/state.svelte.ts` — Global reactive state and UI actions.
  - `src/lib/mail-cache.svelte.ts` — Bounded LRU cache + keep-alive reader iframe pool (do not reorder panes in-place; WebKit reloads moved iframes).
  - `src/lib/html.ts`, `src/lib/phishing.ts`, `src/lib/trust.ts` — Single-pass `DOMParser` HTML sanitization, tracker stripping, and phishing checks.
  - `src/lib/ai/` — LLM providers, AI search, rephrase, and rule drafting.

## Key Conventions & Contracts

1. **Keep `mail-core` Tauri-free:** All mail logic, SQLite queries, and domain models belong in `crates/mail-core` so `mail-mcp` and tests can use them without Tauri.
2. **IPC parity:** Whenever you add, rename, or remove a `#[tauri::command]` in `src-tauri/src/commands/`, update:
   - `tauri::generate_handler![...]` in `src-tauri/src/lib.rs`
   - `src/lib/api.ts`
   - `src/dev-mock.ts`
3. **Batched startup (`bootstrap`):** Startup settings and initial UI state are loaded in a single `bootstrap` IPC call and hydrated via `hydrate*` helpers (`applyPrefs`, `hydrateAi`, `hydrateBadgeMode`, `hydrateTheme`, `hydrateShortcuts`). Add new startup settings to `STARTUP_SETTING_KEYS` in both `src-tauri/src/commands/mod.rs` and `src/lib/state.svelte.ts` instead of adding separate startup IPC calls.
4. **Database migrations:** Never edit shipped migrations (`V1`..`V12` in `crates/mail-core/src/db/migrations.rs`); append a new version and add a migration test in `crates/mail-core/src/db/tests.rs`.
5. **Security boundaries:** Untrusted email HTML must pass through `sanitize()` (composer) or `analyzeEmailHtml()` + sandboxed `iframe[srcdoc]` (reader). Keep restricted agent settings (`mcp_enabled`, `mcp_mode`, `mcp_token`) blocked from generic `set_setting`.

## Verification Commands

Run before finishing changes:

```sh
# Frontend
npm run check
npx tsc --noEmit
npm test
npm run build

# Rust
cargo test --workspace
cargo clippy --workspace --all-targets -- -W dead_code -W unused_imports -W unused_variables -W unused_mut
```
