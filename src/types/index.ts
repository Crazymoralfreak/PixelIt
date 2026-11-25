// Shapes of the data exchanged with the Rust core. They mirror the serde
// models of the crates: `pixelit-core` (pipeline), `pixelit-engine`,
// `pixelit-ai`, `pixelit-tools`, `pixelit-mcp` and `src-tauri`.

export type Language = "ru" | "en" | "de" | "es" | "fr" | "pt" | "pl" | "tr" | "zh" | "ja" | "ko";

export const LANGUAGES: ReadonlyArray<{ code: Language; name: string }> = [
  { code: "en", name: "English" },
  { code: "ru", name: "Русский" },
  { code: "de", name: "Deutsch" },
  { code: "es", name: "Español" },
  { code: "fr", name: "Français" },
  { code: "pt", name: "Português" },
  { code: "pl", name: "Polski" },
  { code: "tr", name: "Türkçe" },
  { code: "zh", name: "中文" },
  { code: "ja", name: "日本語" },
  { code: "ko", name: "한국어" },
];

export interface AppError {
  code: string;
  params?: Record<string, string>;
}

/** `#rrggbb` or `#rrggbbaa`. */
export type Color = string;

export type ImageFormat =
  | "jpeg"
  | "png"
  | "webp"
  | "avif"
  | "jxl"
  | "tiff"
  | "bmp"
  | "gif"
  | "ico"
  | "heic"
  | "raw"
  | "svg"
  | "psd"
  | "pdf";

/** Formats PixelIt writes. */
export const OUTPUT_FORMATS = ["jpeg", "png", "webp", "avif", "jxl", "tiff", "bmp", "gif", "ico"] as const;
export type WritableFormat = (typeof OUTPUT_FORMATS)[number];

// Pipeline

export type Anchor =
  | "topLeft"
  | "top"
  | "topRight"
  | "left"
  | "center"
  | "right"
  | "bottomLeft"
  | "bottom"
  | "bottomRight";

export type ResizeMode = "fit" | "fill" | "exact" | "percent" | "longSide" | "shortSide" | "width" | "height";
export type ResizeFilter =
  | "nearest"
  | "box"
  | "bilinear"
  | "hamming"
  | "catmullRom"
  | "mitchell"
  | "gaussian"
  | "lanczos3";

export interface ResizeOp {
  type: "resize";
  mode: ResizeMode;
  width: number;
  height: number;
  percent: number;
  filter: ResizeFilter;
  gammaCorrect: boolean;
  upscale: boolean;
}

export interface CropOp {
  type: "crop";
  mode: "rect" | "aspect";
  x: number;
  y: number;
  width: number;
  height: number;
  aspectWidth: number;
  aspectHeight: number;
  anchor: Anchor;
}

export interface RotateOp {
  type: "rotate";
  angle: number;
  expand: boolean;
  background: Color;
}

export interface FlipOp {
  type: "flip";
  horizontal: boolean;
  vertical: boolean;
}

export interface AdjustOp {
  type: "adjust";
  exposure: number;
  brightness: number;
  contrast: number;
  highlights: number;
  shadows: number;
  saturation: number;
  vibrance: number;
  temperature: number;
  tint: number;
  curve: Array<[number, number]>;
}

export type FilterKind = "sharpen" | "blur" | "denoise" | "grain" | "vignette" | "grayscale" | "sepia" | "invert";

export interface FilterOp {
  type: "filter";
  kind: FilterKind;
  amount: number;
  radius: number;
}

export interface WatermarkOp {
  type: "watermark";
  kind: "text" | "image";
  text: string;
  font: string;
  size: number;
  color: Color;
  image: string;
  scale: number;
  anchor: Anchor;
  margin: number;
  opacity: number;
  rotation: number;
  tile: boolean;
  tileGap: number;
}

export type ShapeKind = "arrow" | "line" | "rect" | "ellipse" | "pen" | "text" | "highlight" | "blur" | "pixelate";

export interface Shape {
  kind: ShapeKind;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  color: Color;
  width: number;
  fill: boolean;
  text: string;
  font: string;
  size: number;
  points: Array<[number, number]>;
}

export interface AnnotateOp {
  type: "annotate";
  shapes: Shape[];
}

export interface RemoveBackgroundOp {
  type: "removeBackground";
  model: string;
  background: Color | null;
  /** Areas painted by hand that stay as they are. */
  keep: Stroke[];
}

export interface UpscaleOp {
  type: "upscale";
  model: string;
  factor: number;
}

export interface RestoreFacesOp {
  type: "restoreFaces";
  model: string;
  strength: number;
}

export interface Stroke {
  points: Array<[number, number]>;
  width: number;
}

export interface InpaintOp {
  type: "inpaint";
  model: string;
  strokes: Stroke[];
  prompt: string;
}

export interface AiEditOp {
  type: "aiEdit";
  prompt: string;
  /** Area to change; without strokes the whole picture is changed. */
  strokes: Stroke[];
  /** From 0 to 100. */
  strength: number;
  /** Number of the attempt: another number asks for another result. */
  variant: number;
}

export type Op =
  | ResizeOp
  | CropOp
  | RotateOp
  | FlipOp
  | AdjustOp
  | FilterOp
  | WatermarkOp
  | AnnotateOp
  | RemoveBackgroundOp
  | UpscaleOp
  | RestoreFacesOp
  | InpaintOp
  | AiEditOp;

export type OpKind = Op["type"];

export interface Step {
  id: string;
  enabled: boolean;
  op: Op;
}

export type OutputFormat = "same" | "auto" | WritableFormat;

export type Target = { kind: "none" } | { kind: "bytes"; bytes: number } | { kind: "quality"; score: number };

export interface MetadataPolicy {
  mode: "keep" | "strip" | "custom";
  keepExif: boolean;
  keepGps: boolean;
  keepXmp: boolean;
  keepIcc: boolean;
  copyright: string;
  author: string;
}

export type Destination =
  | { kind: "sameFolder" }
  | { kind: "subfolder"; name: string }
  | { kind: "folder"; path: string };

export type Conflict = "rename" | "overwrite" | "skip";

export interface OutputSpec {
  format: OutputFormat;
  autoFormats: ImageFormat[];
  quality: number;
  lossless: boolean;
  effort: "fast" | "balanced" | "max";
  depth: "auto" | "eight" | "sixteen";
  subsampling: "auto" | "full" | "half";
  progressive: boolean;
  target: Target;
  metadata: MetadataPolicy;
  color: "srgb" | "original";
  background: Color;
  naming: string;
  destination: Destination;
  conflict: Conflict;
  replaceOriginal: boolean;
  keepStructure: boolean;
}

export interface Pipeline {
  version: number;
  steps: Step[];
  output: OutputSpec;
}

// Engine

export interface AgentAccess {
  folders: string[];
  allowOverwrite: boolean;
  allowDelete: boolean;
  allowDownloads: boolean;
  allowCloud: boolean;
}

export interface Settings {
  /** Empty follows the operating system. */
  language: string;
  theme: "system" | "light" | "dark";
  threads: number;
  memoryLimitMb: number;
  output: OutputSpec;
  confirmOverwrite: boolean;
  confirmDelete: boolean;
  thumbCacheMb: number;
  closeToTray: boolean;
  startMinimized: boolean;
  autostart: boolean;
  contextMenu: boolean;
  checkUpdates: boolean;
  updateChannel: "stable" | "beta";
  seenVersion: string;
  /** Newer version the user chose to skip. */
  skippedVersion: string;
  captureHotkey: string;
  pdfiumFolder: string;
  /** Folder for pictures downloaded from the web; empty keeps them in the application data. */
  downloadFolder: string;
  agents: AgentAccess;
  recentFolders: string[];
}

export interface AppInfo {
  version: string;
  dataDir: string;
  portable: boolean;
  installed: boolean;
  systemLanguage: Language;
  language: Language;
}

export type UpdateState =
  | "idle"
  | "checking"
  | "upToDate"
  | "available"
  | "downloading"
  | "ready"
  | "installing"
  | "failed";

export interface UpdateStatus {
  state: UpdateState;
  version: string | null;
  notes: string | null;
  downloaded: number;
  total: number;
  portable: boolean;
}

export type JobState = "queued" | "running" | "paused" | "done" | "failed" | "cancelled";

export interface JobInfo {
  id: number;
  kind: string;
  title: string;
  lane: "cpu" | "gpu" | "io";
  state: JobState;
  done: number;
  total: number;
  current: string;
  etaSeconds: number | null;
  error: AppError | null;
  result: unknown;
}

export interface FileEntry {
  path: string;
  name: string;
  bytes: number;
  /** Milliseconds since 1970. */
  modified: number;
}

export interface FolderListing {
  folder: string;
  parent: string | null;
  folders: FileEntry[];
  files: FileEntry[];
}

export interface SourceInfo {
  format: ImageFormat | null;
  width: number;
  height: number;
  bits: number;
  hasAlpha: boolean;
  frames: number;
  orientation: number;
  hasIcc: boolean;
  hasExif: boolean;
  hasXmp: boolean;
}

export interface FileInfo {
  file: FileEntry;
  info: SourceInfo;
}

export interface EditorInfo {
  id: number;
  path: string;
  width: number;
  height: number;
  scale: number;
  info: SourceInfo;
}

export interface Histogram {
  red: number[];
  green: number[];
  blue: number[];
  luma: number[];
}

export interface RenderInfo {
  version: number;
  width: number;
  height: number;
  histogram: Histogram;
}

export interface Outcome {
  source: string;
  output: string;
  status: "done" | "skipped";
  format: ImageFormat;
  width: number;
  height: number;
  sourceBytes: number;
  outputBytes: number;
  quality: number | null;
  score: number | null;
  targetMissed: boolean;
  replaced: boolean;
  sourceTrashed: boolean;
}

export interface BatchItem {
  source: string;
  outcome: Outcome | null;
  error: AppError | null;
  elapsedMs: number;
}

export interface BatchReport {
  items: BatchItem[];
  done: number;
  skipped: number;
  failed: number;
  sourceBytes: number;
  outputBytes: number;
  elapsedMs: number;
  cancelled: boolean;
  folder: string;
  undoId: string | null;
}

export interface BatchStart {
  title: string;
  inputs: string[];
  recursive: boolean;
  pipeline: Pipeline;
  confirmed?: boolean;
}

export interface Estimate {
  bytes: number;
  sourceBytes: number;
  format: ImageFormat;
  width: number;
  height: number;
  quality: number | null;
  score: number | null;
  targetMissed: boolean;
}

export interface Comparison {
  format: ImageFormat;
  bytes: number;
  quality: number | null;
  score: number | null;
  elapsedMs: number;
  error: AppError | null;
}

export interface Recipe {
  id: string;
  name: string;
  description: string;
  pipeline: Pipeline;
  builtin: boolean;
  pinned: boolean;
  updated: number;
}

export interface WatchRule {
  id: string;
  name: string;
  folder: string;
  recursive: boolean;
  recipeId: string;
  enabled: boolean;
  settleMs: number;
}

export interface JournalEntry {
  id: string;
  /** Seconds since 1970. */
  time: number;
  kind: string;
  title: string;
  done: number;
  skipped: number;
  failed: number;
  sourceBytes: number;
  outputBytes: number;
  elapsedMs: number;
  cancelled: boolean;
  folder: string;
  undoId: string | null;
  errors: Array<{ path: string; error: AppError | null }>;
  /** For downloads: where every file came from. Absent for everything else. */
  origins?: JournalOrigin[];
}

/** Where a downloaded file came from and on what terms it may be used. */
export interface JournalOrigin {
  path: string;
  url: string;
  page: string;
  author: string;
  license: string;
  licenseUrl: string;
}

/** Catalogues of pictures that are searched without an account. */
export type WebSource = "openverse" | "commons";

export const WEB_SOURCES: readonly WebSource[] = ["openverse", "commons"];

/** A picture of the web with what is known about where it comes from. */
export interface WebImage {
  /** The address of the picture in its best size. */
  url: string;
  /** A smaller copy, when the source has one; otherwise the same address. */
  preview: string;
  title: string;
  /** The size in pixels; zero when the source does not tell it. */
  width: number;
  height: number;
  /** The page the picture was found on. */
  page: string;
  author: string;
  license: string;
  licenseUrl: string;
  /** The catalogue or the site the picture comes from. */
  source: string;
  /** The picture the page is about: its cover, or the pin itself. */
  main: boolean;
}

/** What a link leads to. */
export interface WebInspected {
  /** The address that answered, after redirects. */
  url: string;
  title: string;
  /** The link is a picture itself, not a page. */
  direct: boolean;
  images: WebImage[];
}

export interface WebSearchPage {
  images: WebImage[];
  /** Whether the next page has something. */
  more: boolean;
}

/** The result of a download job. */
export interface WebDownloaded {
  files: string[];
  failed: Array<{ url: string; error: AppError }>;
  folder: string;
}

export interface UndoReport {
  removed: number;
  restored: number;
  changed: number;
  missing: number;
}

export type IconPreset = "favicon" | "pwa" | "windows" | "android" | "ios" | "macos";

export interface IconRequest {
  source: string;
  folder: string;
  presets: IconPreset[];
  padding: number;
  background: Color | null;
  appName: string;
}

export interface Generated {
  folder: string;
  files: string[];
  snippet: string;
  blurhash: string;
}

export interface SocialSize {
  id: string;
  width: number;
  height: number;
}

export interface WebKitRequest {
  source: string;
  folder: string;
  widths: number[];
  formats: ImageFormat[];
  quality: number;
  alt: string;
  sizes: string;
  urlPrefix: string;
}

export interface CollageSpec {
  columns: number;
  cellWidth: number;
  cellHeight: number;
  gap: number;
  background: Color;
  fill: boolean;
}

export interface PdfSpec {
  dpi: number;
  quality: number;
  title: string;
}

export interface AnimationSpec {
  delayMs: number;
  loopCount: number;
  width: number;
  height: number;
}

export interface DuplicateFile {
  path: string;
  bytes: number;
  width: number;
  height: number;
  distance: number;
}

export interface DuplicateGroup {
  exact: boolean;
  files: DuplicateFile[];
}

// AI

export interface Endpoint {
  url: string;
  model: string;
}

export type GenerationProvider = "none" | "comfy" | "forge" | "sdCpp" | "cloud";

export interface Generation {
  provider: GenerationProvider;
  comfyUrl: string;
  comfyCheckpoint: string;
  comfyWorkflow: string;
  forgeUrl: string;
  sdcppModel: string;
  sdcppArguments: string;
  cloud: Endpoint;
  width: number;
  height: number;
  steps: number;
  cfg: number;
  negative: string;
}

export interface AiSettings {
  device: "auto" | "gpu" | "cpu";
  gpu: number;
  threads: number;
  models: { background: string; upscale: string; inpaint: string; faces: string };
  allowCloud: boolean;
  confirmCloud: boolean;
  vision: Endpoint;
  assistant: Endpoint;
  generation: Generation;
}

export type ComponentKind =
  | "runtime"
  | "pdf"
  | "generator"
  | "background"
  | "upscale"
  | "inpaint"
  | "faces"
  | "detector";

export interface ComponentStatus {
  id: string;
  kind: ComponentKind;
  name: string;
  license: string;
  source: string;
  bytes: number;
  installed: boolean;
  requires: string[];
}

export interface Capabilities {
  runtime: boolean;
  background: boolean;
  upscale: boolean;
  inpaint: boolean;
  faces: boolean;
  generation: boolean;
  vision: boolean;
  assistant: boolean;
  pdf: boolean;
}

export interface Gpu {
  index: number;
  name: string;
  memoryMb: number;
}

export interface Hardware {
  gpus: Gpu[];
  ramMb: number;
  threads: number;
}

export interface Advice {
  useGpu: boolean;
  gpu: number;
  generator: string;
  slowGeneration: boolean;
}

export interface AiStatus {
  capabilities: Capabilities;
  hardware: Hardware;
  advice: Advice;
  settings: AiSettings;
  components: ComponentStatus[];
  generationRemote: boolean;
  visionRemote: boolean;
}

export type ServerKind = "ollama" | "lmStudio" | "llamaCpp" | "comfy" | "forge";

export interface FoundServer {
  kind: ServerKind;
  url: string;
  models: string[];
}

export interface GenerateRequest {
  prompt: string;
  negative: string;
  width: number;
  height: number;
  steps: number;
  cfg: number;
  seed: number | null;
  count: number;
  strength: number;
}

export type VisionTask = "describe" | "altText" | "tags" | "name" | "ocr";

export interface VisionAnswer {
  path: string;
  text: string;
  applied: boolean;
}

export interface VisionResult {
  answers: Array<{ source: string; answer?: VisionAnswer; error?: AppError }>;
}

// Assistant and MCP

export interface ToolCall {
  id: string;
  name: string;
  arguments: unknown;
}

export interface ChatMessage {
  role: "system" | "user" | "assistant" | "tool";
  content: string;
  toolCalls: ToolCall[];
  toolCallId: string;
}

export interface Chat {
  id: string;
  title: string;
  updated: number;
  messages: ChatMessage[];
}

export interface ChatSummary {
  id: string;
  title: string;
  updated: number;
}

export type ChatEvent =
  | { type: "message"; message: ChatMessage }
  | { type: "toolStart"; call: ToolCall }
  | { type: "toolEnd"; id: string; ok: boolean; result: unknown };

export type Permission = "allow" | "ask" | "deny";

export interface ServerConfig {
  id: string;
  name: string;
  enabled: boolean;
  transport: "stdio" | "http";
  command: string;
  args: string[];
  env: Record<string, string>;
  url: string;
  defaultPermission: Permission;
  tools: Record<string, Permission>;
}

export interface ServerStatus {
  id: string;
  connected: boolean;
  server: string;
  error: AppError | null;
  tools: Array<{ name: string; description: string; permission: Permission }>;
}
