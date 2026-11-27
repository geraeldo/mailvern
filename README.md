# Mailvern

A fast IMAP mail client for macOS.

- **Local first.** Your mail is cached on your Mac, so lists and search never wait on the network.
- **AI search.** Ask a question about your mail and get an answer with sources, using your own API key from OpenRouter, Anthropic, OpenAI, Google Gemini and many more.
- **MCP support.** Coding agents like Claude Code, Codex and Opencode can search your mail and save drafts, with your approval.

## Features

- Unified inbox across accounts, with Unread and Flagged views
- Push mail through IMAP IDLE
- Conversations grouped across folders
- Instant local search with operators (`from:`, `is:unread`, `has:attachment`, …), plus server results
- Undo Send, Send Later and drafts saved as you type
- Snooze
- Rules that file, flag, forward and answer new mail
- Remote image and tracker blocking, and phishing warnings
- Calendar invites with RSVP
- Rephrase with AI in the composer
- Per-account notifications and Dock badge
- Light and dark themes, and shortcuts you can rebind

## Install

Download `Mailvern_<version>_universal.dmg` from [Releases](https://github.com/geraeldo/mailvern/releases), open it and drag Mailvern to Applications. It runs on Apple Silicon and Intel Macs with macOS 12 or later.

### "Mailvern can't be opened" (Open Anyway)

Mailvern isn't signed with an Apple Developer ID yet, so macOS blocks it the first time you open it:

1. Open Mailvern. When macOS says it can't verify the developer, click **Done** (not Move to Trash).
2. Open **System Settings › Privacy & Security** and scroll down to **Security**.
3. Next to "Mailvern was blocked to protect your Mac", click **Open Anyway**, then confirm with your password or Touch ID.

On macOS 14 and earlier you can instead right-click the app in Finder and choose **Open**. If macOS says Mailvern "is damaged and can't be opened", clear the download flag in Terminal:

```bash
xattr -dr com.apple.quarantine /Applications/Mailvern.app
```

### Accounts

Mailvern signs in to IMAP and SMTP with a password. It doesn't support OAuth ("Sign in with Google/Microsoft") yet, so:

- **Gmail** works with an [App Password](https://myaccount.google.com/apppasswords), which needs 2-Step Verification. Some Google Workspace admins turn these off.
- **Outlook, Hotmail and Microsoft 365** don't work. Microsoft no longer accepts passwords over IMAP.
- **iCloud, Yahoo, Fastmail, Zoho, Proton Bridge** and most other IMAP servers work, some with an app password.

## Build from source

You need Node.js and a Rust toolchain.

```bash
npm install
npm run tauri dev
```

`npm run dev` on its own serves the UI in a browser with sample data, for UI work. To build the app:

```bash
npm run tauri build
```

The app ends up in `target/release/bundle/macos/Mailvern.app`.

### Releasing

Bump the version in `src-tauri/tauri.conf.json`, `package.json` and `src-tauri/Cargo.toml`, commit and push to `main`, then:

```bash
npm run release
```

It builds a universal `.dmg` and, after you confirm, publishes it as GitHub Release `v<version>`. Add `-- --build-only` to only build it.

To try an MCP agent against a made-up mailbox instead of real mail (add `--auto` for automatic mode):

```bash
cargo run -p mail-mcp --example serve
```

## How it's built

- Rust core (`crates/mail-core`): IMAP sync, IDLE push, SQLite cache with full-text search, SMTP sending.
- Tauri 2 shell (`src-tauri`) with a Svelte UI (`src`).
- MCP server (`crates/mail-mcp`), served by the app at `http://127.0.0.1:47823/mcp` only while it's open and only to requests with its access token.

## Your data

- Accounts, signatures and the mail cache live in `~/Library/Application Support/app.mailvern/mailvern.db`. Passwords and AI API keys are in the macOS Keychain (service "Mailvern"), never in the database. Replacing the app never touches either.
- AI requests go straight from the app to the provider you picked, only when you use AI.
- The database is versioned with `PRAGMA user_version`. On launch, any pending upgrades run in order, each in its own transaction. Before upgrading, a backup is written next to the database as `mailvern.v<old>.bak`.
- A database from a newer Mailvern is refused, not modified.
- Never change the bundle identifier `app.mailvern`. It decides where the data lives.

### Changing the schema

Append a new entry to `MIGRATIONS` in `crates/mail-core/src/db.rs`, for example `ALTER TABLE ... ADD COLUMN ...`. Never edit or reorder an entry that has shipped. Run `cargo test -p mail-core`.

## License

[MIT](LICENSE)
