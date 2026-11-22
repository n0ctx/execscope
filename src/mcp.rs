use crate::{
    engine::Engine,
    model::{BRIEF_TOKENS, DEFAULT_BRIEF_TOKENS, Detail, MAX_REQUIRED, Request, Scope, TOOLS},
};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt, model::*, service::RequestContext,
    transport::async_rw::AsyncRwTransport,
};
use serde_json::{Value, json};
use std::sync::{Arc, LazyLock};

const NAME: &str = "inspect_environment";
const DESCRIPTION: &str = "Inspect the ExecScope server process environment before commands, builds, installs, or debugging: OS, project, tools, and the agent harness linked to this process with its configured model and declared MCP/skills/agents. Describes this server process, not necessarily the caller's terminal, live model, or session. Call with no arguments for a short snapshot; scope=harness focuses on harnesses; requires checks named prerequisites. Only scope=network makes outbound diagnostics.";
static TOOL: LazyLock<Tool> = LazyLock::new(|| {
    let schema: JsonObject = serde_json::from_value(json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "scope": {"type":"string", "enum":Scope::VALUES, "default":"auto"},
            "detail": {"type":"string", "enum":Detail::VALUES, "default":"brief"},
            "cwd": {"type":"string", "description":"Local project directory; does not select another machine or shell."},
            "refresh": {"type":"boolean", "default":false},
            "requires": {"type":"array", "description":"Check named local prerequisites; does not run user commands or permit network access.",
                "items":{"type":"string", "enum":TOOLS}, "maxItems":MAX_REQUIRED, "uniqueItems":true},
            "brief_tokens": {"type":"integer", "minimum":BRIEF_TOKENS.start(), "maximum":BRIEF_TOKENS.end(), "default":DEFAULT_BRIEF_TOKENS,
                "description":"Approximate brief size limit; brief only."}
        }
    })).expect("static tool schema is an object");
    Tool::new(NAME, DESCRIPTION, schema)
        .with_annotations(ToolAnnotations::new().read_only(true).open_world(true))
});

#[derive(Clone)]
struct Server(Arc<Engine>);

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("execscope", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if request.is_some_and(|r| r.cursor.is_some()) {
            return Err(ErrorData::invalid_params("no additional tool pages", None));
        }
        Ok(ListToolsResult::with_all_items(vec![TOOL.clone()])
            .with_ttl_ms(60_000)
            .with_cache_scope(CacheScope::Private))
    }

    // Validate in the handler so invocation errors remain visible as tool results on both revisions.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name != NAME {
            return Err(ErrorData::method_not_found::<CallToolRequestMethod>());
        }
        let args = request.arguments.unwrap_or_default();
        let report = if args.values().any(Value::is_null) {
            Err("tool arguments cannot be null")
        } else if let Ok(request) = serde_json::from_value::<Request>(Value::Object(args)) {
            tokio::select! {
                biased;
                () = ctx.ct.cancelled() => Err("inspection cancelled"),
                result = self.0.inspect(&request) => result,
            }
        } else {
            Err("invalid tool arguments; check supported fields, types and values")
        };
        Ok(match report {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        }
        .into())
    }
}

pub(crate) async fn serve() -> Result<(), &'static str> {
    let transport = AsyncRwTransport::new_server(tokio::io::stdin(), tokio::io::stdout())
        .with_max_line_length(32 * 1024);
    let service = Server(Arc::new(Engine::new(true)))
        .serve(transport)
        .await
        .map_err(|_| "MCP stdio initialization failed")?;
    service
        .waiting()
        .await
        .map_err(|_| "MCP stdio service stopped unexpectedly")?;
    Ok(())
}
