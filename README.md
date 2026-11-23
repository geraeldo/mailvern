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
- Scrolling past the end of the local cache fetches the next 500 older headers from the server.
- Read, flag, archive and delete update locally first, then go to the server.
- Search shows local results at once, then asks each server (IMAP SEARCH) and merges in mail that was never downloaded. Those hits are stored as "sparse" rows outside the synced window.

## Writing and sending

- The composer saves a draft to the account's Drafts folder a moment after each change, and when you close it. Double-click a draft, or use Edit, to keep writing.
- Sending puts the message in a local outbox. It goes out after the undo window (Settings › General, default 10 seconds) or at the time picked with Send Later (the arrow next to Send). Scheduled and failed messages show under All Accounts › Scheduled, where they can be sent now, edited or deleted. The outbox survives restarts.

## Conversations and snooze

- Messages and their replies are grouped into one row by their References / In-Reply-To headers, across folders of the same account. Turn this off in Settings › General.
- Snooze moves a conversation to a "Snoozed" folder on the server and brings it back to the Inbox as unread at the chosen time, so other devices don't show it meanwhile. The app has to be running at that time to move it back.

## Sidebar

Drag an account by its name, or a folder, to reorder. Custom folders below the last standard folder sit behind "More"; drag one higher to keep it visible. Right-click an account for Account Settings; right-click a message for reply, flag, archive and delete.

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
| `v` | Move to folder |
| `b` | Snooze (Unsnooze in Snoozed) |
| `⌘`-click, `⇧`-click, `⇧J` / `⇧K`, `⌘A` | Select several |
| `c`, `⌘N` | New message |
| `/`, `⌘F` | Search |
| `⌘R` | Check for mail |
| `⌘↩` | Send |
| `⌘,` | Accounts |
