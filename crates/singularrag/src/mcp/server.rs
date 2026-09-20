//! The rmcp handler: two tools, server info, and the session key from clientInfo.
//! All engine work goes through the actor; handlers only await a reply.

use std::sync::{Arc, Mutex};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, InitializeRequestParams, InitializeResult,
    ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler};
use singularrag_core::engine::{FindRequest, MapRequest};

use super::actor::EngineHandle;

pub const INSTRUCTIONS: &str = "singularrag gives you a ranked map of this repository. Call repo_map first with your task as the query, then read only the files it points at. Use find_symbol to locate a name. Both tools are read-only. A STALE header means files changed since indexing; the index catches up in the background.";

pub const REPO_MAP_DESCRIPTION: &str = "Token-budgeted map of the symbols most relevant to a task. Call this before reading files. `query` is a question or identifiers; `focus_files` are repo-relative paths you already know matter; `budget_tokens` defaults to 1024, max 8192. Returns paths, line numbers and signatures only, never bodies. The first line says how fresh the index is; if it says STALE, call again after a moment.";

pub const FIND_SYMBOL_DESCRIPTION: &str = "Look up a symbol by name: exact, prefix, or split words (`create session` finds `createSession`). Returns the definition's path, line and signature and which files reference it. Optional `kind` filter: function, class, method, type, const, module. `limit` defaults to 10, max 50.";

/// `clientInfo.name` → the `<client>` part of the session key. Lower-case; whitespace
/// and colons become `-` so the key stays `mcp:<client>:<pid>:<start>`.
pub fn client_slug(name: &str) -> String {
    let slug: String = name
        .trim()
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == ':')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "unknown".to_string()
    } else {
        slug
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MapArgs {
    /// A question or identifiers describing the task.
    pub query: Option<String>,
    /// Repo-relative paths you already know matter; they seed the ranking.
    pub focus_files: Option<Vec<String>>,
    /// Soft token budget for the map. Default 1024, max 8192.
    pub budget_tokens: Option<u32>,
}

impl From<MapArgs> for MapRequest {
    fn from(a: MapArgs) -> Self {
        let d = MapRequest::default();
        MapRequest {
            query: a.query,
            focus_files: a.focus_files.unwrap_or_default(),
            budget_tokens: a.budget_tokens.map_or(d.budget_tokens, |b| b as usize),
        }
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindArgs {
    /// Symbol name: exact, prefix, or split words.
    pub name: String,
    /// One of: function, class, method, type, const, module.
    pub kind: Option<String>,
    /// Max hits. Default 10, max 50.
    pub limit: Option<u32>,
}

impl From<FindArgs> for FindRequest {
    fn from(a: FindArgs) -> Self {
        FindRequest {
            name: a.name,
            kind: a.kind,
            limit: a.limit.map_or(10, |l| l as usize),
        }
    }
}

#[derive(Clone)]
pub struct SingularragServer {
    handle: EngineHandle,
    session_key: Arc<Mutex<Option<String>>>,
    tool_router: ToolRouter<Self>,
}

fn text_result(r: Result<String, String>) -> Result<CallToolResult, ErrorData> {
    Ok(match r {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(msg) => CallToolResult::error(vec![ContentBlock::text(msg)]),
    })
}

#[tool_router]
impl SingularragServer {
    pub fn new(handle: EngineHandle, session_key: Arc<Mutex<Option<String>>>) -> Self {
        Self {
            handle,
            session_key,
            tool_router: Self::tool_router(),
        }
    }

    // NOTE: rmcp-macros 3.4.0 parses `#[tool(description = ..)]` via darling as an
    // `Option<String>` (see `rmcp_macros::tool::ToolAttribute`), so it only accepts a
    // string literal there, not a path expression like `REPO_MAP_DESCRIPTION`. The
    // literals below are kept identical to the constants; the unit test below asserts
    // the router's reported description equals the constant, so any drift between the
    // literal and the constant fails the test.
    #[tool(
        name = "repo_map",
        description = "Token-budgeted map of the symbols most relevant to a task. Call this before reading files. `query` is a question or identifiers; `focus_files` are repo-relative paths you already know matter; `budget_tokens` defaults to 1024, max 8192. Returns paths, line numbers and signatures only, never bodies. The first line says how fresh the index is; if it says STALE, call again after a moment."
    )]
    async fn repo_map(
        &self,
        Parameters(args): Parameters<MapArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        text_result(self.handle.map(args.into()).await.map(|r| r.text))
    }

    #[tool(
        name = "find_symbol",
        description = "Look up a symbol by name: exact, prefix, or split words (`create session` finds `createSession`). Returns the definition's path, line and signature and which files reference it. Optional `kind` filter: function, class, method, type, const, module. `limit` defaults to 10, max 50."
    )]
    async fn find_symbol(
        &self,
        Parameters(args): Parameters<FindArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        text_result(self.handle.find(args.into()).await.map(|r| r.text))
    }
}

#[tool_handler]
impl ServerHandler for SingularragServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "singularrag",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS.to_string())
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        if let Ok(mut slot) = self.session_key.lock() {
            *slot = Some(client_slug(&request.client_info.name));
        }
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_slug_normalises() {
        assert_eq!(client_slug("Claude Code"), "claude-code");
        assert_eq!(client_slug("codex"), "codex");
        assert_eq!(client_slug("Some:Host  CLI"), "some-host-cli");
        assert_eq!(client_slug("   "), "unknown");
    }

    #[test]
    fn tool_list_is_exactly_the_two_spec_tools() {
        let router = SingularragServer::tool_router();
        let mut names: Vec<String> = router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["find_symbol", "repo_map"]);
        let tools = router.list_all();
        let map = tools.iter().find(|t| t.name == "repo_map").unwrap();
        assert_eq!(map.description.as_deref(), Some(REPO_MAP_DESCRIPTION));
        let find = tools.iter().find(|t| t.name == "find_symbol").unwrap();
        assert_eq!(find.description.as_deref(), Some(FIND_SYMBOL_DESCRIPTION));
        let schema = serde_json::to_value(&map.input_schema).unwrap();
        let props = schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("query")
                && props.contains_key("focus_files")
                && props.contains_key("budget_tokens")
        );
        assert!(
            schema
                .get("required")
                .is_none_or(|r| r.as_array().unwrap().is_empty()),
            "{schema}"
        );
        let schema = serde_json::to_value(&find.input_schema).unwrap();
        assert_eq!(schema["required"], serde_json::json!(["name"]));
    }

    #[test]
    fn requests_convert_with_defaults() {
        let m: MapRequest = MapArgs {
            query: None,
            focus_files: None,
            budget_tokens: None,
        }
        .into();
        assert_eq!(m.budget_tokens, singularrag_core::map::DEFAULT_BUDGET);
        assert!(m.focus_files.is_empty());
        let m: MapRequest = MapArgs {
            query: Some("x".into()),
            focus_files: Some(vec!["a.ts".into()]),
            budget_tokens: Some(99_999),
        }
        .into();
        assert_eq!(
            m.budget_tokens, 99_999,
            "clamping is the engine's job, not the server's"
        );
        let f: FindRequest = FindArgs {
            name: "n".into(),
            kind: None,
            limit: None,
        }
        .into();
        assert_eq!(f.limit, 10);
    }
}
