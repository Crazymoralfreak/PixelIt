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
