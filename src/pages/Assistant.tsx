// The assistant: a conversation with a model that works through the tools
// of PixelIt and of the connected MCP servers.

import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../api";
import { useAppEvent } from "../api/events";
import { useApp } from "../app/context";
import { useT } from "../i18n";
import { fileName } from "../lib/format";
import type { ChatMessage, ChatSummary, ToolCall } from "../types";
import { Button, cx, EmptyState, Modal, PageFrame, Tag, TextArea } from "../ui";

type ToolState = "running" | "ok" | "failed";

function pretty(value: unknown): string {
  if (typeof value === "string") {
    return value;
  }
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function MessageView({ message, tools }: { message: ChatMessage; tools: Record<string, ToolState> }) {
  const { t, td } = useT();
  switch (message.role) {
    case "system":
      return null;
    case "user":
      return <div className="message message-user">{message.content}</div>;
    case "assistant":
      return (
        <div className="message message-assistant">
          {message.content ? <div className="message-text">{message.content}</div> : null}
          {message.toolCalls.map((call) => {
            const state = tools[call.id];
            return (
              <details key={call.id} className="tool-call">
                <summary>
                  {t("assistant.toolCall", { name: call.name })}
                  {state ? (
                    <Tag tone={state === "failed" ? "danger" : state === "ok" ? "ok" : "plain"}>
                      {td(`assistant.toolState.${state}`)}
                    </Tag>
                  ) : null}
                </summary>
                <pre className="snippet">{pretty(call.arguments)}</pre>
              </details>
            );
          })}
        </div>
      );
    case "tool":
      return (
        <details className="message message-tool">
          <summary>{t("assistant.toolResult")}</summary>
          <pre className="snippet">{message.content}</pre>
        </details>
      );
    default: {
      const unreachable: never = message.role;
      return unreachable;
    }
  }
}

export default function AssistantPage({ active }: { active: boolean }) {
  const { t, language } = useT();
  const { ai, queue, navigate, confirm, showError } = useApp();
  const [chats, setChats] = useState<ChatSummary[]>([]);
  const [chatId, setChatId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [tools, setTools] = useState<Record<string, ToolState>>({});
  const [text, setText] = useState("");
  const [files, setFiles] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [approval, setApproval] = useState<ToolCall | null>(null);
  const bottom = useRef<HTMLDivElement | null>(null);
  const chatRef = useRef(chatId);
  chatRef.current = chatId;

  const refreshChats = useCallback(() => {
    api.chatsList().then(setChats).catch(showError);
  }, [showError]);

  useEffect(() => {
    if (active) {
      refreshChats();
      api.chatBusy().then(setBusy).catch(showError);
    }
  }, [active, refreshChats, showError]);

  useEffect(() => {
    bottom.current?.scrollIntoView({ block: "end" });
  }, [messages, busy]);

  const openChat = useCallback(
    (id: string | null) => {
      setChatId(id);
      setTools({});
      if (id === null) {
        setMessages([]);
        return;
      }
      api
        .chatGet(id)
        .then((chat) => setMessages(chat.messages))
        .catch(showError);
    },
    [showError],
  );

  useAppEvent("chat:event", ({ chatId: id, event }) => {
    if (id !== chatRef.current) {
      return;
    }
    switch (event.type) {
      case "message":
        setMessages((current) => [...current, event.message]);
        break;
      case "toolStart":
        setTools((current) => ({ ...current, [event.call.id]: "running" }));
        break;
      case "toolEnd":
        setTools((current) => ({ ...current, [event.id]: event.ok ? "ok" : "failed" }));
        break;
      default: {
        const unreachable: never = event;
        throw new Error(`Unknown chat event: ${String(unreachable)}`);
      }
    }
  });

  useAppEvent("chat:approval", ({ call }) => setApproval(call));

  useAppEvent("chat:done", ({ chatId: id, error }) => {
    setBusy(false);
    setApproval(null);
    refreshChats();
    if (error) {
      showError(error);
    }
    if (id === chatRef.current) {
      // The stored conversation is the truth after an answer.
      api
        .chatGet(id)
        .then((chat) => setMessages(chat.messages))
        .catch(() => undefined);
    }
  });

  const send = async () => {
    const message = text.trim();
    if (message === "" || busy) {
      return;
    }
    setBusy(true);
    try {
      const id = await api.chatSend(chatId, message, files);
      const shown = files.length > 0 ? `${message}\n\n${files.map((file) => `· ${fileName(file)}`).join("\n")}` : message;
      setMessages((current) => [...current, { role: "user", content: shown, toolCalls: [], toolCallId: "" }]);
      setChatId(id);
      setText("");
      setFiles([]);
      refreshChats();
    } catch (error) {
      setBusy(false);
      showError(error);
    }
  };

  const decide = (allowed: boolean) => {
    if (approval) {
      api.chatApprove(approval.id, allowed).catch(showError);
      setApproval(null);
    }
  };

  const remove = async (chat: ChatSummary) => {
    const agreed = await confirm({
      title: t("assistant.removeTitle"),
      text: t("assistant.removeText", { name: chat.title }),
      confirmLabel: t("common.delete"),
      danger: true,
    });
    if (!agreed) {
      return;
    }
    try {
      await api.chatRemove(chat.id);
      if (chat.id === chatId) {
        openChat(null);
      }
      refreshChats();
    } catch (error) {
      showError(error);
    }
  };

  const attach = async () => {
    const chosen = await api.pickFiles(true);
    setFiles((current) => [...current, ...chosen.filter((file) => !current.includes(file))]);
  };

  if (ai && !ai.capabilities.assistant) {
    return (
      <PageFrame title={t("nav.assistant")}>
        <EmptyState
          title={t("assistant.noModelTitle")}
          text={t("assistant.noModelText")}
          action={
            <Button variant="primary" onClick={() => navigate("ai")}>
              {t("studio.setUp")}
            </Button>
          }
        />
      </PageFrame>
    );
  }

  return (
    <PageFrame
      title={t("nav.assistant")}
      actions={
        <>
          {ai ? <Tag tone="plain">{ai.settings.assistant.model}</Tag> : null}
          <Button disabled={busy} onClick={() => openChat(null)}>
            {t("assistant.newChat")}
          </Button>
        </>
      }
    >
      <div className="chat">
        <div className="chat-list">
          {chats.map((chat) => (
            <div key={chat.id} className={cx("chat-item", chat.id === chatId && "chat-item-active")}>
              <button type="button" className="chat-item-title" disabled={busy} onClick={() => openChat(chat.id)}>
                {chat.title || t("assistant.untitled")}
                <span className="chat-item-time">{new Date(chat.updated * 1000).toLocaleDateString(language)}</span>
              </button>
              <Button small variant="ghost" disabled={busy} onClick={() => void remove(chat)}>
                ✕
              </Button>
            </div>
          ))}
        </div>
        <div className="chat-main">
          <div className="chat-messages">
            {messages.length === 0 ? (
              <EmptyState title={t("assistant.emptyTitle")} text={t("assistant.emptyText")} />
            ) : (
              messages.map((message, index) => <MessageView key={index} message={message} tools={tools} />)
            )}
            {busy ? <div className="message message-wait">{t("assistant.thinking")}</div> : null}
            <div ref={bottom} />
          </div>
          {files.length > 0 ? (
            <div className="chat-files">
              {files.map((file) => (
                <button
                  key={file}
                  type="button"
                  className="folder-chip"
                  title={file}
                  onClick={() => setFiles(files.filter((item) => item !== file))}
                >
                  {fileName(file)} ✕
                </button>
              ))}
            </div>
          ) : null}
          <div className="chat-input">
            <TextArea
              value={text}
              onChange={setText}
              rows={3}
              placeholder={t("assistant.placeholder")}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.shiftKey) {
                  event.preventDefault();
                  void send();
                }
              }}
            />
            <div className="chat-buttons">
              <Button small onClick={() => void attach()}>
                {t("assistant.attach")}
              </Button>
              <Button
                small
                disabled={queue.length === 0}
                onClick={() => setFiles((current) => [...current, ...queue.filter((item) => !current.includes(item))])}
              >
                {t("tools.fromQueue")}
              </Button>
              {busy ? (
                <Button variant="danger" onClick={() => api.chatCancel().catch(showError)}>
                  {t("assistant.stop")}
                </Button>
              ) : (
                <Button variant="primary" disabled={text.trim() === ""} onClick={() => void send()}>
                  {t("assistant.send")}
                </Button>
              )}
            </div>
          </div>
        </div>
      </div>
      {approval ? (
        <Modal
          title={t("assistant.approveTitle")}
          onClose={() => decide(false)}
          footer={
            <>
              <Button onClick={() => decide(false)}>{t("assistant.deny")}</Button>
              <Button variant="primary" onClick={() => decide(true)}>
                {t("assistant.allow")}
              </Button>
            </>
          }
        >
          <div className="modal-text">{t("assistant.approveText", { name: approval.name })}</div>
          <pre className="snippet">{pretty(approval.arguments)}</pre>
        </Modal>
      ) : null}
    </PageFrame>
  );
}
