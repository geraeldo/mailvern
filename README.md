# Mailvern

A fast, simple IMAP mail client for macOS.

- Rust core (`crates/mail-core`): IMAP sync, IDLE push, SQLite cache with full-text search, SMTP sending.
- Tauri 2 shell (`src-tauri`) with a Svelte UI (`src`).
- MCP server (`crates/mail-mcp`) inside the app, so coding agents on this Mac can search mail and save drafts.
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
- A folder list or message list that the server cut off (a dropped connection, a refusal) is an error, never "the rest was deleted". The sync deletes local mail only on a complete answer.
- Search shows local results at once, then asks each server (IMAP SEARCH) and merges in mail that was never downloaded. Those hits are stored as "sparse" rows outside the synced window.

## Writing and sending

- The composer saves a draft to the account's Drafts folder a moment after each change, and when you close it. Double-click a draft, or use Edit, to keep writing.
- Sending puts the message in a local outbox. It goes out after the undo window (Settings › General, default 10 seconds) or at the time picked with Send Later (the arrow next to Send). Scheduled and failed messages show under All Accounts › Scheduled, where they can be sent now, edited or deleted. The outbox survives restarts.

## Conversations and snooze

- Messages and their replies are grouped into one row by their References / In-Reply-To headers, across folders of the same account. Turn this off in Settings › General.
- Snooze moves a conversation to a "Snoozed" folder on the server and brings it back to the Inbox as unread at the chosen time, so other devices don't show it meanwhile. The app has to be running at that time to move it back.

## AI

Add an OpenRouter API key in Settings › Models and pick two models: one for search, one for writing. The key is stored in the Keychain (service "Mailvern", account "openrouter-api-key"). Calls go straight from the app to OpenRouter through [pi-ai](https://www.npmjs.com/package/@mariozechner/pi-ai). Only its OpenAI-compatible provider is used, and it loads the first time AI is used.

- **Ask AI** (in search: `⌘↩`, or `↩` on a question ending in "?"): the search model works through read-only tools until it can answer. It can search with filters (sender, recipient, dates, unread, flagged, attachments, folder), also on the mail servers, count and group mail, read messages and whole conversations, and open attachments: PDF, Word, PowerPoint, Excel/CSV, forwarded emails, HTML, text and images. The answer fills the reader: a one-line answer, key facts and numbered citations (hover one to preview the email). The list shows the numbered sources. Opening a source shows the email with "‹ Answer" to go back. `Esc` goes back to the answer, then out of it. Ask follow-ups at the bottom; they keep what the model already read. The tools are in `src/lib/ai/search.svelte.ts`; attachment text comes from `crates/mail-core/src/extract.rs`.
- **Rephrase** (the sparkle in the composer): rewrites the selection, or your whole message (everything above the signature and the quoted text). When replying, it also sends the conversation as context. Replace goes through the editor's undo, so `⌘Z` brings your text back.
- "Only use providers that don't store or train on requests" (on by default) asks OpenRouter to skip providers that keep data.
- The AI can't send, move or delete mail. Mail content is treated as data, never as instructions.

## Coding agents (MCP)

Turn on Settings › Agent access to let coding agents on this Mac, such as Claude Code, Codex and Opencode, use Mailvern. It is off by default. The page gives a one-line setup command for each agent.

- The app serves MCP over HTTP at `http://127.0.0.1:47823/mcp`, only on this Mac and only while it is open. Every request needs the access token from the setup command. Reset the token in Settings to cut off every agent. Agents keep their own copy of the token, so after a reset run the setup command again; it replaces the old entry.
- Agents can list accounts and folders, search (also on the mail servers), count, read messages and conversations, read attachments or save them to a temp folder, and save drafts: new messages and replies, with files attached by path. A draft gets the account's signature and, for a reply, the quoted original, and shows up in Drafts to edit and send.
- Agents can also change any draft, including ones you wrote: the body, recipients, subject and attachments. Whatever they leave out stays, and a new body keeps the quoted original, the reply threading and your signature. A draft is found by its Message-ID in the mail itself, so this works after the app restarts. If you have the draft open in the composer, your next change there replaces the agent's.
- Agents can't send, move, delete or mark mail. Mail is untrusted text and agents can run commands, so a person reads every draft before it goes out. Hidden files (keys, `.env`) can't be attached.
- When an account is still syncing or can't sync, the folder list and search results say so, so an agent doesn't take "nothing found" for "no such mail".
- The tools are in `crates/mail-mcp/src/tools.rs`. The server shares the app's engine, so there is still one IMAP connection per account and the UI shows changes at once.

To try an agent against a made-up mailbox instead of real mail:

```bash
cargo run -p mail-mcp --example serve
```

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
