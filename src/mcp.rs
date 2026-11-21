use crate::search;
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path};

pub const CURRENT: &str = "2026-07-28";
pub const LEGACY: &str = "2025-11-25";

fn tools() -> Value {
    let tool = |name, description, properties, required| {
        json!({
            "name":name, "description":description,
            "annotations":{"readOnlyHint":true,"openWorldHint":false},
            "inputSchema":{"type":"object","properties":properties,"required":[required],"additionalProperties":false}
        })
    };
    json!([
        tool(
            "search",
            "Search published Markdown and PDF passages by keyword. Use the id with fetch for more context.",
            json!({"query":{"type":"string","minLength":1,"maxLength":512},
                   "limit":{"type":"integer","minimum":1,"maximum":20,"default":5}}),
            "query"
        ),
        tool(
            "fetch",
            "Fetch a passage by search result id, with neighboring passages in document order.",
            json!({"id":{"type":"string","minLength":1},
                   "context":{"type":"integer","minimum":0,"maximum":2,"default":1}}),
            "id"
        )
    ])
}

fn integer(args: &Value, field: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    let Some(value) = args.get(field) else {
        return Ok(default);
    };
    value
        .as_u64()
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| anyhow::anyhow!("{field} must be an integer from {min} to {max}"))
}

fn call(root: &Path, name: &str, args: &Value) -> Result<Vec<Value>> {
    let args_map = args
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("arguments must be an object"))?;
    if name == "search" {
        ensure!(
            args_map
                .keys()
                .all(|k| matches!(k.as_str(), "query" | "limit")),
            "Unexpected search argument"
        );
        let query = args["query"].as_str().unwrap_or("").trim();
        ensure!(
            !query.is_empty() && query.chars().count() <= 512,
            "query must contain 1 to 512 characters"
        );
        let limit = integer(args, "limit", 5, 1, 20)?;
        search::search(&search::open(root)?, query, limit)
    } else {
        ensure!(
            args_map
                .keys()
                .all(|k| matches!(k.as_str(), "id" | "context")),
            "Unexpected fetch argument"
        );
        let id = args["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Pass a passage id returned by search"))?;
        let context = integer(args, "context", 1, 0, 2)?;
        search::fetch(&search::open(root)?, id, context)
    }
}

pub struct Reply {
    pub status: &'static str,
    pub body: Option<Value>,
}

fn error(status: &'static str, id: &Value, code: i32, message: &str) -> Reply {
    Reply {
        status,
        body: Some(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})),
    }
}

fn name_matches(header: Option<&&str>, value: &Value) -> bool {
    let (Some(header), Some(value)) = (header, value.as_str()) else {
        return false;
    };
    if let Some(encoded) = header
        .strip_prefix("=?base64?")
        .and_then(|s| s.strip_suffix("?="))
    {
        return STANDARD
            .decode(encoded)
            .is_ok_and(|bytes| bytes == value.as_bytes());
    }
    *header == value
}

pub fn dispatch(root: &Path, headers: &HashMap<String, &str>, bytes: &[u8]) -> Reply {
    let Ok(request) = serde_json::from_slice::<Value>(bytes) else {
        return error("400 Bad Request", &Value::Null, -32700, "Parse error");
    };
    let id = &request["id"];
    let method = request["method"].as_str().unwrap_or("");
    let params = &request["params"];
    if request["jsonrpc"] != "2.0"
        || method.is_empty()
        || request.get("id").is_some() && !(id.is_string() || id.is_i64() || id.is_u64())
        || request.get("params").is_some() && !params.is_object()
    {
        return error(
            "400 Bad Request",
            &Value::Null,
            -32600,
            "Invalid JSON-RPC request",
        );
    }
    let version = headers.get("mcp-protocol-version").copied();
    let meta = &params["_meta"];
    let body_version = meta["io.modelcontextprotocol/protocolVersion"].as_str();
    let modern = body_version.is_some() || version == Some(CURRENT) || method == "server/discover";
    if let Some(v) = version.or(body_version)
        && v != LEGACY
        && v != CURRENT
    {
        let mut reply = error(
            "400 Bad Request",
            id,
            -32022,
            "Unsupported protocol version",
        );
        reply.body.as_mut().unwrap()["error"]["data"] =
            json!({"requested":v,"supported":[CURRENT,LEGACY]});
        return reply;
    }
    if modern
        && (body_version != Some(CURRENT)
            || version != body_version
            || headers.get("mcp-method").copied() != Some(method)
            || method == "tools/call" && !name_matches(headers.get("mcp-name"), &params["name"]))
    {
        return error(
            "400 Bad Request",
            id,
            -32020,
            "Missing or mismatched MCP request headers",
        );
    }
    if id.is_null() {
        return Reply {
            status: "202 Accepted",
            body: None,
        };
    }
    let info = json!({"name":"memoc","version":env!("CARGO_PKG_VERSION")});
    let mut result = match method {
        "server/discover" => {
            json!({"supportedVersions":[CURRENT,LEGACY],"capabilities":{"tools":{}}})
        }
        "initialize" if !modern => {
            json!({"protocolVersion":LEGACY,"serverInfo":info,"capabilities":{"tools":{}}})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":tools()}),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("");
            if !matches!(name, "search" | "fetch") {
                return error("200 OK", id, -32602, "Unknown tool");
            }
            match call(root, name, &params["arguments"]) {
                Ok(rows) => {
                    let payload = json!({"passages":rows});
                    json!({"content":[{"type":"text","text":payload.to_string()}],"structuredContent":payload})
                }
                Err(error) => {
                    json!({"isError":true,"content":[{"type":"text","text":error.to_string()}]})
                }
            }
        }
        _ => {
            return error(
                if modern { "404 Not Found" } else { "200 OK" },
                id,
                -32601,
                "Method not found",
            );
        }
    };
    if modern {
        result["_meta"] = json!({"io.modelcontextprotocol/serverInfo":info});
        result["resultType"] = json!("complete");
        if matches!(method, "tools/list" | "server/discover") {
            result["ttlMs"] = json!(0);
            result["cacheScope"] = json!("public");
        }
    }
    Reply {
        status: "200 OK",
        body: Some(json!({"jsonrpc":"2.0","id":id,"result":result})),
    }
}
