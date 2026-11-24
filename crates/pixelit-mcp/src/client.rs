//! A client of external MCP servers: programs talking over their standard
//! streams and servers reached over HTTP.

use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use pixelit_ai::net::{self, Net};
use pixelit_ai::secrets;
use pixelit_core::{CancelToken, Error, Result};

use crate::config::{ServerConfig, TransportKind};
use crate::protocol::{
    failure, notification, read_tool_result, request, response, rpc_error, METHOD_NOT_FOUND,
    VERSIONS,
};

/// Time a server has to start and answer `initialize`.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// Time one tool call may take.
const CALL_TIMEOUT: Duration = Duration::from_mins(10);

/// How often a waiting request looks at its cancellation token.
const POLL: Duration = Duration::from_millis(100);

/// Pages of a tool list read at most.
const MAX_PAGES: usize = 50;

/// Largest plain JSON answer of an HTTP server: 64 MiB.
const MAX_BODY: u64 = 64 * 1024 * 1024;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A tool of an external server.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTool {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments.
    pub schema: Value,
}

struct Process {
    child: Child,
    stdin: Option<ChildStdin>,
    incoming: Receiver<Value>,
}

struct Http {
    net: Net,
    url: String,
    key: Option<String>,
    session: Option<String>,
    /// Protocol revision agreed in `initialize`.
    version: Option<String>,
}

enum Link {
    Process(Process),
    Http(Http),
}

pub struct Client {
    pub id: String,
    pub tools: Vec<RemoteTool>,
    /// Name and version the server reports.
    pub server: String,
    link: Mutex<Link>,
    next_id: AtomicU64,
}

fn disconnected() -> Error {
    Error::new("mcpDisconnected")
}

/// Starts the program of a server. A bare name such as `npx` is also tried
/// as a script, the way a terminal would find it.
fn spawn(config: &ServerConfig, log: Option<&Path>) -> Result<Child> {
    let program = config.command.trim();
    let mut names = vec![program.to_owned()];
    if cfg!(windows) && Path::new(program).extension().is_none() {
        names.push(format!("{program}.cmd"));
        names.push(format!("{program}.bat"));
    }
    let mut last = None;
    for name in names {
        let mut command = Command::new(&name);
        command
            .args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(match log.and_then(|path| File::create(path).ok()) {
                Some(file) => Stdio::from(file),
                None => Stdio::null(),
            });
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(error) if error.kind() == ErrorKind::NotFound => last = Some(error),
            Err(error) => {
                return Err(Error::new("mcpStartFailed")
                    .with("name", &config.name)
                    .detail(error))
            }
        }
    }
    Err(Error::new("mcpStartFailed")
        .with("name", &config.name)
        .detail(last.map_or_else(|| String::from("not found"), |error| error.to_string())))
}

impl Process {
    fn start(config: &ServerConfig, log: Option<&Path>) -> Result<Self> {
        let mut child = spawn(config, log)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or_else(disconnected)?;
        let (sender, incoming) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else {
                    break;
                };
                // Lines that are not JSON are stray output of the program.
                if let Ok(message) = serde_json::from_str::<Value>(&line) {
                    if sender.send(message).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            incoming,
        })
    }

    fn write(&mut self, message: &Value) -> Result<()> {
        let stdin = self.stdin.as_mut().ok_or_else(disconnected)?;
        let mut line = message.to_string();
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|_| disconnected())
    }

    fn request(
        &mut self,
        id: u64,
        message: &Value,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Value> {
        self.write(message)?;
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.is_cancelled() {
                let _ = self.write(&notification(
                    "notifications/cancelled",
                    json!({ "requestId": id, "reason": "cancelled by the user" }),
                ));
                return Err(Error::cancelled());
            }
            if Instant::now() >= deadline {
                return Err(Error::new("mcpTimeout"));
            }
            let message = match self.incoming.recv_timeout(POLL) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Err(disconnected()),
            };
            if let Some(method) = message["method"].as_str() {
                // The server asks something of the client.
                if let Some(asked) = message.get("id").filter(|id| !id.is_null()) {
                    let answer = match method {
                        "ping" => response(asked, json!({})),
                        "roots/list" => response(asked, json!({ "roots": [] })),
                        _ => failure(asked, METHOD_NOT_FOUND, "not supported"),
                    };
                    self.write(&answer)?;
                }
                continue;
            }
            if message["id"].as_u64() == Some(id) {
                return finish(&message);
            }
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Closing the input asks the program to stop on its own.
        self.stdin = None;
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The result of a response, or its error.
fn finish(message: &Value) -> Result<Value> {
    match message.get("error").filter(|error| !error.is_null()) {
        Some(error) => Err(rpc_error(error)),
        None => Ok(message["result"].clone()),
    }
}

/// Finds the response to request `id` in a stream of server-sent events.
fn read_events(reader: impl Read, id: u64) -> Result<Value> {
    let mut data = String::new();
    for line in BufReader::new(reader).lines() {
        let line = line.map_err(|error| Error::new("network").detail(error))?;
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            continue;
        }
        if !line.is_empty() {
            // Event names, ids and comments carry nothing needed here.
            continue;
        }
        let event = std::mem::take(&mut data);
        if let Ok(message) = serde_json::from_str::<Value>(&event) {
            if message.get("method").is_none() && message["id"].as_u64() == Some(id) {
                return Ok(message);
            }
        }
    }
    Err(disconnected())
}

/// One request and its answer over HTTP. Returns the response message and
/// the session the server assigned.
fn exchange(
    net: &Net,
    url: &str,
    key: Option<&str>,
    headers: &[(String, String)],
    message: &Value,
    id: Option<u64>,
    timeout: Duration,
) -> Result<(Value, Option<String>)> {
    let headers: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let stream = net.open(url, key, &headers, message, Some(timeout))?;
    let session = stream.header("mcp-session-id").map(str::to_owned);
    let Some(id) = id else {
        return Ok((Value::Null, session));
    };
    if stream.content_type == "text/event-stream" {
        return Ok((read_events(stream.into_reader(), id)?, session));
    }
    let mut body = Vec::new();
    stream
        .into_reader()
        .take(MAX_BODY)
        .read_to_end(&mut body)
        .map_err(|error| Error::new("network").detail(error))?;
    let answer: Value =
        serde_json::from_slice(&body).map_err(|error| Error::new("badResponse").detail(error))?;
    let message = match answer {
        Value::Array(batch) => batch
            .into_iter()
            .find(|message| message["id"].as_u64() == Some(id))
            .ok_or_else(|| Error::new("badResponse"))?,
        single => single,
    };
    Ok((message, session))
}

impl Http {
    fn headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![(
            String::from("Accept"),
            String::from("application/json, text/event-stream"),
        )];
        if let Some(session) = &self.session {
            headers.push((String::from("Mcp-Session-Id"), session.clone()));
        }
        if let Some(version) = &self.version {
            headers.push((String::from("MCP-Protocol-Version"), version.clone()));
        }
        headers
    }

    /// Sends a message. With an `id` the answer is awaited on another thread,
    /// so a cancelled request returns at once.
    fn send(
        &mut self,
        id: Option<u64>,
        message: &Value,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Value> {
        let (sender, receiver) = mpsc::channel();
        let net = self.net.clone();
        let url = self.url.clone();
        let key = self.key.clone();
        let headers = self.headers();
        let body = message.clone();
        thread::spawn(move || {
            let outcome = exchange(&net, &url, key.as_deref(), &headers, &body, id, timeout);
            let _ = sender.send(outcome);
        });
        loop {
            if cancel.is_cancelled() {
                if let Some(id) = id {
                    self.forget(id);
                }
                return Err(Error::cancelled());
            }
            match receiver.recv_timeout(POLL) {
                Ok(outcome) => {
                    let (message, session) = outcome?;
                    if session.is_some() {
                        self.session = session;
                    }
                    return if id.is_some() {
                        finish(&message)
                    } else {
                        Ok(Value::Null)
                    };
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(disconnected()),
            }
        }
    }

    /// Tells the server that nobody waits for a request any more.
    fn forget(&self, id: u64) {
        let net = self.net.clone();
        let url = self.url.clone();
        let key = self.key.clone();
        let headers = self.headers();
        thread::spawn(move || {
            let message = notification(
                "notifications/cancelled",
                json!({ "requestId": id, "reason": "cancelled by the user" }),
            );
            let _ = exchange(
                &net,
                &url,
                key.as_deref(),
                &headers,
                &message,
                None,
                Duration::from_secs(10),
            );
        });
    }
}

impl Client {
    /// Starts or reaches the server, greets it and reads its tool list.
    /// `allow_remote` lets the client talk to servers outside this computer.
    pub fn connect(
        config: &ServerConfig,
        net: &Net,
        allow_remote: bool,
        log: Option<&Path>,
        cancel: &CancelToken,
    ) -> Result<Self> {
        let link = match config.transport {
            TransportKind::Stdio => Link::Process(Process::start(config, log)?),
            TransportKind::Http => {
                let url = config.url.trim().to_owned();
                let host =
                    net::host_of(&url).ok_or_else(|| Error::new("badUrl").with("url", &url))?;
                if !net::is_local(&url) && !allow_remote {
                    return Err(Error::new("cloudDisabled").with("host", host));
                }
                Link::Http(Http {
                    net: net.clone(),
                    key: secrets::get(&host),
                    url,
                    session: None,
                    version: None,
                })
            }
        };
        let mut client = Self {
            id: config.id.clone(),
            tools: Vec::new(),
            server: String::new(),
            link: Mutex::new(link),
            next_id: AtomicU64::new(1),
        };
        let hello = client.request(
            "initialize",
            json!({
                "protocolVersion": VERSIONS[0],
                "capabilities": {},
                "clientInfo": { "name": "PixelIt", "version": env!("CARGO_PKG_VERSION") },
            }),
            CONNECT_TIMEOUT,
            cancel,
        )?;
        let version = hello["protocolVersion"].as_str().unwrap_or(VERSIONS[0]);
        if !VERSIONS.contains(&version) {
            return Err(Error::new("mcpVersion").with("version", version));
        }
        if let Link::Http(http) = &mut *client.link.lock().unwrap_or_else(PoisonError::into_inner) {
            http.version = Some(version.to_owned());
        }
        let server = format!(
            "{} {}",
            hello["serverInfo"]["name"].as_str().unwrap_or_default(),
            hello["serverInfo"]["version"].as_str().unwrap_or_default()
        );
        server.trim().clone_into(&mut client.server);
        client.notify("notifications/initialized", json!({}))?;
        client.tools = client.list_tools(cancel)?;
        Ok(client)
    }

    fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = request(id, method, params);
        match &mut *self.link.lock().unwrap_or_else(PoisonError::into_inner) {
            Link::Process(process) => process.request(id, &message, timeout, cancel),
            Link::Http(http) => http.send(Some(id), &message, timeout, cancel),
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        let message = notification(method, params);
        match &mut *self.link.lock().unwrap_or_else(PoisonError::into_inner) {
            Link::Process(process) => process.write(&message),
            Link::Http(http) => http
                .send(None, &message, CONNECT_TIMEOUT, &CancelToken::new())
                .map(|_| ()),
        }
    }

    fn list_tools(&self, cancel: &CancelToken) -> Result<Vec<RemoteTool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let page = self.request("tools/list", params, CONNECT_TIMEOUT, cancel)?;
            for tool in page["tools"].as_array().into_iter().flatten() {
                let Some(name) = tool["name"].as_str() else {
                    continue;
                };
                tools.push(RemoteTool {
                    name: name.to_owned(),
                    description: tool["description"].as_str().unwrap_or_default().to_owned(),
                    schema: match &tool["inputSchema"] {
                        Value::Object(schema) => Value::Object(schema.clone()),
                        _ => json!({ "type": "object", "properties": {} }),
                    },
                });
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(tools)
    }

    /// Runs a tool of the server.
    pub fn call_tool(&self, name: &str, arguments: &Value, cancel: &CancelToken) -> Result<Value> {
        let arguments = match arguments {
            Value::Object(_) => arguments.clone(),
            _ => json!({}),
        };
        let result = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
            CALL_TIMEOUT,
            cancel,
        )?;
        read_tool_result(&result)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn events_are_read_until_the_answer() {
        let stream = ": keep-alive\n\n\
event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n\
data: {\"jsonrpc\":\"2.0\",\"id\":7,\n\
data: \"result\":{\"ok\":true}}\n\n\
data: {\"jsonrpc\":\"2.0\",\"id\":8,\"result\":{}}\n\n";
        let answer = read_events(Cursor::new(stream), 7).unwrap();
        assert_eq!(answer["result"]["ok"], true);
        assert_eq!(read_events(Cursor::new(stream), 8).unwrap()["id"], 8);
        assert_eq!(
            read_events(Cursor::new(stream), 9).unwrap_err().code,
            "mcpDisconnected"
        );
    }

    #[test]
    fn responses_carry_results_or_errors() {
        assert_eq!(
            finish(&json!({ "id": 1, "result": { "x": 1 } })).unwrap()["x"],
            1
        );
        let error = finish(&json!({ "id": 1, "error": { "code": -32000, "message": "boom" } }))
            .unwrap_err();
        assert_eq!(error.code, "mcpFailed");
        assert_eq!(error.params["detail"], "boom");
    }

    #[test]
    fn a_missing_program_is_reported() {
        let config = ServerConfig {
            id: String::from("ghost"),
            name: String::from("Ghost"),
            command: String::from("pixelit-no-such-program"),
            ..ServerConfig::default()
        };
        let error = Client::connect(&config, &Net::new(), false, None, &CancelToken::new())
            .err()
            .unwrap();
        assert_eq!(error.code, "mcpStartFailed");
        assert_eq!(error.params["name"], "Ghost");
    }

    #[test]
    fn remote_servers_need_permission() {
        let config = ServerConfig {
            id: String::from("remote"),
            name: String::from("Remote"),
            transport: TransportKind::Http,
            url: String::from("https://mcp.example.com/mcp"),
            ..ServerConfig::default()
        };
        let error = Client::connect(&config, &Net::new(), false, None, &CancelToken::new())
            .err()
            .unwrap();
        assert_eq!(error.code, "cloudDisabled");
        assert_eq!(error.params["host"], "mcp.example.com");
    }
}
