# Mailvern

A fast, simple IMAP mail client for macOS.

- Rust core (`crates/mail-core`): IMAP sync, IDLE push, SQLite cache with full-text search, SMTP sending.
- Tauri 2 shell (`src-tauri`) with a Svelte UI (`src`).
- Passwords are stored in the macOS Keychain (service "Mailvern"), never in the database.

## Run

```bash
npm install
npm run tauri dev
```

Running `npm run dev` on its own serves the UI in a browser with sample data, which is useful for UI work.

## Build

```bash
npm run tauri build
```

The app ends up in `target/release/bundle/macos/Mailvern.app`.

## Your data

- Accounts, signatures and the mail cache live in `~/Library/Application Support/app.mailvern/mailvern.db`. Passwords are in the Keychain. Replacing the app never touches either.
- The database is versioned with `PRAGMA user_version`. On launch, any pending upgrades run in order, each in its own transaction. Before upgrading, a backup is written next to the database as `mailvern.v<old>.bak`.
- A database from a newer Mailvern is refused, not modified.
- Never change the bundle identifier `app.mailvern`. It decides where the data lives.

### Changing the schema

Append a new entry to `MIGRATIONS` in `crates/mail-core/src/db.rs`, for example `ALTER TABLE ... ADD COLUMN ...`. Never edit or reorder an entry that has shipped. Run `cargo test -p mail-core`.

## How it stays fast

- The UI reads only from local SQLite. It never waits on the network to show a list.
- One worker per account owns the IMAP connection. A second connection sits in IDLE on the inbox.
- The newest 500 headers per folder sync first. Recent bodies download in the background.
- Read, flag, archive and delete update locally first, then go to the server.

## Sidebar

Drag an account by its name, or a folder, to reorder. Custom folders below the last standard folder sit behind "More"; drag one higher to keep it visible. Right-click an account, or use its pencil icon, to edit it.

## Keyboard

Defaults below. Rebind anything in Settings › Shortcuts.

| Key | Action |
| --- | --- |
| `j` / `k`, arrows | Next / previous |
| `r`, `a`, `f` | Reply, reply all, forward |
| `e` | Archive |
| `⌫` / `#` | Delete |
| `u` | Mark unread |
| `s` | Flag |
| `c`, `⌘N` | New message |
| `/`, `⌘F` | Search |
| `⌘R` | Check for mail |
| `⌘↩` | Send |
| `⌘,` | Accounts |
