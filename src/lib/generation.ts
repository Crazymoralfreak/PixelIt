// What the studio and the "AI detail" step share about an image generator:
// its tuning, the model it uses and the key a model's settings are kept under.

import type { Generation, GenerationProvider, Tuning } from "../types";

/** The generator's own sampler, schedule and VAE, no LoRA and no embedding. */
export const NO_TUNING: Tuning = { sampler: "", scheduler: "", vae: "", loras: [], embeddings: [] };

export function tuningEmpty(tuning: Tuning): boolean {
  return (
    tuning.sampler === "" &&
    tuning.scheduler === "" &&
    tuning.vae === "" &&
    tuning.loras.length === 0 &&
    tuning.embeddings.length === 0
  );
}

/** Whether a generator takes a sampler, a VAE, LoRA files and embeddings from the form. */
export function takesTuning(provider: GenerationProvider): boolean {
  switch (provider) {
    case "comfy":
    case "forge":
      return true;
    case "none":
    case "sdCpp":
    case "cloud":
      return false;
    default: {
      const unreachable: never = provider;
      return unreachable;
    }
  }
}

/** The file of a model without its folder and extension. */
export function modelTitle(name: string): string {
  const file = name.split(/[\\/]/).pop() ?? name;
  return file.replace(/\.(safetensors|ckpt|pt|pth|sft)$/i, "");
}

/** Marks that end a word of a prompt: lists, weights and groups. */
const PROMPT_MARKS = /[\s,()[\]{}:|<>]+/;

/** A weight the way the generator writes it: `1`, `0.8`, `1.25`. */
function trimWeight(value: number): string {
  return value.toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
}

/** The file of a listed name, without the folder it lies in. */
function fileOf(name: string): string {
  return name.split(/[\\/]/).pop() ?? name;
}

/** Whether `word` names the file a generator lists as `name`. */
function isNamed(name: string, word: string): boolean {
  const wanted = word.toLowerCase();
  return fileOf(name).toLowerCase() === wanted || name.toLowerCase() === wanted;
}

/** Whether the text already names this file as a word of its own. */
function alreadyNamed(text: string, name: string): boolean {
  return text.split(PROMPT_MARKS).some((word) => word !== "" && isNamed(name, word));
}

function appendWord(text: string, word: string): string {
  const trimmed = text.replace(/[\s,]+$/, "");
  return trimmed === "" ? word : `${trimmed}, ${word}`;
}

/**
 * The prompt and the negative prompt as the final-prompt line shows them.
 * ComfyUI and Forge name the chosen embeddings in the text they read, and a
 * LoRA is shown as `<lora:name:weight>`. A generator that takes none of this
 * is left as it is. Nothing here is sent: the generator is given the tuning
 * itself.
 */
export function shownRequest(prompt: string, negative: string, tuning: Tuning, provider: GenerationProvider): [string, string] {
  const syntax = embeddingSyntax(provider);
  if (syntax === "plain") {
    return [prompt, negative];
  }
  let text = prompt;
  let avoided = negative;
  for (const embedding of tuning.embeddings) {
    const target = embedding.negative ? avoided : text;
    if (embedding.name.trim() === "" || alreadyNamed(target, embedding.name)) {
      continue;
    }
    const word = embeddingWord(embedding.name, embedding.weight, syntax === "comfy");
    if (embedding.negative) {
      avoided = appendWord(avoided, word);
    } else {
      text = appendWord(text, word);
    }
  }
  for (const lora of tuning.loras) {
    const name = modelTitle(lora.name);
    if (name === "" || text.toLowerCase().includes(`<lora:${name.toLowerCase()}:`)) {
      continue;
    }
    const tag = `<lora:${name}:${trimWeight(lora.weight)}>`;
    text = text.trimEnd() === "" ? tag : `${text.trimEnd()} ${tag}`;
  }
  return [text, avoided];
}

/** How a generator names an embedding. `plain` means the generator takes none. */
function embeddingSyntax(provider: GenerationProvider): "comfy" | "forge" | "plain" {
  switch (provider) {
    case "comfy":
      return "comfy";
    case "forge":
      return "forge";
    case "none":
    case "sdCpp":
    case "cloud":
      return "plain";
    default: {
      const unreachable: never = provider;
      return unreachable;
    }
  }
}

/** An embedding as a word of the prompt. A weight of 1 is a plain word. */
function embeddingWord(name: string, weight: number, comfy: boolean): string {
  const body = comfy ? `embedding:${name}` : fileOf(name);
  const strength = trimWeight(weight);
  return strength === "1" ? body : `(${body}:${strength})`;
}

const SEPARATOR = "\u0001";

export function defaultsKey(provider: GenerationProvider, model: string): string {
  return `${provider}${SEPARATOR}${model.trim()}`;
}

export function modelOf(generation: Generation): string {
  switch (generation.provider) {
    case "none":
      return "";
    case "comfy":
      return generation.comfyCheckpoint;
    case "forge":
      return generation.forgeCheckpoint;
    case "sdCpp":
      return generation.sdcppModel;
    case "cloud":
      return generation.cloud.model;
    default: {
      const unreachable: never = generation.provider;
      return unreachable;
    }
  }
}

/** The tuning last used with a model, when there is one. */
export function tuningOf(generation: Generation, provider: GenerationProvider, model: string): Tuning | undefined {
  // A setting saved before tuning existed has none.
  const kept: Tuning | undefined = generation.modelDefaults[defaultsKey(provider, model)]?.tuning;
  return kept;
}
