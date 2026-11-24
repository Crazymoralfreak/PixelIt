//! JSON-RPC messages of the Model Context Protocol.

use serde_json::{json, Value};

use pixelit_core::{Error, Result};

/// Protocol revisions this implementation speaks, newest first.
pub const VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// Most characters of a tool result passed on as text.
const MAX_TEXT: usize = 64_000;

pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

pub fn response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn failure(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The revision to answer an `initialize` request with: the requested one
/// when it is known, the newest one otherwise.
pub fn negotiate(requested: &str) -> &'static str {
    VERSIONS
        .iter()
        .find(|version| **version == requested)
        .copied()
        .unwrap_or(VERSIONS[0])
}

/// The outcome of a tool in the shape of a `tools/call` result. A failure is
/// a result too, so the model can read the reason and try something else.
pub fn tool_result(outcome: Result<Value>) -> Value {
    match outcome {
        Ok(value) => {
            let mut result = json!({
                "content": [{ "type": "text", "text": value.to_string() }],
                "isError": false,
            });
            if value.is_object() {
                result["structuredContent"] = value;
            }
            result
        }
        Err(error) => {
            let report = json!({ "error": error.code, "details": error.params });
            json!({
                "content": [{ "type": "text", "text": report.to_string() }],
                "isError": true,
            })
        }
    }
}

/// Reads a `tools/call` result of another server: the structured value when
/// there is one, the joined text otherwise.
pub fn read_tool_result(result: &Value) -> Result<Value> {
    let mut text = String::new();
    let mut other = 0u32;
    for part in result["content"].as_array().into_iter().flatten() {
        match part["type"].as_str() {
            Some("text") => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(part["text"].as_str().unwrap_or_default());
            }
            _ => other += 1,
        }
    }
    if text.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    if result["isError"].as_bool().unwrap_or(false) {
        return Err(Error::new("mcpToolFailed").detail(text));
    }
    if let Some(structured) = result
        .get("structuredContent")
        .filter(|value| value.is_object())
    {
        return Ok(structured.clone());
    }
    let mut answer = json!({ "text": text });
    if other > 0 {
        // Pictures and other attachments are counted, not passed on.
        answer["attachments"] = json!(other);
    }
    Ok(answer)
}

/// The error of a JSON-RPC response as an application error.
pub fn rpc_error(error: &Value) -> Error {
    Error::new("mcpFailed")
        .with("status", error["code"].as_i64().unwrap_or(0))
        .detail(error["message"].as_str().unwrap_or("unknown error"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_negotiated() {
        assert_eq!(negotiate("2024-11-05"), "2024-11-05");
        assert_eq!(negotiate("1999-01-01"), VERSIONS[0]);
    }

    #[test]
    fn tool_outcomes_become_results() {
        let ok = tool_result(Ok(json!({ "done": 2 })));
        assert_eq!(ok["isError"], false);
        assert_eq!(ok["structuredContent"]["done"], 2);
        assert_eq!(ok["content"][0]["text"], "{\"done\":2}");
        let list = tool_result(Ok(json!([1, 2])));
        assert!(list.get("structuredContent").is_none());
        let failed = tool_result(Err(Error::new("agentPathDenied").with("path", "C:\\x")));
        assert_eq!(failed["isError"], true);
        assert!(failed["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("agentPathDenied"));
    }

    #[test]
    fn foreign_results_are_read() {
        let plain = read_tool_result(&json!({
            "content": [
                { "type": "text", "text": "one" },
                { "type": "image", "data": "...", "mimeType": "image/png" },
                { "type": "text", "text": "two" },
            ],
        }))
        .unwrap();
        assert_eq!(plain["text"], "one\ntwo");
        assert_eq!(plain["attachments"], 1);
        let structured = read_tool_result(&json!({
            "content": [{ "type": "text", "text": "{}" }],
            "structuredContent": { "value": 7 },
        }))
        .unwrap();
        assert_eq!(structured["value"], 7);
        let failed = read_tool_result(&json!({
            "content": [{ "type": "text", "text": "no such file" }],
            "isError": true,
        }))
        .unwrap_err();
        assert_eq!(failed.code, "mcpToolFailed");
        assert_eq!(failed.params["detail"], "no such file");
    }

    #[test]
    fn messages_have_the_expected_shape() {
        assert_eq!(request(3, "ping", json!({}))["id"], 3);
        assert!(notification("x", json!({})).get("id").is_none());
        assert_eq!(
            failure(&json!(1), METHOD_NOT_FOUND, "nope")["error"]["code"],
            -32601
        );
        assert_eq!(
            rpc_error(&json!({ "code": -1, "message": "bad" })).params["detail"],
            "bad"
        );
    }
}
