//! The MCP server of PixelIt: `PixelIt.exe mcp`.
//!
//! Messages are JSON-RPC, one per line, on the standard input and output.
//! The server offers the tool registry of `pixelit-tools`, so an external
//! agent can do exactly what the built-in assistant can, under the same
//! permissions of the settings.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::{Mutex, PoisonError};
use std::thread::{self, Scope};

use serde_json::{json, Value};

use pixelit_core::CancelToken;
use pixelit_tools::ToolContext;

use crate::protocol::{
    failure, negotiate, response, tool_result, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND,
    PARSE_ERROR,
};

struct State<'a, W> {
    context: &'a ToolContext,
    output: Mutex<W>,
    /// Running tool calls by request id, for cancellation.
    running: Mutex<HashMap<String, CancelToken>>,
}

impl<W: Write> State<'_, W> {
    fn send(&self, message: &Value) {
        let mut output = self.output.lock().unwrap_or_else(PoisonError::into_inner);
        let mut line = message.to_string();
        line.push('\n');
        if output
            .write_all(line.as_bytes())
            .and_then(|()| output.flush())
            .is_err()
        {
            // The client is gone; nothing waits for the results any more.
            self.cancel_all();
        }
    }

    fn cancel_all(&self) {
        for token in self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            token.cancel();
        }
    }
}

fn instructions(context: &ToolContext) -> String {
    let access = context.access();
    let folders = if access.folders.is_empty() {
        String::from(
            "No folder is open to agents yet: the user adds folders in PixelIt, Settings → Agents.",
        )
    } else {
        format!(
            "Files can be read and written inside: {}.",
            access.folders.join("; ")
        )
    };
    format!(
        "PixelIt converts, compresses, resizes and edits images in batches, removes backgrounds, \
enlarges pictures and generates them. Pass absolute paths. {folders} Results are written as new \
files unless the user allowed replacing; call ai_status to see what is installed and allowed."
    )
}

fn tool_list() -> Value {
    let tools: Vec<Value> = pixelit_tools::tools()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": (tool.schema)(),
            })
        })
        .collect();
    json!({ "tools": tools })
}

fn handle<'scope, W: Write + Send>(
    scope: &'scope Scope<'scope, '_>,
    state: &'scope State<'scope, W>,
    message: Value,
) {
    let Some(method) = message["method"].as_str() else {
        // A response to something this server never asks, or not a message.
        if message.get("result").is_none() && message.get("error").is_none() {
            state.send(&failure(&Value::Null, INVALID_REQUEST, "not a request"));
        }
        return;
    };
    let id = message.get("id").filter(|id| !id.is_null()).cloned();
    let params = &message["params"];
    let Some(id) = id else {
        if method == "notifications/cancelled" {
            let key = params["requestId"].to_string();
            if let Some(token) = state
                .running
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&key)
            {
                token.cancel();
            }
        }
        return;
    };
    match method {
        "initialize" => {
            let version = negotiate(params["protocolVersion"].as_str().unwrap_or_default());
            state.send(&response(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": "pixelit",
                        "title": "PixelIt",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": instructions(state.context),
                }),
            ));
        }
        "ping" => state.send(&response(&id, json!({}))),
        "tools/list" => state.send(&response(&id, tool_list())),
        "tools/call" => {
            let Some(name) = params["name"].as_str().map(str::to_owned) else {
                state.send(&failure(&id, INVALID_PARAMS, "missing tool name"));
                return;
            };
            if !pixelit_tools::tools().iter().any(|tool| tool.name == name) {
                state.send(&failure(
                    &id,
                    INVALID_PARAMS,
                    &format!("unknown tool: {name}"),
                ));
                return;
            }
            let arguments = match &params["arguments"] {
                Value::Null => json!({}),
                other => other.clone(),
            };
            let key = id.to_string();
            let cancel = CancelToken::new();
            state
                .running
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key.clone(), cancel.clone());
            scope.spawn(move || {
                let context = ToolContext {
                    cancel,
                    ..state.context.clone()
                };
                let outcome = pixelit_tools::call(&context, &name, &arguments);
                state
                    .running
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&key);
                state.send(&response(&id, tool_result(outcome)));
            });
        }
        // Optional parts of the protocol a client may probe.
        "resources/list" => state.send(&response(&id, json!({ "resources": [] }))),
        "prompts/list" => state.send(&response(&id, json!({ "prompts": [] }))),
        _ => state.send(&failure(
            &id,
            METHOD_NOT_FOUND,
            &format!("unknown method: {method}"),
        )),
    }
}

/// Serves requests until the input closes. Tool calls run side by side; a
/// closed input cancels the ones still running.
pub fn serve<R: BufRead, W: Write + Send>(context: &ToolContext, input: R, output: W) {
    let state = State {
        context,
        output: Mutex::new(output),
        running: Mutex::new(HashMap::new()),
    };
    tracing::info!("MCP server started");
    thread::scope(|scope| {
        for line in input.lines() {
            let Ok(line) = line else {
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(Value::Array(batch)) => {
                    for message in batch {
                        handle(scope, &state, message);
                    }
                }
                Ok(message) => handle(scope, &state, message),
                Err(_) => state.send(&failure(&Value::Null, PARSE_ERROR, "invalid JSON")),
            }
        }
        state.cancel_all();
    });
    tracing::info!("MCP server stopped");
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{self, BufReader, Cursor, Read};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use pixelit_ai::Ai;
    use pixelit_engine::{Engine, Paths};

    use super::*;

    fn context() -> (tempfile::TempDir, ToolContext, PathBuf) {
        let folder = tempfile::tempdir().unwrap();
        let engine = Arc::new(Engine::open(Paths::at(folder.path().join("data"))).unwrap());
        let ai = Arc::new(Ai::open(engine.paths.clone()));
        let allowed = folder.path().join("pictures");
        fs::create_dir_all(&allowed).unwrap();
        engine
            .update_settings(|settings| {
                settings.agents.folders = vec![allowed.to_string_lossy().into_owned()];
            })
            .unwrap();
        (folder, ToolContext::new(engine, ai), allowed)
    }

    /// What the server wrote, shared with the input of the test client.
    #[derive(Clone, Default)]
    struct Answers(Arc<Mutex<Vec<u8>>>);

    impl Answers {
        fn bytes(&self) -> Vec<u8> {
            self.0.lock().unwrap().clone()
        }
    }

    impl Write for Answers {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The input of a client that keeps the connection open until it has its
    /// answers, as real clients do: a closed input cancels running calls.
    struct Client {
        requests: Cursor<String>,
        answers: Answers,
        expected: usize,
    }

    impl Read for Client {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = self.requests.read(buffer)?;
            if read == 0 {
                let deadline = Instant::now() + Duration::from_secs(60);
                let answered =
                    |answers: &Answers| String::from_utf8_lossy(&answers.bytes()).lines().count();
                while answered(&self.answers) < self.expected && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(5));
                }
            }
            Ok(read)
        }
    }

    /// Runs a session and returns the answers by request id.
    fn session(context: &ToolContext, messages: &[Value]) -> HashMap<String, Value> {
        let mut input = String::new();
        for message in messages {
            input.push_str(&message.to_string());
            input.push('\n');
        }
        let answers = Answers::default();
        let client = Client {
            requests: Cursor::new(input),
            answers: answers.clone(),
            expected: messages
                .iter()
                .filter(|message| message.get("id").is_some())
                .count(),
        };
        serve(context, BufReader::new(client), answers.clone());
        String::from_utf8(answers.bytes())
            .unwrap()
            .lines()
            .map(|line| {
                let answer: Value = serde_json::from_str(line).unwrap();
                (answer["id"].to_string(), answer)
            })
            .collect()
    }

    #[test]
    fn a_session_lists_and_calls_tools() {
        let (_folder, context, allowed) = context();
        let answers = session(
            &context,
            &[
                json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                    "protocolVersion": "2025-03-26", "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1" } } }),
                json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
                json!({ "jsonrpc": "2.0", "id": "three", "method": "tools/call", "params": {
                    "name": "list_images", "arguments": { "folder": allowed } } }),
                json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
                    "name": "list_images", "arguments": { "folder": "C:\\Windows" } } }),
                json!({ "jsonrpc": "2.0", "id": 5, "method": "ping" }),
            ],
        );
        assert_eq!(answers.len(), 5);
        let hello = &answers["1"]["result"];
        assert_eq!(hello["protocolVersion"], "2025-03-26");
        assert_eq!(hello["serverInfo"]["name"], "pixelit");
        assert!(hello["instructions"].as_str().unwrap().contains("pictures"));
        let tools = answers["2"]["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["name"] == "convert"));
        assert!(tools
            .iter()
            .all(|tool| tool["inputSchema"]["type"] == "object"));
        let listed = &answers["\"three\""]["result"];
        assert_eq!(listed["isError"], false, "{listed}");
        assert_eq!(listed["structuredContent"]["count"], 0);
        let denied = &answers["4"]["result"];
        assert_eq!(denied["isError"], true);
        assert_eq!(answers["5"]["result"], json!({}));
    }

    #[test]
    fn bad_input_is_answered_with_errors() {
        let (_folder, context, _allowed) = context();
        let mut output = Vec::new();
        let input = "not json\n\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"nope\"}\n\
{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"nope\"}}\n\
[{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}]\n{\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{}}\n";
        serve(&context, Cursor::new(input), &mut output);
        let lines: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0]["error"]["code"], PARSE_ERROR);
        assert_eq!(lines[1]["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(lines[2]["error"]["code"], INVALID_PARAMS);
        assert_eq!(lines[3]["result"], json!({}));
    }
}
