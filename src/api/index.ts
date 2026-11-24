// Typed wrappers over the Rust commands. Every call rejects with an `AppError`.

import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  AiSettings,
  AiStatus,
  AnimationSpec,
  AppError,
  AppInfo,
  BatchStart,
  Chat,
  ChatSummary,
  CollageSpec,
  Comparison,
  EditorInfo,
  Estimate,
  FileEntry,
  FileInfo,
  FolderListing,
  FoundServer,
  GenerateRequest,
  Generated,
  IconRequest,
  ImageFormat,
  JobInfo,
  JournalEntry,
  PdfSpec,
  Permission,
  Pipeline,
  Recipe,
  RenderInfo,
  ServerConfig,
  ServerStatus,
  Settings,
  SocialSize,
  Step,
  UndoReport,
  UpdateStatus,
  VisionTask,
  WatchRule,
  WebKitRequest,
} from "../types";

export function isAppError(value: unknown): value is AppError {
  return typeof value === "object" && value !== null && typeof (value as AppError).code === "string";
}

/** Turns anything thrown by `invoke` into an `AppError`. */
export function toAppError(value: unknown): AppError {
  if (isAppError(value)) {
    return value;
  }
  return { code: "unknown", params: { message: String(value) } };
}

/** One transparent pixel: the drag plugin wants a picture for the cursor. */
const DRAG_ICON =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  appQuit: () => invoke<void>("app_quit"),
  windowShow: () => invoke<void>("window_show"),
  /** Files PixelIt was started with; handed out once. */
  openedFiles: () => invoke<string[]>("opened_files"),
  settingsGet: () => invoke<Settings>("settings_get"),
  /** Saves and applies settings; returns what was stored. */
  settingsSet: (settings: Settings) => invoke<Settings>("settings_set", { settings }),
  changelog: () => invoke<string>("changelog"),

  updateStatus: () => invoke<UpdateStatus>("update_status"),
  /** Starts a check for updates; the progress arrives as `update:status` events. */
  updateCheck: () => invoke<void>("update_check"),

  jobsList: () => invoke<JobInfo[]>("jobs_list"),
  jobCancel: (id: number) => invoke<void>("job_cancel", { id }),
  jobPause: (id: number, paused: boolean) => invoke<void>("job_pause", { id, paused }),
  jobsClear: () => invoke<void>("jobs_clear"),

  pickFiles: (multiple: boolean) => invoke<string[]>("pick_files", { multiple }),
  pickFile: (name: string, extensions: string[]) => invoke<string | null>("pick_file", { name, extensions }),
  pickFolder: (start?: string) => invoke<string | null>("pick_folder", { start: start ?? null }),
  pickSave: (fileName: string, name: string, extensions: string[]) =>
    invoke<string | null>("pick_save", { fileName, name, extensions }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  revealLogs: () => invoke<void>("reveal_logs"),
  cacheInfo: () => invoke<{ bytes: number }>("cache_info"),
  cacheClear: () => invoke<{ bytes: number }>("cache_clear"),
  fontsList: () => invoke<string[]>("fonts_list"),

  folderList: (folder: string) => invoke<FolderListing>("folder_list", { folder }),
  /** Pictures among files and folders, as a drop or a dialog gives them. */
  filesExpand: (inputs: string[], recursive: boolean) => invoke<FileEntry[]>("files_expand", { inputs, recursive }),
  fileInfo: (path: string) => invoke<FileInfo>("file_info", { path }),
  /** Moves files to the Recycle Bin. */
  filesTrash: (paths: string[]) => invoke<number>("files_trash", { paths }),

  editorOpen: (path: string, maxSide: number, page = 0) =>
    invoke<EditorInfo>("editor_open", { path, maxSide, page }),
  /** Rejects with `cancelled` when a newer render replaced this one. */
  editorRender: (id: number, steps: Step[], confirmed: boolean) =>
    invoke<RenderInfo>("editor_render", { id, steps, confirmed }),
  editorClose: (id: number) => invoke<void>("editor_close", { id }),
  /** Saves a picture from the clipboard (base64) and returns its path. */
  pasteImage: (data: string) => invoke<string>("paste_image", { data }),
  captureScreen: () => invoke<string>("capture_screen"),

  /** Queues a batch; the report is the result of the returned job. */
  batchStart: (request: BatchStart) => invoke<number>("batch_start", { request }),
  exportEstimate: (path: string, pipeline: Pipeline) => invoke<Estimate>("export_estimate", { path, pipeline }),
  compareFormats: (path: string, pipeline: Pipeline, formats: ImageFormat[]) =>
    invoke<Comparison[]>("compare_formats", { path, pipeline, formats }),

  recipesList: () => invoke<Recipe[]>("recipes_list"),
  recipeSave: (recipe: Recipe) => invoke<Recipe>("recipe_save", { recipe }),
  recipeRemove: (id: string) => invoke<boolean>("recipe_remove", { id }),
  recipeExport: (id: string, path: string) => invoke<void>("recipe_export", { id, path }),
  recipeImport: (path: string) => invoke<Recipe>("recipe_import", { path }),

  watchList: () => invoke<WatchRule[]>("watch_list"),
  watchSave: (rule: WatchRule) => invoke<WatchRule>("watch_save", { rule }),
  watchRemove: (id: string) => invoke<boolean>("watch_remove", { id }),

  journalList: () => invoke<JournalEntry[]>("journal_list"),
  journalClear: () => invoke<void>("journal_clear"),
  journalUndo: (undoId: string) => invoke<UndoReport>("journal_undo", { undoId }),

  iconsGenerate: (request: IconRequest) => invoke<Generated>("icons_generate", { request }),
  socialSizes: () => invoke<SocialSize[]>("social_sizes"),
  socialGenerate: (source: string, folder: string, ids: string[], quality: number) =>
    invoke<Generated>("social_generate", { source, folder, ids, quality }),
  webKitGenerate: (request: WebKitRequest) => invoke<Generated>("web_kit_generate", { request }),
  collageCreate: (files: string[], spec: CollageSpec, output: string, quality: number) =>
    invoke<string>("collage_create", { files, spec, output, quality }),
  pdfCreate: (files: string[], spec: PdfSpec, output: string) => invoke<string>("pdf_create", { files, spec, output }),
  animationCreate: (files: string[], spec: AnimationSpec, output: string, quality: number) =>
    invoke<string>("animation_create", { files, spec, output, quality }),
  /** The groups are the result of the returned job. */
  duplicatesFind: (folder: string, recursive: boolean, tolerance: number) =>
    invoke<number>("duplicates_find", { folder, recursive, tolerance }),

  aiStatus: () => invoke<AiStatus>("ai_status"),
  aiSettingsSet: (settings: AiSettings) => invoke<AiSettings>("ai_settings_set", { settings }),
  componentInstall: (id: string) => invoke<number>("component_install", { id }),
  componentRemove: (id: string) => invoke<void>("component_remove", { id }),
  aiKeySet: (host: string, key: string) => invoke<void>("ai_key_set", { host, key }),
  aiKeyHas: (host: string) => invoke<boolean>("ai_key_has", { host }),
  aiKeyDelete: (host: string) => invoke<void>("ai_key_delete", { host }),
  aiDiscover: () => invoke<FoundServer[]>("ai_discover"),
  aiModels: (url: string) => invoke<string[]>("ai_models", { url }),
  /** The saved files are the result of the returned job. */
  generateStart: (request: GenerateRequest, source: string | null, confirmed: boolean) =>
    invoke<number>("generate_start", { start: { request, source, confirmed } }),
  visionStart: (files: string[], task: VisionTask, apply: boolean, confirmed: boolean) =>
    invoke<number>("vision_start", { start: { files, task, apply, confirmed } }),

  chatsList: () => invoke<ChatSummary[]>("chats_list"),
  chatGet: (id: string) => invoke<Chat>("chat_get", { id }),
  chatRemove: (id: string) => invoke<boolean>("chat_remove", { id }),
  chatBusy: () => invoke<boolean>("chat_busy"),
  /** Returns the id of the conversation; the answer arrives as events. */
  chatSend: (id: string | null, text: string, files: string[]) => invoke<string>("chat_send", { id, text, files }),
  chatApprove: (callId: string, allowed: boolean) => invoke<void>("chat_approve", { callId, allowed }),
  chatCancel: () => invoke<void>("chat_cancel"),

  mcpStatus: () => invoke<ServerStatus[]>("mcp_status"),
  mcpServers: () => invoke<ServerConfig[]>("mcp_servers"),
  mcpServerSave: (server: ServerConfig) => invoke<ServerConfig>("mcp_server_save", { server }),
  mcpServerRemove: (id: string) => invoke<boolean>("mcp_server_remove", { id }),
  mcpToolPermission: (id: string, tool: string, permission: Permission) =>
    invoke<void>("mcp_tool_permission", { id, tool, permission }),
  mcpReconnect: (id: string) => invoke<void>("mcp_reconnect", { id }),
  mcpSnippet: () => invoke<unknown>("mcp_snippet"),

  /** Starts dragging files out of the window, into Explorer or another program. */
  dragFiles: (paths: string[]) =>
    invoke<void>("plugin:drag|start_drag", { item: paths, image: DRAG_ICON, onEvent: new Channel<unknown>() }),
};
