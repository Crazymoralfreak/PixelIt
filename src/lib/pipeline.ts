// Defaults of pipeline steps and of the output settings. The values repeat
// the defaults of `pixelit-core`, so a step added here equals a step the
// core fills in by itself.

import type { Translator } from "../i18n";
import {
  OUTPUT_FORMATS,
  type ImageFormat,
  type Op,
  type OpKind,
  type OutputFormat,
  type OutputSpec,
  type Pipeline,
  type Recipe,
  type Shape,
  type ShapeKind,
  type Step,
  type WritableFormat,
} from "../types";
import { newId } from "./format";
import { NO_TUNING } from "./generation";

export const PIPELINE_VERSION = 1;

/** How far an "AI edit" may move from the picture, from 0 to 100. */
export const EDIT_STRENGTH = 75;

/** The same for an "AI detail" step: enough for new detail, too little to change what the picture shows. */
export const DETAIL_STRENGTH = 35;

/** Steps in the order the "add step" menu offers them. */
export const OP_KINDS: readonly OpKind[] = [
  "resize",
  "crop",
  "rotate",
  "flip",
  "adjust",
  "filter",
  "watermark",
  "annotate",
  "removeBackground",
  "upscale",
  "restoreFaces",
  "inpaint",
  "aiEdit",
  "aiDetail",
];

export const AI_KINDS: readonly OpKind[] = ["removeBackground", "upscale", "restoreFaces", "inpaint", "aiEdit", "aiDetail"];

/** Whether a step asks the image generator of the settings, which may be an outside service. */
export function usesGenerator(op: Op): boolean {
  // A detail step with nothing written still asks: it sends a short description of its own.
  if (op.type === "aiDetail") {
    return true;
  }
  return (op.type === "inpaint" || op.type === "aiEdit") && op.prompt.trim() !== "";
}

export function defaultOp(kind: OpKind): Op {
  switch (kind) {
    case "resize":
      return {
        type: "resize",
        mode: "fit",
        width: 1920,
        height: 1080,
        percent: 50,
        filter: "lanczos3",
        gammaCorrect: true,
        upscale: false,
      };
    case "crop":
      return {
        type: "crop",
        mode: "rect",
        x: 0,
        y: 0,
        width: 1,
        height: 1,
        aspectWidth: 1,
        aspectHeight: 1,
        anchor: "center",
      };
    case "rotate":
      return { type: "rotate", angle: 90, expand: true, background: "#00000000" };
    case "flip":
      return { type: "flip", horizontal: true, vertical: false };
    case "adjust":
      return {
        type: "adjust",
        exposure: 0,
        brightness: 0,
        contrast: 0,
        highlights: 0,
        shadows: 0,
        saturation: 0,
        vibrance: 0,
        temperature: 0,
        tint: 0,
        curve: [],
      };
    case "filter":
      return { type: "filter", kind: "sharpen", amount: 50, radius: 1 };
    case "watermark":
      return {
        type: "watermark",
        kind: "text",
        text: "PixelIt",
        font: "",
        size: 5,
        color: "#ffffff",
        image: "",
        scale: 20,
        anchor: "bottomRight",
        margin: 3,
        opacity: 60,
        rotation: 0,
        tile: false,
        tileGap: 60,
      };
    case "annotate":
      return { type: "annotate", shapes: [] };
    case "removeBackground":
      return { type: "removeBackground", model: "", background: null, keep: [] };
    case "upscale":
      return { type: "upscale", model: "", factor: 2 };
    case "restoreFaces":
      return { type: "restoreFaces", model: "", strength: 80 };
    case "inpaint":
      return { type: "inpaint", model: "", strokes: [], prompt: "" };
    case "aiEdit":
      return { type: "aiEdit", prompt: "", strokes: [], strength: EDIT_STRENGTH, variant: 0, provider: "", model: "" };
    case "aiDetail":
      return {
        type: "aiDetail",
        prompt: "",
        strokes: [],
        strength: DETAIL_STRENGTH,
        variant: 0,
        provider: "",
        model: "",
        negative: "",
        steps: 0,
        cfg: 0,
        tuning: NO_TUNING,
      };
    default: {
      const unreachable: never = kind;
      throw new Error(`Unknown step: ${String(unreachable)}`);
    }
  }
}

export function newStep(kind: OpKind): Step {
  return { id: newId(), enabled: true, op: defaultOp(kind) };
}

export function defaultShape(kind: ShapeKind, color: string): Shape {
  return {
    kind,
    x1: 0.25,
    y1: 0.25,
    x2: 0.75,
    y2: 0.75,
    color,
    width: kind === "highlight" ? 3 : 0.6,
    fill: false,
    text: "",
    font: "",
    size: 4,
    points: [],
  };
}

export function defaultOutput(): OutputSpec {
  return {
    format: "same",
    autoFormats: ["jpeg", "webp", "avif"],
    quality: 82,
    lossless: false,
    effort: "balanced",
    depth: "auto",
    subsampling: "auto",
    progressive: true,
    target: { kind: "none" },
    metadata: {
      mode: "keep",
      keepExif: true,
      keepGps: false,
      keepXmp: false,
      keepIcc: true,
      copyright: "",
      author: "",
    },
    color: "srgb",
    background: "#ffffff",
    naming: "{name}",
    destination: { kind: "subfolder", name: "PixelIt" },
    conflict: "rename",
    replaceOriginal: false,
    keepStructure: true,
  };
}

export function newPipeline(output: OutputSpec = defaultOutput()): Pipeline {
  return { version: PIPELINE_VERSION, steps: [], output };
}

/** Whether a run can replace or remove existing files, which is confirmed first. */
export function isDestructive(pipeline: Pipeline): boolean {
  return pipeline.output.conflict === "overwrite" || pipeline.output.replaceOriginal;
}

export function usesAi(pipeline: Pipeline): boolean {
  return pipeline.steps.some((step) => step.enabled && AI_KINDS.includes(step.op.type));
}

/** Names of formats; they are the same in every language. */
export const FORMAT_NAMES: Record<ImageFormat, string> = {
  jpeg: "JPEG",
  png: "PNG",
  webp: "WebP",
  avif: "AVIF",
  jxl: "JPEG XL",
  tiff: "TIFF",
  bmp: "BMP",
  gif: "GIF",
  ico: "ICO",
  heic: "HEIC",
  raw: "RAW",
  svg: "SVG",
  psd: "PSD",
  pdf: "PDF",
};

const LOSSY: readonly ImageFormat[] = ["jpeg", "webp", "avif", "jxl"];
const LOSSLESS_CAPABLE: readonly ImageFormat[] = ["webp", "avif", "jxl"];

/** Whether the quality slider means anything for this format choice. */
export function hasQuality(format: OutputFormat): boolean {
  return format === "same" || format === "auto" || LOSSY.includes(format);
}

export function canBeLossless(format: OutputFormat): boolean {
  return format !== "same" && format !== "auto" && LOSSLESS_CAPABLE.includes(format);
}

export function isWritable(format: ImageFormat): format is WritableFormat {
  return (OUTPUT_FORMATS as readonly ImageFormat[]).includes(format);
}

/** Name of a recipe as lists show it; built-in recipes are named by the translations. */
export function recipeName(recipe: Recipe, td: Translator["td"]): string {
  if (!recipe.builtin) {
    return recipe.name;
  }
  return td(`recipes.builtin.${recipe.id.replace(/^builtin\./, "")}`, undefined, recipe.id);
}

/** The usual extension of a format PixelIt writes. */
export function extensionOfFormat(format: WritableFormat): string {
  switch (format) {
    case "jpeg":
      return "jpg";
    case "tiff":
      return "tif";
    case "png":
    case "webp":
    case "avif":
    case "jxl":
    case "bmp":
    case "gif":
    case "ico":
      return format;
    default: {
      const unreachable: never = format;
      throw new Error(`Unknown format: ${String(unreachable)}`);
    }
  }
}

/** The output format a file extension asks for. */
export function formatOfExtension(extension: string): WritableFormat | null {
  switch (extension.toLowerCase()) {
    case "jpg":
    case "jpeg":
      return "jpeg";
    case "tif":
    case "tiff":
      return "tiff";
    case "png":
    case "webp":
    case "avif":
    case "jxl":
    case "bmp":
    case "gif":
    case "ico":
      return extension.toLowerCase() as WritableFormat;
    default:
      return null;
  }
}
