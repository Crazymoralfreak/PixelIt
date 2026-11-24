//! Chat with a language model through the OpenAI-compatible protocol that
//! Ollama, LM Studio, llama.cpp and cloud services all speak.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use pixelit_core::{Error, Result};

use crate::config::{AiSettings, Endpoint};
use crate::generate::access;
use crate::net::Net;

const CHAT_TIMEOUT: Duration = Duration::from_mins(5);
const LIST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// A request of the model to run a tool.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// JPEG or PNG pictures attached to a user message; not stored in chats.
    #[serde(skip)]
    pub images: Vec<Vec<u8>>,
    /// Tools the assistant asked for in this message.
    pub tool_calls: Vec<ToolCall>,
    /// For a tool result: the call it answers.
    pub tool_call_id: String,
}

impl Message {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            ..Self::default()
        }
    }
}

/// A tool offered to the model.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments.
    pub parameters: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

fn image_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else {
        "image/jpeg"
    }
}

fn wire(message: &Message) -> Value {
    match message.role {
        Role::Tool => json!({
            "role": "tool",
            "tool_call_id": message.tool_call_id,
            "content": message.content,
        }),
        Role::Assistant if !message.tool_calls.is_empty() => {
            let calls: Vec<Value> = message
                .tool_calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": { "name": call.name, "arguments": call.arguments.to_string() },
                    })
                })
                .collect();
            json!({ "role": "assistant", "content": message.content, "tool_calls": calls })
        }
        Role::System | Role::User | Role::Assistant => {
            if message.images.is_empty() {
                json!({ "role": message.role.name(), "content": message.content })
            } else {
                let mut parts = vec![json!({ "type": "text", "text": message.content })];
                for image in &message.images {
                    let url = format!(
                        "data:{};base64,{}",
                        image_type(image),
                        STANDARD.encode(image)
                    );
                    parts.push(json!({ "type": "image_url", "image_url": { "url": url } }));
                }
                json!({ "role": message.role.name(), "content": parts })
            }
        }
    }
}

/// Removes the reasoning some models print before the answer.
pub fn strip_thinking(text: &str) -> String {
    let mut rest = text;
    let mut out = String::new();
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("</think>") {
            rest = &rest[start + end + "</think>".len()..];
        } else {
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
}

fn parse_reply(answer: &Value) -> Result<Reply> {
    let message = &answer["choices"][0]["message"];
    if message.is_null() {
        return Err(Error::new("badResponse").detail(
            answer["error"]["message"]
                .as_str()
                .unwrap_or("no message in the answer"),
        ));
    }
    let content = match &message["content"] {
        Value::String(text) => strip_thinking(text),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    let mut tool_calls = Vec::new();
    for (index, call) in message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let function = &call["function"];
        let Some(name) = function["name"].as_str() else {
            continue;
        };
        let arguments = match &function["arguments"] {
            Value::String(text) if text.trim().is_empty() => json!({}),
            Value::String(text) => {
                serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
            }
            Value::Null => json!({}),
            other => other.clone(),
        };
        tool_calls.push(ToolCall {
            id: call["id"]
                .as_str()
                .map_or_else(|| format!("call_{index}"), str::to_owned),
            name: name.to_owned(),
            arguments,
        });
    }
    Ok(Reply {
        content,
        tool_calls,
    })
}

/// Sends the conversation and returns the model's next message.
pub fn chat(
    net: &Net,
    settings: &AiSettings,
    endpoint: &Endpoint,
    messages: &[Message],
    tools: &[ToolSpec],
) -> Result<Reply> {
    if !endpoint.is_set() {
        return Err(Error::new("providerNotSet"));
    }
    let key = access(settings, &endpoint.url)?;
    let mut body = json!({
        "model": endpoint.model,
        "messages": messages.iter().map(wire).collect::<Vec<_>>(),
        "stream": false,
    });
    if !tools.is_empty() {
        body["tools"] = tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    },
                })
            })
            .collect();
    }
    let answer = net.post_json(
        &format!("{}/chat/completions", endpoint.url),
        key.as_deref(),
        &body,
        Some(CHAT_TIMEOUT),
    )?;
    parse_reply(&answer)
}

/// Models a server offers.
pub fn models(net: &Net, settings: &AiSettings, url: &str) -> Result<Vec<String>> {
    let url = url.trim().trim_end_matches('/');
    let key = access(settings, url)?;
    let answer = net.get_json(&format!("{url}/models"), key.as_deref(), Some(LIST_TIMEOUT))?;
    let mut names: Vec<String> = answer["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["id"].as_str().map(str::to_owned))
        .collect();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_take_the_wire_form() {
        let mut user = Message::new(Role::User, "what is this?");
        assert_eq!(wire(&user)["content"], "what is this?");
        user.images.push(vec![0xff, 0xd8, 0xff]);
        let with_image = wire(&user);
        assert_eq!(with_image["content"][0]["text"], "what is this?");
        assert!(with_image["content"][1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/jpeg;base64,"));

        let mut assistant = Message::new(Role::Assistant, "");
        assistant.tool_calls.push(ToolCall {
            id: String::from("call_1"),
            name: String::from("resize"),
            arguments: json!({ "width": 800 }),
        });
        let wired = wire(&assistant);
        assert_eq!(
            wired["tool_calls"][0]["function"]["arguments"],
            r#"{"width":800}"#
        );

        let mut tool = Message::new(Role::Tool, "done");
        tool.tool_call_id = String::from("call_1");
        assert_eq!(wire(&tool)["tool_call_id"], "call_1");
    }

    #[test]
    fn replies_are_parsed() {
        let answer = json!({ "choices": [{ "message": {
            "content": "<think>hm</think> Sure.",
            "tool_calls": [
                { "id": "a", "function": { "name": "resize", "arguments": "{\"width\": 640}" } },
                { "function": { "name": "list", "arguments": "" } },
                { "function": { "name": "odd", "arguments": "not json" } },
            ],
        } }] });
        let reply = parse_reply(&answer).unwrap();
        assert_eq!(reply.content, "Sure.");
        assert_eq!(reply.tool_calls.len(), 3);
        assert_eq!(reply.tool_calls[0].arguments["width"], 640);
        assert_eq!(reply.tool_calls[1].id, "call_1");
        assert_eq!(reply.tool_calls[1].arguments, json!({}));
        assert_eq!(reply.tool_calls[2].arguments, "not json");
    }

    #[test]
    fn errors_of_the_server_are_reported() {
        let answer = json!({ "error": { "message": "model not found" } });
        let error = parse_reply(&answer).unwrap_err();
        assert_eq!(error.code, "badResponse");
        assert_eq!(error.params["detail"], "model not found");
    }

    #[test]
    fn thinking_is_removed() {
        assert_eq!(strip_thinking("<think>a</think>b<think>c</think> d"), "b d");
        assert_eq!(strip_thinking("plain"), "plain");
        assert_eq!(strip_thinking("x <think>never closed"), "x");
    }

    #[test]
    fn an_unset_endpoint_is_refused() {
        let error = chat(
            &Net::new(),
            &AiSettings::default(),
            &Endpoint::default(),
            &[],
            &[],
        )
        .unwrap_err();
        assert_eq!(error.code, "providerNotSet");
    }
}
