// What every screen can reach: application facts, settings, jobs, shared
// selections and the dialogs of the shell.

import { createContext, useContext } from "react";
import type { AiStatus, AppInfo, JobInfo, Pipeline, Recipe, Settings, UpdateStatus } from "../types";

export type Page =
  | "viewer"
  | "editor"
  | "batch"
  | "recipes"
  | "tools"
  | "studio"
  | "web"
  | "assistant"
  | "journal"
  | "ai"
  | "settings"
  | "about";

export const PAGES: readonly Page[] = [
  "viewer",
  "editor",
  "batch",
  "recipes",
  "tools",
  "studio",
  "web",
  "assistant",
  "journal",
  "ai",
  "settings",
  "about",
];

export interface ConfirmOptions {
  title: string;
  text: string;
  confirmLabel?: string;
  danger?: boolean;
}

export interface AskNameOptions {
  title: string;
  label: string;
  initial?: string;
  confirmLabel?: string;
}

/** Files and folders handed to the viewer from outside: a drop, a dialog, a second start. */
export interface Incoming {
  paths: string[];
  /** Grows with every hand-over, so the same paths can arrive twice. */
  token: number;
}

/** A web address handed to the "From the web" page: a paste outside any field. */
export interface IncomingLink {
  url: string;
  /** Grows with every hand-over, so the same address can arrive twice. */
  token: number;
}

/** One file dropped onto the studio. */
export interface IncomingFile {
  path: string;
  token: number;
}

export interface AppContextValue {
  info: AppInfo;
  settings: Settings;
  /** Saves and applies settings; shows the error and resolves to `false` when the core refuses. */
  saveSettings: (next: Settings) => Promise<boolean>;
  update: UpdateStatus;

  page: Page;
  navigate: (page: Page) => void;

  jobs: JobInfo[];
  /** Resolves when the job has finished, whatever its end. */
  waitJob: (id: number) => Promise<JobInfo>;
  /** The result of a finished job; rejects with the error of a failed or cancelled one. */
  jobResult: <T>(id: number) => Promise<T>;

  /** Models, components and providers; `null` until the first answer of the core. */
  ai: AiStatus | null;
  refreshAi: () => void;
  recipes: Recipe[];
  refreshRecipes: () => void;

  /** Files and folders waiting for a batch. */
  queue: string[];
  setQueue: (inputs: string[]) => void;
  addToQueue: (inputs: string[]) => void;
  /** Steps and output settings of the batch page. */
  batchPipeline: Pipeline;
  setBatchPipeline: (pipeline: Pipeline) => void;

  incoming: Incoming | null;
  /** Shows files or a folder in the viewer. */
  openPaths: (paths: string[]) => void;
  /** The file the editor works on. */
  editorPath: string | null;
  openInEditor: (path: string) => void;
  webLink: IncomingLink | null;
  /** Opens the "From the web" page with the address filled in; nothing is requested yet. */
  openLink: (url: string) => void;
  /** A file dropped on the window while the studio is open. */
  studioDrop: IncomingFile | null;

  confirm: (options: ConfirmOptions) => Promise<boolean>;
  /** Asks for a name; resolves to `null` when the dialog is dismissed. */
  askName: (options: AskNameOptions) => Promise<string | null>;
  notify: (text: string) => void;
  /** Shows an error of a command; a cancelled operation stays silent. */
  showError: (error: unknown) => void;
  /**
   * Runs a call that may send pictures to a cloud provider. When the core asks
   * for consent, shows the question and repeats the call with `confirmed`.
   */
  withCloud: <T>(call: (confirmed: boolean) => Promise<T>) => Promise<T>;
}

export const AppContext = createContext<AppContextValue | null>(null);

export function useApp(): AppContextValue {
  const value = useContext(AppContext);
  if (!value) {
    throw new Error("useApp is used outside of the application shell");
  }
  return value;
}
