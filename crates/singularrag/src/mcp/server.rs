//! The rmcp handler: six tools, server info, and the session key from clientInfo.
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
use singularrag_core::engine::{
    AnnotateRequest, ChangedRequest, EntitiesRequest, FindRequest, MapRequest, TraceRequest,
    ENTITIES_LIMIT_DEFAULT,
};

use crate::actor::EngineHandle;

pub const INSTRUCTIONS: &str = "singularrag gives you a ranked map of this workspace: code symbols and document sections (specs, notes, READMEs, config keys) together. Call repo_map first with your task as the query and answer from it; read only to confirm a detail the map does not show, and read the section the map points at rather than the whole file. Use find_symbol to locate a name or a heading, trace_path to see how two symbols connect (a note that mentions a symbol counts), and changed to see what a diff touches and who references it. Use entities to learn what the corpus says about a person, system or concept and how it connects, and for the ordered steps of a process; prose queries to repo_map work in plain language. When you learn something about a file that its signatures do not say, record it with annotate so the next session starts from it. Only annotate writes, and only a note into .singularrag/map.toml. A STALE header means files changed since indexing; the index catches up in the background. Entity descriptions are extracted text, not verified facts.";

// Only read by the unit test below, which asserts the router's reported description
// equals these constants (see the note on the `#[tool_router]` impl block); the macro
// itself needs a string literal, not a path to these, so they're otherwise unused outside
// `#[cfg(test)]`.
#[allow(dead_code)]
pub const REPO_MAP_DESCRIPTION: &str = "Token-budgeted map of the code symbols and document sections most relevant to a task, each file with the files that reference it. Call this first and answer locate, trace, blast-radius and placement questions from it; read a file only to confirm a detail the map does not show. `query` is a question or identifiers; `focus_files` are workspace-relative paths you already know matter; `budget_tokens` defaults to 1024, up to 8192 for trace and blast-radius questions. Each file header ends with `← ` and the files that reference it; rows are `line  signature` for code and `line  ## heading` for document sections, never bodies. The first line says how fresh the index is; if it says STALE, call again after a moment. `entities` and `themes` are optional: entity names you already know matter; themes as short phrases.";

#[allow(dead_code)]
pub const FIND_SYMBOL_DESCRIPTION: &str = "Look up a symbol by name: exact, prefix, or split words (`create session` finds `createSession`). Returns the definition's path, line and signature and which files reference it. Optional `kind` filter: function, class, method, type, const, module, section, document, element, rule, key. `limit` defaults to 10, max 50.";

#[allow(dead_code)]
pub const ANNOTATE_DESCRIPTION: &str = "Record what you learned about a file or symbol that its signatures do not say: what it is for, an entry point, a trap, a convention. One or two sentences; the next session and the developer will see it in the map. `path` is workspace-relative; `symbol` narrows the note to one definition in that file. Empty `text` removes your note. You can replace your own note on a target; a note the developer wrote is theirs.";

#[allow(dead_code)]
pub const TRACE_PATH_DESCRIPTION: &str = "How two symbols connect: the shortest chain of references between `from` and `to`, each `path::name`, up to 6 hops, with the symbol each hop goes through. Use it for trace questions before reading files.";

#[allow(dead_code)]
pub const ENTITIES_DESCRIPTION: &str = "What the corpus knows about a person, system, concept, event or process, and how it connects: matched entities with type and description, their relations, and the document sections that state them (path::heading, lines). A process comes back as ordered steps, each with its role, systems, documenting section and implementing code. `query` is a name or a question; `entities` are names you already know; `limit` defaults to 10, max 25. Descriptions are extracted text, not verified facts. Use it before reading a document about someone or something, and to learn how a process works.";

#[allow(dead_code)]
pub const CHANGED_DESCRIPTION: &str = "What a change touches: the symbols whose lines a diff modifies and the files that reference each. `base` is a git ref; omitted means the working tree against HEAD. Use it before editing to see the blast radius and after editing to check it.";

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
    /// Entity names you already know matter; they seed the sections that mention them.
    pub entities: Option<Vec<String>>,
    /// Themes as short phrases; they seed the sections stating the nearest relations.
    pub themes: Option<Vec<String>>,
}

impl From<MapArgs> for MapRequest {
    fn from(a: MapArgs) -> Self {
        let d = MapRequest::default();
        MapRequest {
            query: a.query,
            focus_files: a.focus_files.unwrap_or_default(),
            budget_tokens: a.budget_tokens.map_or(d.budget_tokens, |b| b as usize),
            entities: a.entities.unwrap_or_default(),
            themes: a.themes.unwrap_or_default(),
        }
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindArgs {
    /// Symbol name: exact, prefix, or split words.
    pub name: String,
    /// One of: function, class, method, type, const, module, section, document, element, rule, key.
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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AnnotateArgs {
    /// Repo-relative path of an indexed file.
    pub path: String,
    /// A symbol defined in that file; omit for a note on the file.
    pub symbol: Option<String>,
    /// One or two sentences; empty removes your note on the target.
    pub text: String,
}

impl From<AnnotateArgs> for AnnotateRequest {
    fn from(a: AnnotateArgs) -> Self {
        AnnotateRequest {
            path: a.path,
            symbol: a.symbol.filter(|s| !s.trim().is_empty()),
            text: a.text,
        }
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TraceArgs {
    /// `path::name` of the starting symbol.
    pub from: String,
    /// `path::name` of the target symbol.
    pub to: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ChangedArgs {
    /// A git ref to diff against; omit for the working tree against HEAD.
    pub base: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EntitiesArgs {
    /// A name or a question.
    pub query: String,
    /// Entity names you already know.
    pub entities: Option<Vec<String>>,
    /// Max entities. Default 10, max 25.
    pub limit: Option<usize>,
}

impl From<EntitiesArgs> for EntitiesRequest {
    fn from(a: EntitiesArgs) -> Self {
        EntitiesRequest {
            query: a.query,
            entities: a.entities.unwrap_or_default(),
            limit: a.limit.unwrap_or(ENTITIES_LIMIT_DEFAULT),
        }
    }
}

fn split_symbol(s: &str) -> Result<(String, String), String> {
    s.split_once("::")
        .filter(|(p, n)| !p.is_empty() && !n.is_empty())
        .map(|(p, n)| (p.to_string(), n.to_string()))
        .ok_or_else(|| format!("expected path::name, got {s:?}"))
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
        description = "Token-budgeted map of the code symbols and document sections most relevant to a task, each file with the files that reference it. Call this first and answer locate, trace, blast-radius and placement questions from it; read a file only to confirm a detail the map does not show. `query` is a question or identifiers; `focus_files` are workspace-relative paths you already know matter; `budget_tokens` defaults to 1024, up to 8192 for trace and blast-radius questions. Each file header ends with `← ` and the files that reference it; rows are `line  signature` for code and `line  ## heading` for document sections, never bodies. The first line says how fresh the index is; if it says STALE, call again after a moment. `entities` and `themes` are optional: entity names you already know matter; themes as short phrases."
    )]
    async fn repo_map(
        &self,
        Parameters(args): Parameters<MapArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        text_result(self.handle.map(args.into()).await.map(|r| r.text))
    }

    #[tool(
        name = "find_symbol",
        description = "Look up a symbol by name: exact, prefix, or split words (`create session` finds `createSession`). Returns the definition's path, line and signature and which files reference it. Optional `kind` filter: function, class, method, type, const, module, section, document, element, rule, key. `limit` defaults to 10, max 50."
    )]
    async fn find_symbol(
        &self,
        Parameters(args): Parameters<FindArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        text_result(self.handle.find(args.into()).await.map(|r| r.text))
    }

    #[tool(
        name = "annotate",
        description = "Record what you learned about a file or symbol that its signatures do not say: what it is for, an entry point, a trap, a convention. One or two sentences; the next session and the developer will see it in the map. `path` is workspace-relative; `symbol` narrows the note to one definition in that file. Empty `text` removes your note. You can replace your own note on a target; a note the developer wrote is theirs."
    )]
    async fn annotate(
        &self,
        Parameters(args): Parameters<AnnotateArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        text_result(self.handle.annotate(args.into()).await.map(|r| r.text))
    }

    #[tool(
        name = "trace_path",
        description = "How two symbols connect: the shortest chain of references between `from` and `to`, each `path::name`, up to 6 hops, with the symbol each hop goes through. Use it for trace questions before reading files."
    )]
    async fn trace_path(
        &self,
        Parameters(args): Parameters<TraceArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let req = match (split_symbol(&args.from), split_symbol(&args.to)) {
            (Ok((from_path, from_symbol)), Ok((to_path, to_symbol))) => TraceRequest {
                from_path,
                from_symbol,
                to_path,
                to_symbol,
            },
            (Err(e), _) | (_, Err(e)) => return text_result(Err(e)),
        };
        text_result(self.handle.trace_path(req).await.map(|r| r.text))
    }

    #[tool(
        name = "changed",
        description = "What a change touches: the symbols whose lines a diff modifies and the files that reference each. `base` is a git ref; omitted means the working tree against HEAD. Use it before editing to see the blast radius and after editing to check it."
    )]
    async fn changed(
        &self,
        Parameters(args): Parameters<ChangedArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        text_result(
            self.handle
                .changed(ChangedRequest {
                    base: args.base.filter(|b| !b.trim().is_empty()),
                })
                .await
                .map(|r| r.text),
        )
    }

    #[tool(
        name = "entities",
        description = "What the corpus knows about a person, system, concept, event or process, and how it connects: matched entities with type and description, their relations, and the document sections that state them (path::heading, lines). A process comes back as ordered steps, each with its role, systems, documenting section and implementing code. `query` is a name or a question; `entities` are names you already know; `limit` defaults to 10, max 25. Descriptions are extracted text, not verified facts. Use it before reading a document about someone or something, and to learn how a process works."
    )]
    async fn entities(
        &self,
        Parameters(args): Parameters<EntitiesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        text_result(self.handle.entities(args.into()).await.map(|r| r.text))
    }
}

#[tool_handler(router = self.tool_router.clone())]
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
        match self.session_key.lock() {
            Ok(mut slot) => *slot = Some(client_slug(&request.client_info.name)),
            // Not fatal — the actor falls back to `unknown` — but every retrieval this
            // session records would then be unattributable, which is worth a line.
            Err(e) => tracing::warn!("session key mutex poisoned; client will log as unknown: {e}"),
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
    fn tool_list_is_exactly_the_six_spec_tools() {
        let router = SingularragServer::tool_router();
        let mut names: Vec<String> = router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "annotate",
                "changed",
                "entities",
                "find_symbol",
                "repo_map",
                "trace_path"
            ]
        );
        let tools = router.list_all();
        let map = tools.iter().find(|t| t.name == "repo_map").unwrap();
        assert_eq!(map.description.as_deref(), Some(REPO_MAP_DESCRIPTION));
        let find = tools.iter().find(|t| t.name == "find_symbol").unwrap();
        assert_eq!(find.description.as_deref(), Some(FIND_SYMBOL_DESCRIPTION));
        let ann = tools.iter().find(|t| t.name == "annotate").unwrap();
        assert_eq!(ann.description.as_deref(), Some(ANNOTATE_DESCRIPTION));
        let trace = tools.iter().find(|t| t.name == "trace_path").unwrap();
        assert_eq!(trace.description.as_deref(), Some(TRACE_PATH_DESCRIPTION));
        let changed = tools.iter().find(|t| t.name == "changed").unwrap();
        assert_eq!(changed.description.as_deref(), Some(CHANGED_DESCRIPTION));
        let ent = tools.iter().find(|t| t.name == "entities").unwrap();
        assert_eq!(ent.description.as_deref(), Some(ENTITIES_DESCRIPTION));
        let schema = serde_json::to_value(&ent.input_schema).unwrap();
        let props = schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("query")
                && props.contains_key("entities")
                && props.contains_key("limit")
        );
        assert_eq!(schema["required"], serde_json::json!(["query"]));
        let schema = serde_json::to_value(&ann.input_schema).unwrap();
        let props = schema["properties"].as_object().unwrap();
        assert!(
            props.contains_key("path")
                && props.contains_key("symbol")
                && props.contains_key("text")
        );
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r == "path") && required.iter().any(|r| r == "text"));
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
        let schema = serde_json::to_value(&trace.input_schema).unwrap();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("from") && props.contains_key("to"));
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r == "from") && required.iter().any(|r| r == "to"));
        let schema = serde_json::to_value(&changed.input_schema).unwrap();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("base"));
        assert!(
            schema
                .get("required")
                .is_none_or(|r| r.as_array().unwrap().is_empty()),
            "{schema}"
        );
    }

    #[test]
    fn requests_convert_with_defaults() {
        let m: MapRequest = MapArgs {
            query: None,
            focus_files: None,
            budget_tokens: None,
            entities: None,
            themes: None,
        }
        .into();
        assert_eq!(m.budget_tokens, singularrag_core::map::DEFAULT_BUDGET);
        assert!(m.focus_files.is_empty());
        assert!(m.entities.is_empty() && m.themes.is_empty());
        let m: MapRequest = MapArgs {
            query: Some("x".into()),
            focus_files: Some(vec!["a.ts".into()]),
            budget_tokens: Some(99_999),
            entities: Some(vec!["SessionStore".into()]),
            themes: Some(vec!["refresh first".into()]),
        }
        .into();
        assert_eq!(m.entities, vec!["SessionStore".to_string()]);
        assert_eq!(m.themes, vec!["refresh first".to_string()]);
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
        let e: EntitiesRequest = EntitiesArgs {
            query: "q".into(),
            entities: None,
            limit: None,
        }
        .into();
        assert_eq!(e.limit, ENTITIES_LIMIT_DEFAULT);
        assert!(e.entities.is_empty());
    }
}
