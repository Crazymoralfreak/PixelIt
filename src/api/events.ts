// Events emitted by the Rust core.

import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef } from "react";
import type { AppError, ChatEvent, JobInfo, Settings, ToolCall, UpdateStatus } from "../types";

export interface AppEvents {
  /** Every change of a job: queued, progress, finished. */
  "jobs:changed": JobInfo;
  "journal:changed": null;
  /** Settings changed in the core, for example by an assistant tool. */
  "settings:changed": Settings;
  /** Every step of a check for updates and of the installation of one. */
  "update:status": UpdateStatus;
  /** Components, models or AI settings changed. */
  "ai:changed": null;
  /** Connections to MCP servers or their permissions changed. */
  "mcp:changed": null;
  /** A second start of PixelIt passed these files. */
  "open:files": string[];
  /** A screenshot was taken by the hotkey or from the tray. */
  "capture:done": string;
  "chat:event": { chatId: string; event: ChatEvent };
  /** The assistant asks whether it may call a tool. */
  "chat:approval": { chatId: string; call: ToolCall };
  "chat:done": { chatId: string; error: AppError | null };
}

export type AppEventName = keyof AppEvents;

/** Subscribes for the lifetime of the component; the handler may change between renders. */
export function useAppEvent<K extends AppEventName>(name: K, handler: (payload: AppEvents[K]) => void): void {
  const latest = useRef(handler);
  latest.current = handler;

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen<AppEvents[K]>(name, (event) => latest.current(event.payload)).then((stop) => {
      if (cancelled) {
        stop();
      } else {
        unlisten = stop;
      }
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [name]);
}
