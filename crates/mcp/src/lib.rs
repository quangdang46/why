use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use why_archaeologist::analyze_target;
use why_locator::parse_target;
use why_scanner::{scan_coupling, scan_hotspots, scan_rename_safe, scan_time_bombs};
use why_splitter::suggest_split;

const JSONRPC_VERSION: &str = "2.0";
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
            Err(error) => JsonRpcResponse::error(
                None,
                ErrorCode::ParseError,
                format!("failed to parse JSON-RPC request: {error}"),
            ),
        };

        serde_json::to_writer(&mut writer, &response).context("failed to serialize response")?;
        writer.write_all(b"\n").context("failed to write newline")?;
        writer.flush().context("failed to flush stdout")?;
    }

    Ok(())
}

fn handle_request(request: JsonRpcRequest) -> JsonRpcResponse {
    if request.jsonrpc != JSONRPC_VERSION {
        return JsonRpcResponse::error(
            request.id,
            ErrorCode::InvalidRequest,
            format!("unsupported jsonrpc version: {}", request.jsonrpc),
        );
    }

    let id = request.id;
    let result = match request.method.as_str() {
        "initialize" => Ok(initialize_result()),
        "tools/list" => Ok(tools_list_result()),
        "tools/call" => call_tool(request.params),
        _ => Err(McpError::new(
            ErrorCode::MethodNotFound,
            format!("unknown method: {}", request.method),
        )),
    };

    match result {
        Ok(value) => JsonRpcResponse::success(id, value),
        Err(error) => JsonRpcResponse::error(id, error.code, error.message),
    }
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": JSONRPC_VERSION,
        "serverInfo": {
            "name": "why",
            "version": env!("CARGO_PKG_VERSION")
        },
        "capabilities": {
            "tools": {}
        }
    })
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
            jsonrpc: JSONRPC_VERSION,
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Option<RequestId>, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION,
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
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(RequestId::Number(1)),
            method: method.to_string(),
            params: Some(params),
        }
    }

    #[test]
    fn initialize_returns_server_metadata() {
        let response = handle_request(request("initialize", json!({})));
        assert!(response.error.is_none());
        assert!(response.result.is_some(), "initialize should return result");
        let result = response.result.unwrap_or(Value::Null);
        assert_eq!(result["protocolVersion"], JSONRPC_VERSION);
        assert_eq!(result["serverInfo"]["name"], "why");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_returns_expected_tools() {
        let response = handle_request(request("tools/list", json!({})));
        assert!(response.error.is_none());
        assert!(response.result.is_some(), "tools/list should return result");
        let result = response.result.unwrap_or(Value::Null);
        assert!(result["tools"].is_array(), "tools should be array");
        let empty_tools = Vec::new();
        let tools = result["tools"].as_array().unwrap_or(&empty_tools);
        assert_eq!(tools.len(), 6);
        assert_eq!(tools[0]["name"], "why_symbol");
        assert_eq!(tools[1]["name"], "why_split");
        assert_eq!(tools[2]["name"], "why_time_bombs");
        assert_eq!(tools[3]["name"], "why_hotspots");
        assert_eq!(tools[4]["name"], "why_coupling");
        assert_eq!(tools[5]["name"], "why_rename_safe");
    }

    #[test]
    fn rejects_unknown_method() {
        let response = handle_request(request("wat", json!({})));
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
        });
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
        ));
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
        ));
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
        ));
        assert!(response.result.is_none());
        assert!(response.error.is_some(), "invalid args should error");
        let error = response.error.unwrap_or(JsonRpcError {
            code: 0,
            message: String::new(),
        });
        assert_eq!(error.code, ErrorCode::InvalidParams.as_i32());
        assert!(error.message.contains("limit must be greater than zero"));
    }
}
