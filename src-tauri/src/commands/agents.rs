//! Commands of the assistant chat and of the connections to MCP servers.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::Emitter;

use pixelit_ai::{Message, Role, ToolCall};
use pixelit_core::{CancelToken, Error, Result};
use pixelit_mcp::{McpConfig, Permission, ServerConfig, ServerStatus};
use pixelit_tools::{Assistant, Builtin, Chat, ChatEvent, Chats, ToolContext, ToolSource};

use super::{display, CoreState};
use crate::app::{lock, Core};

/// How often a waiting approval looks whether the answer was cancelled.
const APPROVAL_POLL: Duration = Duration::from_millis(200);

// Conversations

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    pub updated: i64,
}

#[tauri::command(async)]
pub fn chats_list(core: CoreState<'_>) -> Vec<ChatSummary> {
    Chats::load(core.paths())
        .chats
        .into_iter()
        .map(|chat| ChatSummary {
            id: chat.id,
            title: chat.title,
            updated: chat.updated,
        })
        .collect()
}

#[tauri::command(async)]
pub fn chat_get(core: CoreState<'_>, id: String) -> Result<Chat> {
    Chats::load(core.paths())
        .get(&id)
        .cloned()
        .ok_or_else(|| Error::new("chatMissing"))
}

#[tauri::command(async)]
pub fn chat_remove(core: CoreState<'_>, id: String) -> Result<bool> {
    let mut chats = Chats::load(core.paths());
    let removed = chats.remove(&id);
    chats.save(core.paths())?;
    Ok(removed)
}

/// Whether an answer is being written right now.
#[tauri::command(async)]
pub fn chat_busy(core: CoreState<'_>) -> bool {
    core.chat.busy.load(Ordering::SeqCst)
}

/// Asks the user about a tool call and waits for the decision. A cancelled
/// answer counts as a refusal.
fn ask(core: &Core, chat_id: &str, call: &ToolCall, cancel: &CancelToken) -> bool {
    let (sender, receiver) = mpsc::channel();
    lock(&core.chat.approvals).insert(call.id.clone(), sender);
    let _ = core
        .app()
        .emit("chat:approval", json!({ "chatId": chat_id, "call": call }));
    let allowed = loop {
        match receiver.recv_timeout(APPROVAL_POLL) {
            Ok(allowed) => break allowed,
            Err(RecvTimeoutError::Timeout) if !cancel.is_cancelled() => {}
            Err(_) => break false,
        }
    };
    lock(&core.chat.approvals).remove(&call.id);
    allowed
}

/// Writes the answer to the last message of a conversation and stores it.
fn answer(core: &Arc<Core>, mut chat: Chat, cancel: &CancelToken) -> (Chat, Result<()>) {
    let context = ToolContext {
        cancel: cancel.clone(),
        language: core.lang(),
        ..ToolContext::new(Arc::clone(&core.engine), Arc::clone(&core.ai))
    };
    let builtin: Arc<dyn ToolSource> = Arc::new(Builtin(context.clone()));
    let hub: Arc<dyn ToolSource> = core.hub.clone();
    let assistant = Assistant::new(&context, vec![builtin, hub]);
    let chat_id = chat.id.clone();
    let mut events = |event: ChatEvent| {
        let _ = core
            .app()
            .emit("chat:event", json!({ "chatId": chat_id, "event": event }));
    };
    let approve = |call: &ToolCall| ask(core, &chat_id, call, cancel);
    let outcome = assistant.answer(
        core.ai.as_ref(),
        &mut chat.messages,
        &mut events,
        &approve,
        cancel,
    );
    (chat, outcome)
}

/// Adds a message of the user to a conversation (a new one without `id`) and
/// starts the answer in the background. Returns the id of the conversation;
/// the answer arrives as `chat:event` events and ends with `chat:done`.
#[tauri::command(async)]
pub fn chat_send(
    core: CoreState<'_>,
    id: Option<String>,
    text: String,
    files: Vec<String>,
) -> Result<String> {
    core.ensure_not_updating()?;
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::new("emptyPrompt"));
    }
    if !core.ai.settings().assistant.is_set() {
        return Err(Error::new("providerNotSet"));
    }
    if core.chat.busy.swap(true, Ordering::SeqCst) {
        return Err(Error::new("chatBusy"));
    }
    let mut chats = Chats::load(core.paths());
    let mut chat = id
        .and_then(|id| chats.get(&id).cloned())
        .unwrap_or_default();
    // Attached files are named in the message, so the model can pass them to tools.
    let mut content = text.to_owned();
    if !files.is_empty() {
        content.push_str("\n\nFiles:");
        for file in &files {
            content.push_str("\n- ");
            content.push_str(file);
        }
    }
    chat.messages.push(Message::new(Role::User, content));
    let chat = chats.upsert(chat);
    if let Err(error) = chats.save(core.paths()) {
        core.chat.busy.store(false, Ordering::SeqCst);
        return Err(error);
    }
    let cancel = CancelToken::new();
    *lock(&core.chat.cancel) = cancel.clone();
    let chat_id = chat.id.clone();
    let core = Arc::clone(core.inner());
    let worker = Arc::clone(&core);
    let spawned = thread::Builder::new()
        .name(String::from("pixelit-assistant"))
        .spawn(move || {
            let core = worker;
            let (chat, outcome) = answer(&core, chat, &cancel);
            let id = chat.id.clone();
            let mut chats = Chats::load(core.paths());
            chats.upsert(chat);
            let saved = chats.save(core.paths());
            core.chat.busy.store(false, Ordering::SeqCst);
            let error = outcome.and(saved).err();
            let _ = core
                .app()
                .emit("chat:done", json!({ "chatId": id, "error": error }));
        });
    if let Err(error) = spawned {
        core.chat.busy.store(false, Ordering::SeqCst);
        return Err(Error::from(error));
    }
    Ok(chat_id)
}

/// The decision of the user about a tool call the assistant asked for.
#[tauri::command(async)]
pub fn chat_approve(core: CoreState<'_>, call_id: String, allowed: bool) {
    if let Some(sender) = lock(&core.chat.approvals).remove(&call_id) {
        let _ = sender.send(allowed);
    }
}

/// Stops the answer in progress; open questions count as refused.
#[tauri::command(async)]
pub fn chat_cancel(core: CoreState<'_>) {
    lock(&core.chat.cancel).cancel();
    let waiting: HashMap<_, _> = std::mem::take(&mut *lock(&core.chat.approvals));
    for sender in waiting.into_values() {
        let _ = sender.send(false);
    }
}

// MCP servers PixelIt connects to

/// Starts or stops connections after a change of the configuration.
fn resync(core: &Arc<Core>) {
    let core = Arc::clone(core);
    let _ = thread::Builder::new()
        .name(String::from("pixelit-mcp"))
        .spawn(move || {
            core.hub
                .sync(core.ai.settings().allow_cloud, &CancelToken::new());
            let _ = core.app().emit("mcp:changed", ());
        });
}

#[tauri::command(async)]
pub fn mcp_status(core: CoreState<'_>) -> Vec<ServerStatus> {
    core.hub.status()
}

/// The configured servers with their environment variables, for editing.
#[tauri::command(async)]
pub fn mcp_servers(core: CoreState<'_>) -> Vec<ServerConfig> {
    McpConfig::load_with_secrets(core.paths()).servers
}

#[tauri::command(async)]
pub fn mcp_server_save(core: CoreState<'_>, server: ServerConfig) -> Result<ServerConfig> {
    let mut config = McpConfig::load_with_secrets(core.paths());
    let saved = config.upsert(server)?;
    config.save(core.paths())?;
    // The command or the address may have changed: connect anew.
    core.hub.disconnect(&saved.id);
    resync(core.inner());
    Ok(saved)
}

#[tauri::command(async)]
pub fn mcp_server_remove(core: CoreState<'_>, id: String) -> Result<bool> {
    let mut config = McpConfig::load_with_secrets(core.paths());
    let removed = config.remove(&id);
    config.save(core.paths())?;
    core.hub.disconnect(&id);
    resync(core.inner());
    Ok(removed)
}

/// Sets what the assistant may do with one tool of a server.
#[tauri::command(async)]
pub fn mcp_tool_permission(
    core: CoreState<'_>,
    id: String,
    tool: String,
    permission: Permission,
) -> Result<()> {
    let mut config = McpConfig::load_with_secrets(core.paths());
    let mut server = config
        .get(&id)
        .cloned()
        .ok_or_else(|| Error::new("mcpBadServer").with("name", &id))?;
    if permission == server.default_permission {
        server.tools.remove(&tool);
    } else {
        server.tools.insert(tool, permission);
    }
    config.upsert(server)?;
    config.save(core.paths())?;
    let _ = core.app().emit("mcp:changed", ());
    Ok(())
}

/// Drops a connection and makes it again.
#[tauri::command(async)]
pub fn mcp_reconnect(core: CoreState<'_>, id: String) {
    core.hub.disconnect(&id);
    resync(core.inner());
}

// PixelIt as an MCP server for other programs

/// The entry other MCP clients put into their configuration to use PixelIt.
#[tauri::command(async)]
pub fn mcp_snippet() -> Result<Value> {
    let exe = std::env::current_exe()?;
    Ok(json!({
        "mcpServers": {
            "pixelit": { "command": display(&exe), "args": ["mcp"] }
        }
    }))
}
