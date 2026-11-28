// Shared numbers and text of the studio: aspect ratios, the canvas grow
// and style text.

import type { Expand, GeneratedRecord, Generation, GenerationProvider, Style } from "../../types";

export const ASPECTS: ReadonlyArray<readonly [number, number]> = [
  [1, 1],
  [3, 2],
  [2, 3],
  [16, 9],
  [9, 16],
];

export const NO_EXPAND: Expand = { left: 0, right: 0, top: 0, bottom: 0 };

/** The picture a record was made from, when the two can be shown under a curtain. */
export function madeFrom(record: GeneratedRecord): string | undefined {
  if (record.source === "") {
    return undefined;
  }
  switch (record.kind) {
    case "edit":
    case "upscale":
    case "refine":
      return record.source;
    case "create":
    // A grown canvas has another shape, so the two would not lie over each other.
    case "outpaint":
      return undefined;
    default: {
      const unreachable: never = record.kind;
      return unreachable;
    }
  }
}

export function withModel(generation: Generation, provider: GenerationProvider, model: string): Generation {
  switch (provider) {
    case "none":
      return { ...generation, provider };
    case "comfy":
      return { ...generation, provider, comfyCheckpoint: model };
    case "forge":
      return { ...generation, provider, forgeCheckpoint: model };
    case "sdCpp":
      return { ...generation, provider, sdcppModel: model };
    case "cloud":
      return { ...generation, provider, cloud: { ...generation.cloud, model } };
    default: {
      const unreachable: never = provider;
      return unreachable;
    }
  }
}

function snap64(value: number): number {
  return Math.min(4096, Math.max(64, Math.round(value / 64) * 64));
}

/** A size of about the same area, with both sides a multiple of 64. */
export function fitRatio(width: number, height: number, ratioW: number, ratioH: number): [number, number] {
  const area = Math.min(4096, Math.max(64, width)) * Math.min(4096, Math.max(64, height));
  const ratio = ratioW / Math.max(1, ratioH);
  const nextHeight = Math.sqrt(area / ratio);
  return [snap64(nextHeight * ratio), snap64(nextHeight)];
}

function snap8(value: number): number {
  return Math.ceil(value / 8) * 8;
}

/** How far to grow a picture so it reaches a target shape. */
export function marginsForRatio(width: number, height: number, ratioW: number, ratioH: number): Expand {
  const safeW = Math.max(1, ratioW);
  const safeH = Math.max(1, ratioH);
  if (width * safeH >= height * safeW) {
    const extra = Math.max(0, snap8((width * safeH) / safeW) - height);
    return { left: 0, right: 0, top: Math.floor(extra / 2), bottom: extra - Math.floor(extra / 2) };
  }
  const extra = Math.max(0, snap8((height * safeW) / safeH) - width);
  return { left: Math.floor(extra / 2), right: extra - Math.floor(extra / 2), top: 0, bottom: 0 };
}

export function expandEmpty(expand: Expand): boolean {
  return expand.left === 0 && expand.right === 0 && expand.top === 0 && expand.bottom === 0;
}

function splitNegative(text: string): string[] {
  return text
    .split(",")
    .map((part) => part.trim())
    .filter((part) => part !== "");
}

/** The prompt and the negative prompt after the styles, as the generator receives them. */
export function applyStyles(prompt: string, negative: string, styles: readonly Style[]): [string, string] {
  let text = prompt.trim();
  const parts = splitNegative(negative);
  for (const style of styles) {
    const addition = style.prompt.trim();
    if (addition.includes("{prompt}")) {
      text = addition.replaceAll("{prompt}", text.trim());
    } else if (addition !== "" && text.trim() === "") {
      text = addition;
    } else if (addition !== "") {
      text = `${text.trim().replace(/,+$/, "")}, ${addition}`;
    }
    for (const part of splitNegative(style.negative)) {
      if (!parts.some((known) => known.toLowerCase() === part.toLowerCase())) {
        parts.push(part);
      }
    }
  }
  return [text, parts.join(", ")];
}
