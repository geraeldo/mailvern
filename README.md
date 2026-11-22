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

## How it stays fast

- The UI reads only from local SQLite. It never waits on the network to show a list.
- One worker per account owns the IMAP connection. A second connection sits in IDLE on the inbox.
- The newest 500 headers per folder sync first. Recent bodies download in the background.
- Read, flag, archive and delete update locally first, then go to the server.

## Keyboard

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
