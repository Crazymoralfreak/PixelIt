// Pictures of the clipboard, as base64 for the core.

export function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ""));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
}

/** The picture of a paste event, when there is one. */
export function pastedPicture(data: DataTransfer | null): File | null {
  return Array.from(data?.files ?? []).find((file) => file.type.startsWith("image/")) ?? null;
}

/** Longest address taken from the clipboard. */
const MAX_LINK = 2048;

/** The web address of a paste event: the pasted text, when it is one address and nothing else. */
export function pastedLink(data: DataTransfer | null): string | null {
  const text = data?.getData("text/plain").trim() ?? "";
  return text.length <= MAX_LINK && /^https?:\/\/\S+$/i.test(text) ? text : null;
}

/** Whether a paste lands in a field that takes text itself. */
export function isTextTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName))
  );
}

/** The picture the clipboard holds right now; `null` without one or without access. */
export async function readClipboardPicture(): Promise<Blob | null> {
  try {
    for (const item of await navigator.clipboard.read()) {
      const type = item.types.find((name) => name.startsWith("image/"));
      if (type) {
        return await item.getType(type);
      }
    }
  } catch {
    // The web view refused access; Ctrl+V still works.
  }
  return null;
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
