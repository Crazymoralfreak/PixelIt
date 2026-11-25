//! The assistant: a conversation with a language model that may call tools.
//!
//! The model sees the built-in tools of PixelIt and the tools of connected
//! MCP servers through the same [`ToolSource`] interface.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use pixelit_ai::{Ai, Message, Reply, Role, ToolCall, ToolSpec};
use pixelit_core::{CancelToken, Error, Result};
use pixelit_engine::settings::AgentAccess;
use pixelit_engine::{clock, store, Paths};

use crate::ToolContext;

pub const CHATS_VERSION: u32 = 1;

/// Rounds of tool calls within one answer.
const MAX_ROUNDS: usize = 24;

/// Messages of the conversation a model receives.
const MAX_HISTORY: usize = 60;

/// Characters of a tool result a model receives.
const MAX_RESULT: usize = 16_000;

/// Conversations kept on disk.
const MAX_CHATS: usize = 100;

const TITLE_LENGTH: usize = 60;

/// Where tools come from.
pub trait ToolSource: Send + Sync {
    fn specs(&self) -> Vec<ToolSpec>;

    /// Whether the user must agree to this call first.
    fn needs_approval(&self, _name: &str) -> bool {
        false
    }

    fn call(&self, name: &str, arguments: &Value, cancel: &CancelToken) -> Result<Value>;
}

/// The built-in tools of PixelIt.
pub struct Builtin(pub ToolContext);

impl ToolSource for Builtin {
    fn specs(&self) -> Vec<ToolSpec> {
        crate::specs()
    }

    fn call(&self, name: &str, arguments: &Value, cancel: &CancelToken) -> Result<Value> {
        let context = ToolContext {
            cancel: cancel.clone(),
            ..self.0.clone()
        };
        crate::call(&context, name, arguments)
    }
}

/// The language model behind the assistant.
pub trait Model {
    fn reply(&self, messages: &[Message], tools: &[ToolSpec]) -> Result<Reply>;
}

impl Model for Ai {
    fn reply(&self, messages: &[Message], tools: &[ToolSpec]) -> Result<Reply> {
        self.chat(messages, tools)
    }
}

/// What happens during an answer, for the interface.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatEvent {
    /// The model said something or asked for tools.
    Message {
        message: Message,
    },
    ToolStart {
        call: ToolCall,
    },
    #[serde(rename_all = "camelCase")]
    ToolEnd {
        id: String,
        ok: bool,
        result: Value,
    },
}

pub struct Assistant {
    sources: Vec<Arc<dyn ToolSource>>,
    language: String,
    access: AgentAccess,
}

/// The instructions every conversation starts with.
pub fn system_prompt(language: &str, access: &AgentAccess) -> String {
    let folders = if access.folders.is_empty() {
        String::from("No folder is open to you yet. Ask the user to add one in Settings → Agents.")
    } else {
        format!(
            "You may read and write files only inside these folders: {}.",
            access.folders.join("; ")
        )
    };
    let mut rules = String::new();
    if !access.allow_overwrite {
        rules.push_str(" Existing files are never replaced: results are written as new files.");
    }
    if !access.allow_delete {
        rules.push_str(" You cannot delete files.");
    }
    if !access.allow_downloads {
        rules.push_str(
            " You cannot download models; tell the user which one to install on the Models page.",
        );
    }
    if !access.allow_cloud {
        rules.push_str(" Pictures are not sent to servers outside this computer.");
    }
    format!(
        "You are the assistant of PixelIt, a desktop application for working with images: \
converting, compressing, resizing, batch processing, removing backgrounds, enlarging, generating.\n\
Use the tools to do what the user asks; prefer one call over many files to many calls. \
Always pass absolute paths. Look at a folder with list_images before processing it when the user \
did not name the files. When a tool fails, explain the reason in plain words and say what to do.\n\
{folders}{rules}\n\
After the work, report briefly: what was done, how many files, where the results are. \
Answer in {language}.",
        language = pixelit_ai::vision::language_name(language),
    )
}

/// The last messages of a conversation, starting at a user message so that
/// tool results never lose the call they answer.
fn window(messages: &[Message]) -> &[Message] {
    if messages.len() <= MAX_HISTORY {
        return messages;
    }
    let start = messages.len() - MAX_HISTORY;
    let shift = messages[start..]
        .iter()
        .position(|message| message.role == Role::User)
        .unwrap_or(0);
    &messages[start + shift..]
}

fn clip(mut text: String) -> String {
    if text.len() > MAX_RESULT {
        let mut end = MAX_RESULT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("… (truncated)");
    }
    text
}

impl Assistant {
    pub fn new(context: &ToolContext, sources: Vec<Arc<dyn ToolSource>>) -> Self {
        Self {
            sources,
            language: context.language.clone(),
            access: context.access(),
        }
    }

    /// Every tool offered to the model, with the source it belongs to. A name
    /// offered twice keeps its first source.
    fn tools(&self) -> Vec<(ToolSpec, usize)> {
        let mut tools: Vec<(ToolSpec, usize)> = Vec::new();
        for (index, source) in self.sources.iter().enumerate() {
            for spec in source.specs() {
                if !tools.iter().any(|(known, _)| known.name == spec.name) {
                    tools.push((spec, index));
                }
            }
        }
        tools
    }

    fn run_tool(
        &self,
        tools: &[(ToolSpec, usize)],
        call: &ToolCall,
        approve: &dyn Fn(&ToolCall) -> bool,
        cancel: &CancelToken,
    ) -> Result<Value> {
        let (_, index) = tools
            .iter()
            .find(|(spec, _)| spec.name == call.name)
            .ok_or_else(|| Error::new("unknownTool").with("name", &call.name))?;
        let source = &self.sources[*index];
        if source.needs_approval(&call.name) && !approve(call) {
            return Err(Error::new("toolDeclined").with("name", &call.name));
        }
        source.call(&call.name, &call.arguments, cancel)
    }

    /// Answers the last user message of the conversation. New messages are
    /// appended to `messages`; a cancelled or failed answer keeps what was
    /// said so far, so the conversation stays consistent.
    pub fn answer(
        &self,
        model: &dyn Model,
        messages: &mut Vec<Message>,
        events: &mut dyn FnMut(ChatEvent),
        approve: &dyn Fn(&ToolCall) -> bool,
        cancel: &CancelToken,
    ) -> Result<()> {
        let tools = self.tools();
        let specs: Vec<ToolSpec> = tools.iter().map(|(spec, _)| spec.clone()).collect();
        let system = Message::new(Role::System, system_prompt(&self.language, &self.access));
        for _ in 0..MAX_ROUNDS {
            cancel.check()?;
            let mut request = vec![system.clone()];
            request.extend_from_slice(window(messages));
            let reply = model.reply(&request, &specs)?;
            cancel.check()?;
            let message = Message {
                role: Role::Assistant,
                content: reply.content,
                tool_calls: reply.tool_calls,
                ..Message::default()
            };
            events(ChatEvent::Message {
                message: message.clone(),
            });
            let calls = message.tool_calls.clone();
            messages.push(message);
            if calls.is_empty() {
                return Ok(());
            }
            for call in &calls {
                events(ChatEvent::ToolStart { call: call.clone() });
                // A cancelled call still gets an answer: a conversation with
                // an unanswered tool call is refused by most servers.
                let outcome = if cancel.is_cancelled() {
                    Err(Error::cancelled())
                } else {
                    self.run_tool(&tools, call, approve, cancel)
                };
                let (ok, result) = match outcome {
                    Ok(value) => (true, value),
                    Err(error) => (
                        false,
                        json!({ "error": error.code, "details": error.params }),
                    ),
                };
                events(ChatEvent::ToolEnd {
                    id: call.id.clone(),
                    ok,
                    result: result.clone(),
                });
                messages.push(Message {
                    role: Role::Tool,
                    content: clip(result.to_string()),
                    tool_call_id: call.id.clone(),
                    ..Message::default()
                });
            }
        }
        Err(Error::new("assistantLoop"))
    }
}

/// A stored conversation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Chat {
    pub id: String,
    pub title: String,
    /// Unix time of the last change, in seconds.
    pub updated: i64,
    pub messages: Vec<Message>,
}

impl Chat {
    pub fn new() -> Self {
        let updated = clock::unix_now();
        Self {
            id: format!("{updated:x}-{:08x}", fastrand::u32(..)),
            updated,
            ..Self::default()
        }
    }

    /// Names the conversation after its first user message.
    pub fn retitle(&mut self) {
        if !self.title.is_empty() {
            return;
        }
        if let Some(first) = self
            .messages
            .iter()
            .find(|message| message.role == Role::User)
        {
            let line = first.content.lines().next().unwrap_or_default().trim();
            self.title = line.chars().take(TITLE_LENGTH).collect();
        }
    }
}

/// The conversations of the assistant, newest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Chats {
    pub chats: Vec<Chat>,
}

impl Chats {
    pub fn load(paths: &Paths) -> Self {
        store::load_or_default(&paths.chats(), CHATS_VERSION, store::no_migration)
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        store::save_json(&paths.chats(), CHATS_VERSION, self)
    }

    pub fn get(&self, id: &str) -> Option<&Chat> {
        self.chats.iter().find(|chat| chat.id == id)
    }

    /// Stores a conversation and moves it to the top.
    pub fn upsert(&mut self, mut chat: Chat) -> Chat {
        chat.retitle();
        chat.updated = clock::unix_now();
        self.chats.retain(|known| known.id != chat.id);
        self.chats.insert(0, chat.clone());
        self.chats.truncate(MAX_CHATS);
        chat
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.chats.len();
        self.chats.retain(|chat| chat.id != id);
        self.chats.len() != before
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use crate::testing::{context, picture};

    use super::*;

    /// A model that answers from a script and remembers what it was sent.
    struct Script {
        replies: RefCell<VecDeque<Reply>>,
        seen: RefCell<Vec<Vec<Message>>>,
    }

    impl Script {
        fn new(replies: Vec<Reply>) -> Self {
            Self {
                replies: RefCell::new(replies.into()),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl Model for Script {
        fn reply(&self, messages: &[Message], _tools: &[ToolSpec]) -> Result<Reply> {
            self.seen.borrow_mut().push(messages.to_vec());
            self.replies
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| Error::new("badResponse"))
        }
    }

    fn asks(name: &str, arguments: Value) -> Reply {
        Reply {
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: String::from("call_1"),
                name: name.to_owned(),
                arguments,
            }],
        }
    }

    fn says(text: &str) -> Reply {
        Reply {
            content: text.to_owned(),
            tool_calls: Vec::new(),
        }
    }

    struct Guarded;

    impl ToolSource for Guarded {
        fn specs(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: String::from("remote__ping"),
                description: String::from("Ping"),
                parameters: json!({ "type": "object", "properties": {} }),
            }]
        }

        fn needs_approval(&self, _name: &str) -> bool {
            true
        }

        fn call(&self, _name: &str, _arguments: &Value, _cancel: &CancelToken) -> Result<Value> {
            Ok(json!({ "pong": true }))
        }
    }

    #[test]
    fn a_tool_is_called_and_its_result_returned_to_the_model() {
        let (_folder, context, allowed) = context();
        picture(&allowed.join("a.png"), 16, 16);
        let assistant = Assistant::new(&context, vec![Arc::new(Builtin(context.clone()))]);
        let model = Script::new(vec![
            asks("list_images", json!({ "folder": allowed })),
            says("One picture."),
        ]);
        let mut messages = vec![Message::new(Role::User, "What is in the folder?")];
        let mut events = Vec::new();
        assistant
            .answer(
                &model,
                &mut messages,
                &mut |event| events.push(event),
                &|_| true,
                &CancelToken::new(),
            )
            .unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2].role, Role::Tool);
        assert_eq!(messages[2].tool_call_id, "call_1");
        assert!(messages[2].content.contains("\"count\":1"));
        assert_eq!(messages[3].content, "One picture.");
        assert_eq!(events.len(), 4);
        assert!(matches!(&events[2], ChatEvent::ToolEnd { ok: true, .. }));
        let seen = model.seen.borrow();
        assert_eq!(seen[0][0].role, Role::System);
        assert_eq!(seen[1].len(), 4);
    }

    #[test]
    fn failures_and_refusals_are_told_to_the_model() {
        let (_folder, context, _allowed) = context();
        let assistant = Assistant::new(
            &context,
            vec![Arc::new(Builtin(context.clone())), Arc::new(Guarded)],
        );
        let model = Script::new(vec![
            asks("list_images", json!({ "folder": "C:\\Windows" })),
            asks("remote__ping", json!({})),
            asks("no_such_tool", json!({})),
            says("Done."),
        ]);
        let mut messages = vec![Message::new(Role::User, "Go")];
        assistant
            .answer(
                &model,
                &mut messages,
                &mut |_| {},
                &|_| false,
                &CancelToken::new(),
            )
            .unwrap();
        assert!(messages[2].content.contains("agentPathDenied"));
        assert!(messages[4].content.contains("toolDeclined"));
        assert!(messages[6].content.contains("unknownTool"));

        let model = Script::new(vec![asks("remote__ping", json!({})), says("Done.")]);
        let mut messages = vec![Message::new(Role::User, "Go")];
        assistant
            .answer(
                &model,
                &mut messages,
                &mut |_| {},
                &|_| true,
                &CancelToken::new(),
            )
            .unwrap();
        assert!(messages[2].content.contains("pong"));
    }

    #[test]
    fn endless_tool_calls_stop() {
        let (_folder, context, _allowed) = context();
        let assistant = Assistant::new(&context, vec![Arc::new(Builtin(context.clone()))]);
        let model = Script::new(vec![asks("list_recipes", json!({})); MAX_ROUNDS + 1]);
        let mut messages = vec![Message::new(Role::User, "Go")];
        let error = assistant
            .answer(
                &model,
                &mut messages,
                &mut |_| {},
                &|_| true,
                &CancelToken::new(),
            )
            .unwrap_err();
        assert_eq!(error.code, "assistantLoop");
        assert_eq!(messages.len(), 1 + MAX_ROUNDS * 2);
    }

    #[test]
    fn cancelling_stops_before_the_model_is_asked() {
        let (_folder, context, _allowed) = context();
        let assistant = Assistant::new(&context, Vec::new());
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut messages = vec![Message::new(Role::User, "Go")];
        let error = assistant
            .answer(
                &Script::new(Vec::new()),
                &mut messages,
                &mut |_| {},
                &|_| true,
                &cancel,
            )
            .unwrap_err();
        assert!(error.is_cancelled());
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn long_conversations_are_windowed_at_a_user_message() {
        let mut messages = Vec::new();
        for index in 0..50 {
            messages.push(Message::new(Role::User, format!("question {index}")));
            messages.push(Message::new(Role::Assistant, "answer"));
            messages.push(Message::new(Role::Tool, "result"));
        }
        let shown = window(&messages);
        assert!(shown.len() <= MAX_HISTORY);
        assert_eq!(shown[0].role, Role::User);
        assert_eq!(window(&messages[..10]).len(), 10);
    }

    #[test]
    fn results_are_clipped_on_a_character_boundary() {
        let clipped = clip("я".repeat(MAX_RESULT));
        assert!(clipped.ends_with("(truncated)"));
        assert!(clipped.len() < MAX_RESULT + 32);
        assert_eq!(clip(String::from("short")), "short");
    }

    #[test]
    fn the_prompt_states_the_rules() {
        let mut access = AgentAccess::default();
        let closed = system_prompt("ru", &access);
        assert!(closed.contains("No folder is open"));
        assert!(closed.contains("Russian"));
        assert!(closed.contains("cannot delete"));
        access.folders = vec![String::from("D:\\Photos")];
        access.allow_delete = true;
        let open = system_prompt("en", &access);
        assert!(open.contains("D:\\Photos"));
        assert!(!open.contains("cannot delete"));
    }

    #[test]
    fn chats_are_stored_newest_first() {
        let (_folder, context, _allowed) = context();
        let paths = &context.engine.paths;
        let mut chats = Chats::load(paths);
        let mut first = Chat::new();
        first
            .messages
            .push(Message::new(Role::User, "Сожми фото\nвторая строка"));
        let first = chats.upsert(first);
        assert_eq!(first.title, "Сожми фото");
        let second = chats.upsert(Chat::new());
        assert_ne!(first.id, second.id);
        chats.upsert(first.clone());
        chats.save(paths).unwrap();
        let loaded = Chats::load(paths);
        assert_eq!(loaded.chats.len(), 2);
        assert_eq!(loaded.chats[0].id, first.id);
        assert!(loaded.get(&second.id).is_some());
        let mut loaded = loaded;
        assert!(loaded.remove(&second.id));
        assert!(!loaded.remove(&second.id));
    }
}
