//! Serves the MCP tools over a made-up mailbox, to try an agent against Mailvern without
//! touching real mail:
//!
//!     cargo run -p mail-mcp --example serve
//!
//! It prints the address and token. Saving drafts fails here: there is no mail server.

#[path = "../tests/support/sample.rs"]
mod sample;

use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("mailvern-mcp-demo-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let engine = Arc::new(mail_core::Engine::open(&dir.join("mailvern.db"))?);
    sample::seed(&engine);
    let mut args = std::env::args().skip(1).peekable();
    let mut auto = false;
    if args.peek().is_some_and(|a| a == "--auto") {
        auto = true;
        args.next();
    }
    if auto {
        engine.set_agent_default_mode(mail_core::model::AgentMode::Auto)?;
    }
    let port = args.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let token = args.next().unwrap_or_else(mail_mcp::new_token);
    let server = mail_mcp::Server::start(engine, port, token.clone()).await?;
    println!("{}\nAuthorization: Bearer {token}", server.url());
    tokio::signal::ctrl_c().await?;
    drop(server);
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}
