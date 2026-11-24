// Addresses of pictures served by the `pixelit://` protocol of the core.
// On Windows a custom protocol is reached as `http://<scheme>.localhost`.

import type { FileEntry } from "../types";

const BASE = "http://pixelit.localhost";

/** A cached thumbnail. The change time makes a new address for a changed file. */
export function thumbUrl(file: Pick<FileEntry, "path" | "modified">, size: number): string {
  return `${BASE}/thumb?path=${encodeURIComponent(file.path)}&size=${size}&m=${file.modified}`;
}

/** A file reduced for the screen. */
export function previewUrl(file: Pick<FileEntry, "path" | "modified">, size: number, page = 0): string {
  return `${BASE}/preview?path=${encodeURIComponent(file.path)}&size=${size}&page=${page}&m=${file.modified}`;
}

/** The picture of an editor session before any step. */
export function editorBaseUrl(id: number): string {
  return `${BASE}/editor/${id}/base`;
}

/** A render of an editor session; `version` comes from the render call. */
export function editorRenderUrl(id: number, version: number): string {
  return `${BASE}/editor/${id}/render?v=${version}`;
}
