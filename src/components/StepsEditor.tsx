// The stack of pipeline steps with a form for each operation. Shared by the
// editor, the batch page and the recipe dialog.

import { useEffect, useState, type ReactNode } from "react";
import { api } from "../api";
import { useApp } from "../app/context";
import { useT } from "../i18n";
import { AI_KINDS, newStep, OP_KINDS } from "../lib/pipeline";
import type {
  AdjustOp,
  Anchor,
  ComponentKind,
  CropOp,
  FilterKind,
  FilterOp,
  Op,
  OpKind,
  ResizeFilter,
  ResizeMode,
  ResizeOp,
  RotateOp,
  Step,
  WatermarkOp,
} from "../types";
import { Button, Checkbox, ColorInput, cx, Field, NumberInput, Segmented, Select, Slider, TextInput } from "../ui";
import { PathField } from "./PathField";

const RESIZE_MODES: readonly ResizeMode[] = [
  "fit",
  "fill",
  "exact",
  "percent",
  "longSide",
  "shortSide",
  "width",
  "height",
];
const RESIZE_FILTERS: ReadonlyArray<{ value: ResizeFilter; label: string }> = [
  { value: "lanczos3", label: "Lanczos" },
  { value: "catmullRom", label: "Catmull-Rom" },
  { value: "mitchell", label: "Mitchell" },
  { value: "gaussian", label: "Gaussian" },
  { value: "hamming", label: "Hamming" },
  { value: "bilinear", label: "Bilinear" },
  { value: "box", label: "Box" },
  { value: "nearest", label: "Nearest" },
];
const ANCHORS: readonly Anchor[] = [
  "topLeft",
  "top",
  "topRight",
  "left",
  "center",
  "right",
  "bottomLeft",
  "bottom",
  "bottomRight",
];
const FILTER_KINDS: readonly FilterKind[] = [
  "sharpen",
  "blur",
  "denoise",
  "grain",
  "vignette",
  "grayscale",
  "sepia",
  "invert",
];
const ASPECTS: ReadonlyArray<[number, number]> = [
  [1, 1],
  [4, 3],
  [3, 2],
  [16, 9],
  [9, 16],
  [4, 5],
];
const ADJUST_SLIDERS = [
  "brightness",
  "contrast",
  "highlights",
  "shadows",
  "saturation",
  "vibrance",
  "temperature",
  "tint",
] as const;

let fontsRequest: Promise<string[]> | null = null;

/** Font families of the system, asked from the core once. */
function useFonts(): string[] {
  const [fonts, setFonts] = useState<string[]>([]);
  useEffect(() => {
    fontsRequest ??= api.fontsList().catch(() => []);
    let alive = true;
    void fontsRequest.then((list) => alive && setFonts(list));
    return () => {
      alive = false;
    };
  }, []);
  return fonts;
}

function FontSelect({ value, onChange }: { value: string; onChange: (font: string) => void }) {
  const { t } = useT();
  const fonts = useFonts();
  const known = value === "" || fonts.includes(value) ? fonts : [value, ...fonts];
  return (
    <Select
      value={value}
      options={[{ value: "", label: t("op.fontDefault") }, ...known.map((font) => ({ value: font, label: font }))]}
      onChange={onChange}
    />
  );
}

/** Choice among the installed models of one task. */
function ModelSelect({
  kind,
  value,
  onChange,
}: {
  kind: ComponentKind;
  value: string;
  onChange: (model: string) => void;
}) {
  const { t } = useT();
  const { ai, navigate } = useApp();
  const installed = (ai?.components ?? []).filter((component) => component.kind === kind && component.installed);
  if (ai && installed.length === 0) {
    return (
      <span className="inline-note">
        {t("op.modelMissing")}
        <Button small onClick={() => navigate("ai")}>
          {t("op.openModels")}
        </Button>
      </span>
    );
  }
  return (
    <Select
      value={value}
      options={[
        { value: "", label: t("op.modelDefault") },
        ...installed.map((component) => ({ value: component.id, label: component.name })),
      ]}
      onChange={onChange}
    />
  );
}

function ResizeForm({ op, onChange }: { op: ResizeOp; onChange: (op: Op) => void }) {
  const { t, td } = useT();
  const set = (patch: Partial<ResizeOp>) => onChange({ ...op, ...patch });
  const needsWidth = op.mode !== "percent" && op.mode !== "height";
  const needsHeight = op.mode === "fit" || op.mode === "fill" || op.mode === "exact" || op.mode === "height";
  return (
    <>
      <Field label={t("op.mode")}>
        <Select
          value={op.mode}
          options={RESIZE_MODES.map((mode) => ({ value: mode, label: td(`op.resizeMode.${mode}`) }))}
          onChange={(mode) => set({ mode })}
        />
      </Field>
      {op.mode === "percent" ? (
        <Field label={t("op.percent")}>
          <NumberInput value={op.percent} onChange={(percent) => set({ percent })} min={1} max={1000} suffix="%" />
        </Field>
      ) : (
        <Field label={t("op.size")}>
          {needsWidth ? (
            <NumberInput
              value={op.width}
              onChange={(width) => set({ width })}
              min={0}
              max={65535}
              suffix={needsHeight ? "×" : "px"}
              wide
            />
          ) : null}
          {needsHeight ? (
            <NumberInput value={op.height} onChange={(height) => set({ height })} min={0} max={65535} suffix="px" wide />
          ) : null}
        </Field>
      )}
      <Field label={t("op.resample")}>
        <Select value={op.filter} options={RESIZE_FILTERS} onChange={(filter) => set({ filter })} />
      </Field>
      <Field label="">
        <Checkbox checked={op.upscale} onChange={(upscale) => set({ upscale })} label={t("op.allowEnlarge")} />
        <Checkbox
          checked={op.gammaCorrect}
          onChange={(gammaCorrect) => set({ gammaCorrect })}
          label={t("op.gammaCorrect")}
        />
      </Field>
    </>
  );
}

function CropForm({ op, onChange }: { op: CropOp; onChange: (op: Op) => void }) {
  const { t, td } = useT();
  const set = (patch: Partial<CropOp>) => onChange({ ...op, ...patch });
  const percent = (value: number) => Math.round(value * 1000) / 10;
  return (
    <>
      <Field label={t("op.mode")}>
        <Segmented
          value={op.mode}
          options={[
            { value: "rect", label: t("op.cropRect") },
            { value: "aspect", label: t("op.cropAspect") },
          ]}
          onChange={(mode) => set({ mode })}
        />
      </Field>
      {op.mode === "rect" ? (
        <>
          <Field label={t("op.offset")}>
            <NumberInput
              value={percent(op.x)}
              onChange={(x) => set({ x: Math.min(x / 100, 1 - op.width) })}
              min={0}
              max={99}
              step={0.1}
              suffix="%"
            />
            <NumberInput
              value={percent(op.y)}
              onChange={(y) => set({ y: Math.min(y / 100, 1 - op.height) })}
              min={0}
              max={99}
              step={0.1}
              suffix="%"
            />
          </Field>
          <Field label={t("op.size")}>
            <NumberInput
              value={percent(op.width)}
              onChange={(width) => set({ width: Math.min(width / 100, 1 - op.x) })}
              min={1}
              max={100}
              step={0.1}
              suffix="%"
            />
            <NumberInput
              value={percent(op.height)}
              onChange={(height) => set({ height: Math.min(height / 100, 1 - op.y) })}
              min={1}
              max={100}
              step={0.1}
              suffix="%"
            />
          </Field>
        </>
      ) : (
        <>
          <Field label={t("op.aspect")}>
            <NumberInput value={op.aspectWidth} onChange={(aspectWidth) => set({ aspectWidth })} min={1} max={10000} />
            <NumberInput
              value={op.aspectHeight}
              onChange={(aspectHeight) => set({ aspectHeight })}
              min={1}
              max={10000}
            />
          </Field>
          <Field label="">
            {ASPECTS.map(([width, height]) => (
              <Button key={`${width}:${height}`} small onClick={() => set({ aspectWidth: width, aspectHeight: height })}>
                {width}:{height}
              </Button>
            ))}
          </Field>
          <Field label={t("op.anchor")}>
            <Select
              value={op.anchor}
              options={ANCHORS.map((anchor) => ({ value: anchor, label: td(`op.anchorKind.${anchor}`) }))}
              onChange={(anchor) => set({ anchor })}
            />
          </Field>
        </>
      )}
    </>
  );
}

function RotateForm({ op, onChange }: { op: RotateOp; onChange: (op: Op) => void }) {
  const { t } = useT();
  const set = (patch: Partial<RotateOp>) => onChange({ ...op, ...patch });
  return (
    <>
      <Field label={t("op.angle")}>
        <Slider value={op.angle} onChange={(angle) => set({ angle })} min={-180} max={180} step={0.5} reset={0} />
      </Field>
      <Field label="">
        {[-90, 90, 180].map((angle) => (
          <Button key={angle} small onClick={() => set({ angle })}>
            {angle > 0 ? `+${angle}°` : `${angle}°`}
          </Button>
        ))}
      </Field>
      <Field label="">
        <Checkbox checked={op.expand} onChange={(expand) => set({ expand })} label={t("op.expandCanvas")} />
      </Field>
      <Field label={t("op.cornerColor")}>
        <ColorInput value={op.background.slice(0, 7)} onChange={(background) => set({ background })} />
        <Checkbox
          checked={op.background.length === 9 && op.background.endsWith("00")}
          onChange={(transparent) =>
            set({ background: transparent ? `${op.background.slice(0, 7)}00` : op.background.slice(0, 7) })
          }
          label={t("op.transparent")}
        />
      </Field>
    </>
  );
}

function AdjustForm({ op, onChange }: { op: AdjustOp; onChange: (op: Op) => void }) {
  const { t, td } = useT();
  return (
    <>
      <Field label={t("op.adjust.exposure")}>
        <Slider
          value={op.exposure}
          onChange={(exposure) => onChange({ ...op, exposure })}
          min={-5}
          max={5}
          step={0.05}
          reset={0}
        />
      </Field>
      {ADJUST_SLIDERS.map((name) => (
        <Field key={name} label={td(`op.adjust.${name}`)}>
          <Slider value={op[name]} onChange={(value) => onChange({ ...op, [name]: value })} min={-100} max={100} reset={0} />
        </Field>
      ))}
    </>
  );
}

function FilterForm({ op, onChange }: { op: FilterOp; onChange: (op: Op) => void }) {
  const { t, td } = useT();
  const set = (patch: Partial<FilterOp>) => onChange({ ...op, ...patch });
  const hasRadius = op.kind === "sharpen" || op.kind === "blur" || op.kind === "denoise";
  return (
    <>
      <Field label={t("op.filter")}>
        <Select
          value={op.kind}
          options={FILTER_KINDS.map((kind) => ({ value: kind, label: td(`op.filterKind.${kind}`) }))}
          onChange={(kind) => set({ kind, radius: kind === "vignette" ? 50 : kind === "blur" ? 4 : 1 })}
        />
      </Field>
      {op.kind === "blur" ? null : (
        <Field label={t("op.amount")}>
          <Slider
            value={op.amount}
            onChange={(amount) => set({ amount })}
            min={op.kind === "vignette" ? -100 : 0}
            max={100}
            reset={50}
          />
        </Field>
      )}
      {hasRadius ? (
        <Field label={t("op.radius")}>
          <Slider value={op.radius} onChange={(radius) => set({ radius })} min={0} max={60} step={0.1} reset={1} />
        </Field>
      ) : null}
      {op.kind === "vignette" ? (
        <Field label={t("op.vignetteInner")}>
          <Slider value={op.radius} onChange={(radius) => set({ radius })} min={0} max={95} reset={50} />
        </Field>
      ) : null}
    </>
  );
}

function WatermarkForm({ op, onChange }: { op: WatermarkOp; onChange: (op: Op) => void }) {
  const { t, td } = useT();
  const set = (patch: Partial<WatermarkOp>) => onChange({ ...op, ...patch });
  return (
    <>
      <Field label={t("op.mode")}>
        <Segmented
          value={op.kind}
          options={[
            { value: "text", label: t("op.watermarkText") },
            { value: "image", label: t("op.watermarkImage") },
          ]}
          onChange={(kind) => set({ kind })}
        />
      </Field>
      {op.kind === "text" ? (
        <>
          <Field label={t("op.text")}>
            <TextInput value={op.text} onChange={(text) => set({ text })} wide />
          </Field>
          <Field label={t("op.font")}>
            <FontSelect value={op.font} onChange={(font) => set({ font })} />
          </Field>
          <Field label={t("op.textSize")} hint={t("op.shareOfShortSide")}>
            <Slider value={op.size} onChange={(size) => set({ size })} min={0.5} max={50} step={0.5} reset={5} />
            <ColorInput value={op.color} onChange={(color) => set({ color })} />
          </Field>
        </>
      ) : (
        <>
          <Field label={t("op.picture")}>
            <PathField kind="picture" value={op.image} onChange={(image) => set({ image })} />
          </Field>
          <Field label={t("op.scale")} hint={t("op.shareOfWidth")}>
            <Slider value={op.scale} onChange={(scale) => set({ scale })} min={1} max={100} reset={20} />
          </Field>
        </>
      )}
      <Field label={t("op.anchor")}>
        <Select
          value={op.anchor}
          options={ANCHORS.map((anchor) => ({ value: anchor, label: td(`op.anchorKind.${anchor}`) }))}
          onChange={(anchor) => set({ anchor })}
          disabled={op.tile}
        />
      </Field>
      <Field label={t("op.margin")}>
        <Slider value={op.margin} onChange={(margin) => set({ margin })} min={0} max={30} step={0.5} reset={3} />
      </Field>
      <Field label={t("op.opacity")}>
        <Slider value={op.opacity} onChange={(opacity) => set({ opacity })} min={0} max={100} reset={60} />
      </Field>
      <Field label={t("op.angle")}>
        <Slider value={op.rotation} onChange={(rotation) => set({ rotation })} min={-180} max={180} reset={0} />
      </Field>
      <Field label="">
        <Checkbox checked={op.tile} onChange={(tile) => set({ tile })} label={t("op.tile")} />
      </Field>
      {op.tile ? (
        <Field label={t("op.tileGap")}>
          <Slider value={op.tileGap} onChange={(tileGap) => set({ tileGap })} min={0} max={300} reset={60} />
        </Field>
      ) : null}
    </>
  );
}

interface OpFormProps {
  op: Op;
  onChange: (op: Op) => void;
  /** The host has a canvas to draw shapes and masks on. */
  interactive: boolean;
}

function OpForm({ op, onChange, interactive }: OpFormProps) {
  const { t } = useT();
  switch (op.type) {
    case "resize":
      return <ResizeForm op={op} onChange={onChange} />;
    case "crop":
      return <CropForm op={op} onChange={onChange} />;
    case "rotate":
      return <RotateForm op={op} onChange={onChange} />;
    case "flip":
      return (
        <Field label="">
          <Checkbox
            checked={op.horizontal}
            onChange={(horizontal) => onChange({ ...op, horizontal })}
            label={t("op.flipHorizontal")}
          />
          <Checkbox
            checked={op.vertical}
            onChange={(vertical) => onChange({ ...op, vertical })}
            label={t("op.flipVertical")}
          />
        </Field>
      );
    case "adjust":
      return <AdjustForm op={op} onChange={onChange} />;
    case "filter":
      return <FilterForm op={op} onChange={onChange} />;
    case "watermark":
      return <WatermarkForm op={op} onChange={onChange} />;
    case "annotate":
      return (
        <Field label={t("op.shapes")} hint={interactive ? t("op.drawHint") : t("op.drawInEditor")}>
          <span>{op.shapes.length}</span>
          <Button small disabled={op.shapes.length === 0} onClick={() => onChange({ ...op, shapes: op.shapes.slice(0, -1) })}>
            {t("op.removeLast")}
          </Button>
          <Button small disabled={op.shapes.length === 0} onClick={() => onChange({ ...op, shapes: [] })}>
            {t("common.clear")}
          </Button>
        </Field>
      );
    case "removeBackground":
      return (
        <>
          <Field label={t("op.model")}>
            <ModelSelect kind="background" value={op.model} onChange={(model) => onChange({ ...op, model })} />
          </Field>
          <Field label={t("op.newBackground")}>
            <Checkbox
              checked={op.background === null}
              onChange={(transparent) => onChange({ ...op, background: transparent ? null : "#ffffff" })}
              label={t("op.transparent")}
            />
            {op.background === null ? null : (
              <ColorInput value={op.background} onChange={(background) => onChange({ ...op, background })} />
            )}
          </Field>
        </>
      );
    case "upscale":
      return (
        <>
          <Field label={t("op.model")}>
            <ModelSelect kind="upscale" value={op.model} onChange={(model) => onChange({ ...op, model })} />
          </Field>
          <Field label={t("op.factor")}>
            <Segmented
              value={String(op.factor)}
              options={[
                { value: "2", label: "2×" },
                { value: "4", label: "4×" },
              ]}
              onChange={(factor) => onChange({ ...op, factor: Number(factor) })}
            />
          </Field>
        </>
      );
    case "restoreFaces":
      return (
        <>
          <Field label={t("op.model")}>
            <ModelSelect kind="faces" value={op.model} onChange={(model) => onChange({ ...op, model })} />
          </Field>
          <Field label={t("op.strength")}>
            <Slider value={op.strength} onChange={(strength) => onChange({ ...op, strength })} min={0} max={100} reset={80} />
          </Field>
        </>
      );
    case "inpaint":
      return (
        <>
          <Field label={t("op.mask")} hint={interactive ? t("op.maskHint") : t("op.drawInEditor")}>
            <span>{op.strokes.length}</span>
            <Button small disabled={op.strokes.length === 0} onClick={() => onChange({ ...op, strokes: op.strokes.slice(0, -1) })}>
              {t("op.removeLast")}
            </Button>
            <Button small disabled={op.strokes.length === 0} onClick={() => onChange({ ...op, strokes: [] })}>
              {t("common.clear")}
            </Button>
          </Field>
          <Field label={t("op.prompt")} hint={t("op.promptHint")}>
            <TextInput value={op.prompt} onChange={(prompt) => onChange({ ...op, prompt })} wide />
          </Field>
          {op.prompt.trim() === "" ? (
            <Field label={t("op.model")}>
              <ModelSelect kind="inpaint" value={op.model} onChange={(model) => onChange({ ...op, model })} />
            </Field>
          ) : null}
        </>
      );
    default: {
      const unreachable: never = op;
      return unreachable;
    }
  }
}

interface StepsEditorProps {
  steps: Step[];
  onChange: (steps: Step[]) => void;
  /** The step whose form is open. Without it the list keeps the choice itself. */
  selected?: string | null;
  onSelect?: (id: string | null) => void;
  /** Controls the host adds to the open step, such as the tools of a canvas. */
  extra?: (step: Step) => ReactNode;
  interactive?: boolean;
}

export function StepsEditor({ steps, onChange, selected, onSelect, extra, interactive = false }: StepsEditorProps) {
  const { t, td } = useT();
  const [own, setOwn] = useState<string | null>(null);
  const open = selected === undefined ? own : selected;
  const select = (id: string | null) => {
    setOwn(id);
    onSelect?.(id);
  };

  const replace = (id: string, patch: Partial<Step>) =>
    onChange(steps.map((step) => (step.id === id ? { ...step, ...patch } : step)));

  const move = (index: number, shift: number) => {
    const target = index + shift;
    if (target < 0 || target >= steps.length) {
      return;
    }
    const next = [...steps];
    [next[index], next[target]] = [next[target], next[index]];
    onChange(next);
  };

  const add = (kind: OpKind) => {
    const step = newStep(kind);
    onChange([...steps, step]);
    select(step.id);
  };

  return (
    <div className="steps">
      {steps.length === 0 ? <div className="steps-empty">{t("steps.empty")}</div> : null}
      {steps.map((step, index) => {
        const isOpen = step.id === open;
        return (
          <div key={step.id} className={cx("step", isOpen && "step-open", !step.enabled && "step-off")}>
            <div className="step-head">
              <input
                type="checkbox"
                checked={step.enabled}
                title={t("steps.enabled")}
                onChange={(event) => replace(step.id, { enabled: event.target.checked })}
              />
              <button type="button" className="step-title" onClick={() => select(isOpen ? null : step.id)}>
                {td(`ops.${step.op.type}`)}
                {AI_KINDS.includes(step.op.type) ? <span className="step-badge">AI</span> : null}
              </button>
              <Button small variant="ghost" disabled={index === 0} title={t("steps.up")} onClick={() => move(index, -1)}>
                ↑
              </Button>
              <Button
                small
                variant="ghost"
                disabled={index === steps.length - 1}
                title={t("steps.down")}
                onClick={() => move(index, 1)}
              >
                ↓
              </Button>
              <Button
                small
                variant="ghost"
                title={t("common.remove")}
                onClick={() => {
                  onChange(steps.filter((item) => item.id !== step.id));
                  if (isOpen) {
                    select(null);
                  }
                }}
              >
                ✕
              </Button>
            </div>
            {isOpen ? (
              <div className="step-body">
                {extra?.(step)}
                <OpForm op={step.op} onChange={(op) => replace(step.id, { op })} interactive={interactive} />
              </div>
            ) : null}
          </div>
        );
      })}
      <select
        className="input select steps-add"
        value=""
        onChange={(event) => {
          if (event.target.value !== "") {
            add(event.target.value as OpKind);
          }
        }}
      >
        <option value="">{t("steps.add")}</option>
        {OP_KINDS.map((kind) => (
          <option key={kind} value={kind}>
            {td(`ops.${kind}`)}
          </option>
        ))}
      </select>
    </div>
  );
}
