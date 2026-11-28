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
