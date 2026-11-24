// Numbers, sizes and paths as the interface shows them.

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** `1536` becomes `1.5 KB`. */
export function formatBytes(bytes: number): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

/** Change of size in percent: `-37%` for a file that became smaller. */
export function formatSaving(before: number, after: number): string {
  if (before <= 0) {
    return "";
  }
  const change = Math.round(((after - before) / before) * 100);
  return `${change > 0 ? "+" : ""}${change}%`;
}

/** `75` seconds become `1:15`. */
export function formatDuration(seconds: number): string {
  const whole = Math.max(0, Math.round(seconds));
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor((whole % 3600) / 60);
  const rest = String(whole % 60).padStart(2, "0");
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${rest}` : `${minutes}:${rest}`;
}

/** Date and time in the format of the interface language. */
export function formatTime(seconds: number, language: string): string {
  return new Date(seconds * 1000).toLocaleString(language, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function fileName(path: string): string {
  const cut = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return cut < 0 ? path : path.slice(cut + 1);
}

export function folderOf(path: string): string {
  const cut = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return cut < 0 ? "" : path.slice(0, cut);
}

/** The file name without its extension. */
export function stemOf(path: string): string {
  const name = fileName(path);
  const dot = name.lastIndexOf(".");
  return dot <= 0 ? name : name.slice(0, dot);
}

export function extensionOf(path: string): string {
  const name = fileName(path);
  const dot = name.lastIndexOf(".");
  return dot <= 0 ? "" : name.slice(dot + 1).toLowerCase();
}

export function joinPath(folder: string, name: string): string {
  return folder.endsWith("\\") || folder.endsWith("/") ? `${folder}${name}` : `${folder}\\${name}`;
}

/** A short random id for steps and shapes. */
export function newId(): string {
  return Math.random().toString(36).slice(2, 10);
}
