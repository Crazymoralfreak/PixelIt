// What differs between the systems PixelIt runs on: the key that starts a
// shortcut and the way shortcuts are written.

import type { Platform } from "../types";

let platform: Platform = "windows";

/** Called once, before the first screen is shown: the core tells which system this is. */
export function setPlatform(next: Platform): void {
  platform = next;
}

export function isMac(): boolean {
  return platform === "macos";
}

interface Modifiers {
  ctrlKey: boolean;
  metaKey: boolean;
}

/** Whether the key that starts a shortcut is held: Command on a Mac, Ctrl elsewhere. */
export function hasMod(event: Modifiers): boolean {
  return isMac() ? event.metaKey : event.ctrlKey;
}

/** Whether the key removes what is selected. A Mac keyboard has Backspace where others have Delete. */
export function isRemoveKey(event: { key: string }): boolean {
  return event.key === "Delete" || (isMac() && event.key === "Backspace");
}

/** Whether the key sends files to the trash: Delete, and Command+Backspace on a Mac, as in Finder. */
export function isTrashKey(event: { key: string; metaKey: boolean }): boolean {
  return event.key === "Delete" || (isMac() && event.metaKey && event.key === "Backspace");
}

/** Modifiers as a Mac writes them, in the order a Mac writes them. */
const MAC_MODIFIERS: readonly (readonly [string, string])[] = [
  ["Ctrl", "⌃"],
  ["Alt", "⌥"],
  ["Shift", "⇧"],
  ["Mod", "⌘"],
  ["Super", "⌘"],
];

/**
 * A shortcut as this system writes it. `Mod` is the key that starts
 * shortcuts: `Mod+Shift+S` reads `Ctrl+Shift+S`, and `⇧⌘S` on a Mac.
 */
export function keyLabel(shortcut: string): string {
  const parts = shortcut.split("+");
  if (!isMac()) {
    return parts.map((part) => (part === "Mod" ? "Ctrl" : part)).join("+");
  }
  const names = MAC_MODIFIERS.map(([name]) => name);
  const modifiers = MAC_MODIFIERS.filter(([name]) => parts.includes(name)).map(([, sign]) => sign);
  const keys = parts.filter((part) => !names.includes(part));
  return [...new Set(modifiers), ...keys].join("");
}
