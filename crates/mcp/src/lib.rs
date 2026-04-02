use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use why_archaeologist::analyze_target;
use why_locator::parse_target;
use why_scanner::{scan_coupling, scan_hotspots, scan_rename_safe, scan_time_bombs};
use why_splitter::suggest_split;
use why_workflows::{load_builtin_workflow, load_builtin_workflows};

const JSON_RPC_VERSION: &str = "2.0";
const LATEST_MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const SUPPORTED_MCP_PROTOCOL_VERSIONS: &[&str] =
    &[LATEST_MCP_PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];
const DEFAULT_TIME_BOMB_AGE_DAYS: i64 = 30;
const DEFAULT_HOTSPOT_LIMIT: usize = 10;

pub fn run_stdio() -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut writer = io::BufWriter::new(stdout.lock());

    for line in stdin.lock().lines() {
        let line = line.context("failed to read stdin")?;
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => handle_request(request),
            Err(error) => Some(JsonRpcResponse::error(
                None,
                ErrorCode::ParseError,
                format!("failed to parse JSON-RPC request: {error}"),
            )),
        };

        if let Some(response) = response {
            serde_json::to_writer(&mut writer, &response)
                .context("failed to serialize response")?;
            writer.write_all(b"\n").context("failed to write newline")?;
            writer.flush().context("failed to flush stdout")?;
        }
    }

    Ok(())
}

fn handle_request(request: JsonRpcRequest) -> Option<JsonRpcResponse> {
    if request.jsonrpc != JSON_RPC_VERSION {
        return Some(JsonRpcResponse::error(
            request.id,
            ErrorCode::InvalidRequest,
            format!("unsupported jsonrpc version: {}", request.jsonrpc),
        ));
    }

    if is_notification(&request) {
        return None;
    }

    let id = request.id.clone();
    let result = match request.method.as_str() {
        "initialize" => initialize_result(request.params),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list_result()),
        "tools/call" => call_tool(request.params),
        _ => Err(McpError::new(
            ErrorCode::MethodNotFound,
            format!("unknown method: {}", request.method),
        )),
    };

    match result {
        Ok(value) => Some(JsonRpcResponse::success(id, value)),
        Err(error) => Some(JsonRpcResponse::error(id, error.code, error.message)),
    }
}

fn is_notification(request: &JsonRpcRequest) -> bool {
    request.id.is_none()
        && matches!(
            request.method.as_str(),
            "notifications/initialized" | "initialized" | "notifications/cancelled"
        )
}

fn initialize_result(params: Option<Value>) -> std::result::Result<Value, McpError> {
    let params: InitializeParams = deserialize_optional_params(params, "initialize params")?;
    let protocol_version = negotiate_protocol_version(params.protocol_version.as_deref());

    Ok(json!({
        "protocolVersion": protocol_version,
        "serverInfo": {
            "name": "why",
            "version": env!("CARGO_PKG_VERSION")
        },
        "capabilities": {
            "tools": {}
        }
    }))
}

fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|version| {
            SUPPORTED_MCP_PROTOCOL_VERSIONS
                .iter()
                .copied()
                .find(|supported| *supported == version)
        })
        .unwrap_or(LATEST_MCP_PROTOCOL_VERSION)
}

fn tools_list_result() -> Value {
    json!({
        "tools": [
            tool_definition(
                "why_symbol",
                "Explain why a symbol, line, or line range exists using git archaeology.",
                json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string" },
                        "lines": { "type": "string" },
                        "no_llm": { "type": "boolean" }
                    },
                    "required": ["target"],
                    "additionalProperties": false
                })
            ),
            tool_definition(
                "why_split",
                "Suggest archaeology-guided splits for a symbol target.",
                json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string" }
                    },
                    "required": ["target"],
                    "additionalProperties": false
                })
            ),
            tool_definition(
                "why_time_bombs",
                "Scan the current repository for stale TODO/HACK/TEMP markers.",
                json!({
                    "type": "object",
                    "properties": {
                        "max_age_days": { "type": "integer" }
                    },
                    "additionalProperties": false
                })
            ),
            tool_definition(
                "why_hotspots",
                "Rank high-churn files by churn × archaeology-derived risk.",
                json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer" }
                    },
                    "additionalProperties": false
                })
            ),
            tool_definition(
                "why_coupling",
                "Rank files that frequently co-change with the queried target.",
                json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string" },
                        "lines": { "type": "string" },
                        "limit": { "type": "integer" }
                    },
                    "required": ["target"],
                    "additionalProperties": false
                })
            ),
            tool_definition(
                "why_rename_safe",
                "Assess Rust symbol rename risk by ranking the target and its caller symbols.",
                json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string" },
                        "lines": { "type": "string" },
                        "since_days": { "type": "integer" }
                    },
                    "required": ["target"],
                    "additionalProperties": false
                })
            ),
            tool_definition(
                "why_list_workflows",
                "List builtin why investigation workflows loaded from markdown on disk.",
                json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                })
            ),
            tool_definition(
                "why_get_workflow",
                "Load a named builtin why investigation workflow from markdown on disk.",
                json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" }
                    },
                    "required": ["id"],
                    "additionalProperties": false
                })
            )
        ]
    })
}

fn tool_definition(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn call_tool(params: Option<Value>) -> std::result::Result<Value, McpError> {
    let params = params.ok_or_else(|| McpError::new(ErrorCode::InvalidParams, "missing params"))?;
    let request: ToolCallRequest = serde_json::from_value(params).map_err(|error| {
        McpError::new(
            ErrorCode::InvalidParams,
            format!("invalid tool call params: {error}"),
        )
    })?;

    let cwd = std::env::current_dir().map_err(|error| {
        McpError::new(
            ErrorCode::InternalError,
            format!("failed to determine current directory: {error}"),
        )
    })?;

    let value = match request.name.as_str() {
        "why_symbol" => {
            let args: WhySymbolArgs = deserialize_arguments(request.arguments)?;
            let target = parse_target(&args.target, args.lines.as_deref())
                .map_err(|error| McpError::tool_error(error.to_string()))?;
            let _ = args.no_llm;
            serde_json::to_value(
                analyze_target(&target, &cwd)
                    .map_err(|error| McpError::tool_error(error.to_string()))?,
            )
            .map_err(|error| {
                McpError::new(
                    ErrorCode::InternalError,
                    format!("failed to serialize why_symbol result: {error}"),
                )
            })?
        }
        "why_split" => {
            let args: WhySplitArgs = deserialize_arguments(request.arguments)?;
            let target = parse_target(&args.target, None)
                .map_err(|error| McpError::tool_error(error.to_string()))?;
            serde_json::to_value(
                suggest_split(&target, &cwd)
                    .map_err(|error| McpError::tool_error(error.to_string()))?,
            )
            .map_err(|error| {
                McpError::new(
                    ErrorCode::InternalError,
                    format!("failed to serialize why_split result: {error}"),
                )
            })?
        }
        "why_time_bombs" => {
            let args: WhyTimeBombsArgs = deserialize_arguments(request.arguments)?;
            let max_age_days = args.max_age_days.unwrap_or(DEFAULT_TIME_BOMB_AGE_DAYS);
            if max_age_days <= 0 {
                return Err(McpError::new(
                    ErrorCode::InvalidParams,
                    "max_age_days must be greater than zero",
                ));
            }
            serde_json::to_value(
                scan_time_bombs(&cwd, max_age_days)
                    .map_err(|error| McpError::tool_error(error.to_string()))?,
            )
            .map_err(|error| {
                McpError::new(
                    ErrorCode::InternalError,
                    format!("failed to serialize why_time_bombs result: {error}"),
                )
            })?
        }
        "why_hotspots" => {
            let args: WhyHotspotsArgs = deserialize_arguments(request.arguments)?;
            let limit = args.limit.unwrap_or(DEFAULT_HOTSPOT_LIMIT);
            if limit == 0 {
                return Err(McpError::new(
                    ErrorCode::InvalidParams,
                    "limit must be greater than zero",
                ));
            }
            serde_json::to_value(
                scan_hotspots(&cwd, limit, None)
                    .map_err(|error| McpError::tool_error(error.to_string()))?,
            )
            .map_err(|error| {
                McpError::new(
                    ErrorCode::InternalError,
                    format!("failed to serialize why_hotspots result: {error}"),
                )
            })?
        }
        "why_coupling" => {
            let args: WhyCouplingArgs = deserialize_arguments(request.arguments)?;
            let target = parse_target(&args.target, args.lines.as_deref())
                .map_err(|error| McpError::tool_error(error.to_string()))?;
            let limit = args.limit.unwrap_or(DEFAULT_HOTSPOT_LIMIT);
            if limit == 0 {
                return Err(McpError::new(
                    ErrorCode::InvalidParams,
                    "limit must be greater than zero",
                ));
            }
            serde_json::to_value(
                scan_coupling(&cwd, &target, limit)
                    .map_err(|error| McpError::tool_error(error.to_string()))?,
            )
            .map_err(|error| {
                McpError::new(
                    ErrorCode::InternalError,
                    format!("failed to serialize why_coupling result: {error}"),
                )
            })?
        }
        "why_rename_safe" => {
            let args: WhyRenameSafeArgs = deserialize_arguments(request.arguments)?;
            let target = parse_target(&args.target, args.lines.as_deref())
                .map_err(|error| McpError::tool_error(error.to_string()))?;
            serde_json::to_value(
                scan_rename_safe(&cwd, &target, args.since_days)
                    .map_err(|error| McpError::tool_error(error.to_string()))?,
            )
            .map_err(|error| {
                McpError::new(
                    ErrorCode::InternalError,
                    format!("failed to serialize why_rename_safe result: {error}"),
                )
            })?
        }
        "why_list_workflows" => serde_json::to_value(
            load_builtin_workflows().map_err(|error| McpError::tool_error(error.to_string()))?,
        )
        .map_err(|error| {
            McpError::new(
                ErrorCode::InternalError,
                format!("failed to serialize why_list_workflows result: {error}"),
            )
        })?,
        "why_get_workflow" => {
            let args: WhyGetWorkflowArgs = deserialize_arguments(request.arguments)?;
            let workflow = load_builtin_workflow(&args.id)
                .map_err(|error| McpError::tool_error(error.to_string()))?
                .ok_or_else(|| {
                    McpError::new(
                        ErrorCode::InvalidParams,
                        format!("unknown workflow: {}", args.id),
                    )
                })?;
            serde_json::to_value(workflow).map_err(|error| {
                McpError::new(
                    ErrorCode::InternalError,
                    format!("failed to serialize why_get_workflow result: {error}"),
                )
            })?
        }
        other => {
            return Err(McpError::new(
                ErrorCode::InvalidParams,
                format!("unknown tool: {other}"),
            ));
        }
    };

    Ok(json!({ "content": [{ "type": "json", "json": value }] }))
}

fn deserialize_arguments<T: for<'de> Deserialize<'de>>(
    value: Option<Value>,
) -> std::result::Result<T, McpError> {
    serde_json::from_value(value.unwrap_or(Value::Object(Default::default()))).map_err(|error| {
        McpError::new(
            ErrorCode::InvalidParams,
            format!("invalid tool arguments: {error}"),
        )
    })
}

fn deserialize_optional_params<T>(
    value: Option<Value>,
    context: &str,
) -> std::result::Result<T, McpError>
where
    T: for<'de> Deserialize<'de> + Default,
{
    match value {
        None | Some(Value::Null) => Ok(T::default()),
        Some(value) => serde_json::from_value(value).map_err(|error| {
            McpError::new(
                ErrorCode::InvalidParams,
                format!("invalid {context}: {error}"),
            )
        }),
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    #[serde(default)]
    protocol_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct WhySymbolArgs {
    target: String,
    #[serde(default)]
    lines: Option<String>,
    #[serde(default)]
    no_llm: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct WhySplitArgs {
    target: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WhyTimeBombsArgs {
    #[serde(default)]
    max_age_days: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WhyHotspotsArgs {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WhyCouplingArgs {
    target: String,
    #[serde(default)]
    lines: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct WhyRenameSafeArgs {
    target: String,
    #[serde(default)]
    lines: Option<String>,
    #[serde(default)]
    since_days: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct WhyGetWorkflowArgs {
    id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ToolCallRequest {
    name: String,
    #[serde(default)]
    arguments: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<RequestId>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum RequestId {
    Number(i64),
    String(String),
}

#[derive(Debug, Clone, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<RequestId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    fn success(id: Option<RequestId>, result: Value) -> Self {
        Self {
            jsonrpc: JSON_RPC_VERSION,
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<RequestId>, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSON_RPC_VERSION,
            id,
            result: None,
            error: Some(JsonRpcError {
                code: code.as_i32(),
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

#[derive(Debug, Clone, Copy)]
enum ErrorCode {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    InternalError,
}

impl ErrorCode {
    fn as_i32(self) -> i32 {
        match self {
            Self::ParseError => -32700,
            Self::InvalidRequest => -32600,
            Self::MethodNotFound => -32601,
            Self::InvalidParams => -32602,
            Self::InternalError => -32603,
        }
    }
}

#[derive(Debug, Clone)]
struct McpError {
    code: ErrorCode,
    message: String,
}

impl McpError {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn tool_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InternalError, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: JSON_RPC_VERSION.to_string(),
            id: Some(RequestId::Number(1)),
            method: method.to_string(),
            params: Some(params),
        }
    }

    #[test]
    fn initialize_returns_server_metadata() {
        let response = handle_request(request(
            "initialize",
            json!({ "protocolVersion": LATEST_MCP_PROTOCOL_VERSION }),
        ))
        .expect("initialize should return a response");
        assert!(response.error.is_none());
        assert!(response.result.is_some(), "initialize should return result");
        let result = response.result.unwrap_or(Value::Null);
        assert_eq!(result["protocolVersion"], LATEST_MCP_PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], "why");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[test]
    fn initialize_negotiates_requested_supported_protocol_version() {
        let response = handle_request(request(
            "initialize",
            json!({ "protocolVersion": "2024-11-05" }),
        ))
        .expect("initialize should return a response");
        let result = response.result.unwrap_or(Value::Null);
        assert_eq!(result["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn initialize_falls_back_to_latest_supported_protocol_version() {
        let response = handle_request(request("initialize", json!({ "protocolVersion": "2.0" })))
            .expect("initialize should return a response");
        let result = response.result.unwrap_or(Value::Null);
        assert_eq!(result["protocolVersion"], LATEST_MCP_PROTOCOL_VERSION);
    }

    #[test]
    fn initialized_notification_returns_no_response() {
        let response = handle_request(JsonRpcRequest {
            jsonrpc: JSON_RPC_VERSION.to_string(),
            id: None,
            method: "notifications/initialized".to_string(),
            params: None,
        });
        assert!(
            response.is_none(),
            "initialized notification should not reply"
        );
    }

    #[test]
    fn tools_list_returns_expected_tools() {
        let response = handle_request(request("tools/list", json!({})))
            .expect("tools/list should return a response");
        assert!(response.error.is_none());
        assert!(response.result.is_some(), "tools/list should return result");
        let result = response.result.unwrap_or(Value::Null);
        assert!(result["tools"].is_array(), "tools should be array");
        let empty_tools = Vec::new();
        let tools = result["tools"].as_array().unwrap_or(&empty_tools);
        assert_eq!(tools.len(), 8);
        assert_eq!(tools[0]["name"], "why_symbol");
        assert_eq!(tools[1]["name"], "why_split");
        assert_eq!(tools[2]["name"], "why_time_bombs");
        assert_eq!(tools[3]["name"], "why_hotspots");
        assert_eq!(tools[4]["name"], "why_coupling");
        assert_eq!(tools[5]["name"], "why_rename_safe");
        assert_eq!(tools[6]["name"], "why_list_workflows");
        assert_eq!(tools[7]["name"], "why_get_workflow");
    }

    #[test]
    fn rejects_unknown_method() {
        let response =
            handle_request(request("wat", json!({}))).expect("unknown request should respond");
        assert!(response.result.is_none());
        assert!(response.error.is_some(), "unknown method should error");
        let error = response.error.unwrap_or(JsonRpcError {
            code: 0,
            message: String::new(),
        });
        assert_eq!(error.code, ErrorCode::MethodNotFound.as_i32());
        assert!(error.message.contains("unknown method"));
    }

    #[test]
    fn rejects_invalid_jsonrpc_version() {
        let response = handle_request(JsonRpcRequest {
            jsonrpc: "1.0".to_string(),
            id: Some(RequestId::Number(1)),
            method: "initialize".to_string(),
            params: Some(json!({})),
        })
        .expect("invalid jsonrpc requests should respond");
        assert!(response.result.is_none());
        assert!(response.error.is_some(), "invalid version should error");
        let error = response.error.unwrap_or(JsonRpcError {
            code: 0,
            message: String::new(),
        });
        assert_eq!(error.code, ErrorCode::InvalidRequest.as_i32());
        assert!(error.message.contains("unsupported jsonrpc version"));
    }

    #[test]
    fn rejects_unknown_tool() {
        let response = handle_request(request(
            "tools/call",
            json!({
                "name": "missing_tool",
                "arguments": {}
            }),
        ))
        .expect("tool call should return a response");
        assert!(response.result.is_none());
        assert!(response.error.is_some(), "unknown tool should error");
        let error = response.error.unwrap_or(JsonRpcError {
            code: 0,
            message: String::new(),
        });
        assert_eq!(error.code, ErrorCode::InvalidParams.as_i32());
        assert!(error.message.contains("unknown tool"));
    }

    #[test]
    fn rejects_invalid_time_bomb_args() {
        let response = handle_request(request(
            "tools/call",
            json!({
                "name": "why_time_bombs",
                "arguments": { "max_age_days": 0 }
            }),
        ))
        .expect("tool call should return a response");
        assert!(response.result.is_none());
        assert!(response.error.is_some(), "invalid args should error");
        let error = response.error.unwrap_or(JsonRpcError {
            code: 0,
            message: String::new(),
        });
        assert_eq!(error.code, ErrorCode::InvalidParams.as_i32());
        assert!(
            error
                .message
                .contains("max_age_days must be greater than zero")
        );
    }

    #[test]
    fn rejects_invalid_hotspot_args() {
        let response = handle_request(request(
            "tools/call",
            json!({
                "name": "why_hotspots",
                "arguments": { "limit": 0 }
            }),
        ))
        .expect("tool call should return a response");
        assert!(response.result.is_none());
        assert!(response.error.is_some(), "invalid args should error");
        let error = response.error.unwrap_or(JsonRpcError {
            code: 0,
            message: String::new(),
        });
        assert_eq!(error.code, ErrorCode::InvalidParams.as_i32());
        assert!(error.message.contains("limit must be greater than zero"));
    }

    #[test]
    fn workflow_tools_return_builtin_markdown_workflows() {
        let list_response = handle_request(request(
            "tools/call",
            json!({
                "name": "why_list_workflows",
                "arguments": {}
            }),
        ))
        .expect("workflow list should return a response");
        assert!(list_response.error.is_none());
        let list_payload = &list_response.result.unwrap_or(Value::Null)["content"][0]["json"];
        let workflows = list_payload
            .as_array()
            .expect("workflow list should be array");
        assert!(
            workflows
                .iter()
                .any(|workflow| workflow["id"] == "root-cause-archaeology")
        );

        let get_response = handle_request(request(
            "tools/call",
            json!({
                "name": "why_get_workflow",
                "arguments": { "id": "root-cause-archaeology" }
            }),
        ))
        .expect("workflow get should return a response");
        assert!(get_response.error.is_none());
        let workflow = &get_response.result.unwrap_or(Value::Null)["content"][0]["json"];
        assert_eq!(workflow["id"], "root-cause-archaeology");
        assert!(
            workflow["body"]
                .as_str()
                .is_some_and(|body| body.contains("Run the default `why` report first"))
        );
    }
}
