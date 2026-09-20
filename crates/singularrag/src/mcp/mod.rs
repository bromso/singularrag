//! `singularrag mcp`: stdio MCP server over the plan-1 Engine.

pub mod actor;
pub mod server;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::{transport::stdio, ServiceExt};

/// Serve MCP over stdin/stdout until the client closes the connection.
/// stdout is the protocol; logs go to stderr (initialised by `main`).
pub fn run(root: PathBuf, refresh_budget: Duration) -> anyhow::Result<()> {
    let session_key = Arc::new(Mutex::new(None));
    let (handle, join, mut died) = actor::spawn(actor::EngineConfig {
        root,
        session_key: Arc::clone(&session_key),
        refresh_budget,
    });
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = rt.block_on(async {
        let service = match server::SingularragServer::new(handle.clone(), session_key)
            .serve(stdio())
            .await
        {
            Ok(service) => service,
            Err(e) => {
                // A client that closes stdin before (or during) initialize surfaces here
                // as an initialize-phase error from rmcp's stdio transport, not as a clean
                // `waiting()` return. Treat that as a normal disconnect rather than a
                // failure: an agent host closing stdin without sending `initialize` is a
                // legitimate way to end the session.
                let msg = e.to_string();
                let lower = msg.to_lowercase();
                if lower.contains("eof")
                    || lower.contains("closed")
                    || lower.contains("end of file")
                {
                    tracing::debug!("mcp stdio closed before initialize: {msg}");
                    return Ok(());
                }
                return Err(anyhow::anyhow!("mcp initialize failed: {msg}"));
            }
        };
        // Spec §2: the one fatal path is the actor thread dying. Without this arm a
        // panicked actor leaves the server up and answering "engine thread is gone" to
        // every call for the rest of the session, with nothing on stderr and no exit for
        // the host to restart.
        tokio::select! {
            served = service.waiting() => {
                served.map_err(|e| anyhow::anyhow!("mcp transport error: {e}"))?;
            }
            _ = &mut died => {
                tracing::error!("engine thread exited unexpectedly");
                std::process::exit(2);
            }
        }
        Ok::<(), anyhow::Error>(())
    });
    handle.shutdown();
    if join.join().is_err() {
        tracing::error!("engine thread panicked");
        std::process::exit(2);
    }
    result
}
