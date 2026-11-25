// The editor: a picture, a stack of steps that never touches the file, and
// the export. Steps shaped by hand (crop, annotations, masks) are drawn over
// the picture as it is before that step.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, toAppError } from "../api";
import { editorBaseUrl, editorRenderUrl } from "../api/urls";
import { useApp } from "../app/context";
import { OutputEditor } from "../components/OutputEditor";
import { StepsEditor } from "../components/StepsEditor";
import { useT, type Translator } from "../i18n";
import { extensionOf, fileName, folderOf, formatBytes, formatSaving, newId, stemOf } from "../lib/format";
import {
  extensionOfFormat,
  FORMAT_NAMES,
  formatOfExtension,
  isDestructive,
  isWritable,
  PIPELINE_VERSION,
  recipeName,
  usesGenerator,
} from "../lib/pipeline";
import type {
  AppError,
  BatchReport,
  Comparison,
  EditorInfo,
  Estimate,
  Histogram,
  ImageFormat,
  Op,
  OutputSpec,
  Pipeline,
  RenderInfo,
  ShapeKind,
  Step,
} from "../types";
import {
  Button,
  Checkbox,
  ColorInput,
  EmptyState,
  Field,
  Modal,
  PageFrame,
  Select,
  Slider,
  Tabs,
  TextInput,
} from "../ui";
import { DEFAULT_TOOL, Overlay, type BrushedOp, type ShapedOp, type ToolState } from "./editor/Overlay";

/** Long side of the picture the editor works on, in pixels. */
const PREVIEW_SIDE = 2048;
const RENDER_DELAY_MS = 120;
/** Changes closer than this make one undo step, so a slider drag is undone at once. */
const UNDO_MERGE_MS = 600;
const COMPARED: readonly ImageFormat[] = ["jpeg", "webp", "avif", "jxl", "png"];
const SHAPES: readonly ShapeKind[] = [
  "arrow",
  "line",
  "rect",
  "ellipse",
  "pen",
  "text",
  "highlight",
  "blur",
  "pixelate",
];

function isTyping(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement
  );
}

/** Whether a step is shaped on the picture as long as it is selected. */
function shapedByHand(op: Op): boolean {
  return (op.type === "crop" && op.mode === "rect") || op.type === "annotate";
}

/** Whether a step has an area painted with the brush, which is done in a mode of its own. */
function isBrushed(op: Op): op is BrushedOp {
  return op.type === "inpaint" || op.type === "aiEdit" || op.type === "removeBackground";
}

/** Whether a brushed step has nothing to show yet, so it opens ready to be painted. */
function awaitsBrush(op: BrushedOp): boolean {
  switch (op.type) {
    case "inpaint":
      return op.strokes.length === 0;
    case "aiEdit":
      return op.prompt.trim() === "";
    case "removeBackground":
      return false;
    default: {
      const unreachable: never = op;
      return unreachable;
    }
  }
}

/** Whether a brushed step has what it needs to run. */
function canRun(op: BrushedOp): boolean {
  switch (op.type) {
    case "inpaint":
      return op.strokes.length > 0;
    case "aiEdit":
      return op.prompt.trim() !== "";
    case "removeBackground":
      return true;
    default: {
      const unreachable: never = op;
      return unreachable;
    }
  }
}

/** Limits of the brush, in percent of the short side, and what a key press changes it by. */
const BRUSH_MIN = 0.5;
const BRUSH_MAX = 30;
const BRUSH_KEY_STEP = 1;

/** The brush after a key press: whole sizes, with the smallest one below them. */
function nextBrush(brush: number, grow: boolean): number {
  const next = grow ? Math.floor(brush + BRUSH_KEY_STEP) : Math.ceil(brush - BRUSH_KEY_STEP);
  return Math.min(BRUSH_MAX, Math.max(BRUSH_MIN, next));
}

/** The button that turns the brush of a step on. */
function paintLabel(op: BrushedOp, t: Translator["t"]): string {
  switch (op.type) {
    case "inpaint":
    case "aiEdit":
      return t("editor.editArea");
    case "removeBackground":
      return t("editor.markKeep");
    default: {
      const unreachable: never = op;
      return unreachable;
    }
  }
}

/** The step the overlay shapes, when the op is one it can shape. */
function asShaped(op: Op): ShapedOp | null {
  return op.type === "crop" || op.type === "annotate" || isBrushed(op) ? op : null;
}

function HistogramView({ histogram }: { histogram: Histogram }) {
  const peak = Math.max(1, ...histogram.luma.slice(1, 255));
  const curve = (values: number[]) =>
    `M0 64 ${values.map((value, index) => `L${index} ${64 - Math.min(1, value / peak) * 64}`).join(" ")} L255 64Z`;
  return (
    <svg className="histogram" viewBox="0 0 255 64" preserveAspectRatio="none">
      <path d={curve(histogram.luma)} fill="currentColor" opacity={0.35} />
      <path d={curve(histogram.red)} fill="#e53935" opacity={0.35} />
      <path d={curve(histogram.green)} fill="#43a047" opacity={0.35} />
      <path d={curve(histogram.blue)} fill="#1e88e5" opacity={0.35} />
    </svg>
  );
}

function ToolControls({
  step,
  tool,
  painting,
  onTool,
}: {
  step: Step;
  tool: ToolState;
  /** The area of this step is being painted with the brush. */
  painting: boolean;
  onTool: (patch: Partial<ToolState>) => void;
}) {
  const { t, td } = useT();
  if (isBrushed(step.op)) {
    // The brush is always in sight; while it is off, the hint names the button that turns it on.
    const button = paintLabel(step.op, t);
    return (
      <Field label={t("editor.brush")} hint={painting ? t("editor.brushHint") : t("editor.brushIdle", { button })}>
        <Slider
          value={tool.brush}
          onChange={(brush) => onTool({ brush })}
          min={BRUSH_MIN}
          max={BRUSH_MAX}
          step={0.5}
          reset={DEFAULT_TOOL.brush}
        />
      </Field>
    );
  }
  if (step.op.type !== "annotate") {
    return null;
  }
  const outlined = tool.shape === "rect" || tool.shape === "ellipse";
  const stroked = tool.shape !== "text" && tool.shape !== "blur" && tool.shape !== "pixelate" && tool.shape !== "highlight";
  return (
    <>
      <Field label={t("editor.tool")}>
        <Select
          value={tool.shape}
          options={SHAPES.map((shape) => ({ value: shape, label: td(`op.shape.${shape}`) }))}
          onChange={(shape) => onTool({ shape })}
        />
        <ColorInput value={tool.color} onChange={(color) => onTool({ color })} />
      </Field>
      {stroked ? (
        <Field label={t("editor.strokeWidth")}>
          <Slider value={tool.width} onChange={(width) => onTool({ width })} min={0.1} max={10} step={0.1} reset={0.6} />
        </Field>
      ) : null}
      {outlined ? (
        <Field label="">
          <Checkbox checked={tool.fill} onChange={(fill) => onTool({ fill })} label={t("editor.fill")} />
        </Field>
      ) : null}
      {tool.shape === "text" ? (
        <>
          <Field label={t("op.text")} hint={t("editor.textHint")}>
            <TextInput value={tool.text} onChange={(text) => onTool({ text })} wide />
          </Field>
          <Field label={t("op.textSize")}>
            <Slider value={tool.size} onChange={(size) => onTool({ size })} min={1} max={30} step={0.5} reset={4} />
          </Field>
        </>
      ) : null}
    </>
  );
}

/** The buttons that run a brushed step or return to painting its area. */
function RunControls({
  op,
  painting,
  busy,
  onPaint,
  onAgain,
}: {
  op: BrushedOp;
  painting: boolean;
  busy: boolean;
  onPaint: (painting: boolean) => void;
  /** Asks the generator for another result. */
  onAgain: () => void;
}) {
  const { t } = useT();
  if (painting) {
    return (
      <div className="step-run">
        <Button variant="primary" disabled={!canRun(op)} onClick={() => onPaint(false)}>
          {op.type === "removeBackground" ? t("editor.done") : t("editor.run")}
        </Button>
      </div>
    );
  }
  switch (op.type) {
    case "inpaint":
      return (
        <div className="step-run">
          <Button small onClick={() => onPaint(true)}>
            {t("editor.editArea")}
          </Button>
          {busy ? <span className="step-run-state">{t("common.working")}</span> : null}
        </div>
      );
    case "aiEdit":
      return (
        <div className="step-run">
          <Button small onClick={() => onPaint(true)}>
            {t("editor.editArea")}
          </Button>
          <Button small disabled={busy} onClick={onAgain}>
            {t("editor.anotherVariant")}
          </Button>
          {busy ? <span className="step-run-state">{t("common.working")}</span> : null}
        </div>
      );
    case "removeBackground":
      return (
        <div className="step-run">
          <Button small onClick={() => onPaint(true)}>
            {t("editor.markKeep")}
          </Button>
          {busy ? <span className="step-run-state">{t("common.working")}</span> : null}
        </div>
      );
    default: {
      const unreachable: never = op;
      return unreachable;
    }
  }
}

function CompareDialog({
  rows,
  onPick,
  onClose,
}: {
  rows: Comparison[] | null;
  onPick: (format: ImageFormat) => void;
  onClose: () => void;
}) {
  const { t, error } = useT();
  const smallest = Math.min(...(rows ?? []).filter((row) => !row.error).map((row) => row.bytes));
  return (
    <Modal
      title={t("editor.compareTitle")}
      onClose={onClose}
      wide
      footer={
        <Button variant="primary" onClick={onClose}>
          {t("common.close")}
        </Button>
      }
    >
      {rows === null ? (
        <div className="modal-text">{t("common.working")}</div>
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th>{t("output.format")}</th>
              <th>{t("editor.fileSize")}</th>
              <th>{t("editor.score")}</th>
              <th>{t("editor.encodeTime")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.format} className={row.bytes === smallest && !row.error ? "table-best" : undefined}>
                <td>{FORMAT_NAMES[row.format]}</td>
                {row.error ? (
                  <td colSpan={3}>{error(row.error)}</td>
                ) : (
                  <>
                    <td>{formatBytes(row.bytes)}</td>
                    <td>{row.score === null ? "—" : row.score.toFixed(1)}</td>
                    <td>{t("editor.milliseconds", { ms: row.elapsedMs })}</td>
                  </>
                )}
                <td>
                  <Button small disabled={Boolean(row.error)} onClick={() => onPick(row.format)}>
                    {t("editor.useFormat")}
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Modal>
  );
}

export default function EditorPage({ active }: { active: boolean }) {
  const { t, td, error: errorText } = useT();
  const {
    settings,
    editorPath,
    openInEditor,
    recipes,
    refreshRecipes,
    confirm,
    askName,
    notify,
    showError,
    withCloud,
    jobResult,
  } = useApp();

  const [session, setSession] = useState<EditorInfo | null>(null);
  const [loading, setLoading] = useState(false);
  const [history, setHistory] = useState<Step[][]>([[]]);
  const [cursor, setCursor] = useState(0);
  const [selected, setSelected] = useState<string | null>(null);
  /** The brushed step whose area is being painted; it runs when the painting ends. */
  const [painting, setPainting] = useState<string | null>(null);
  const [render, setRender] = useState<RenderInfo | null>(null);
  const [rendering, setRendering] = useState(false);
  const [renderError, setRenderError] = useState<AppError | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [comparing, setComparing] = useState(false);
  const [actual, setActual] = useState(false);
  const [tool, setTool] = useState<ToolState>(DEFAULT_TOOL);
  const [output, setOutput] = useState<OutputSpec>(settings.output);
  const [panel, setPanel] = useState<"steps" | "export">("steps");
  const [estimate, setEstimate] = useState<Estimate | null>(null);
  const [estimating, setEstimating] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [comparison, setComparison] = useState<{ rows: Comparison[] | null } | null>(null);
  const [stage, setStage] = useState({ width: 0, height: 0 });
  const [dirty, setDirty] = useState(false);

  const steps = history[cursor];
  const stageRef = useRef<HTMLDivElement | null>(null);
  const lastCommit = useRef(0);
  const cloudAgreed = useRef(false);
  const renderCount = useRef(0);
  const sessionRef = useRef<EditorInfo | null>(null);
  sessionRef.current = session;

  const commit = useCallback(
    (next: Step[]) => {
      const now = Date.now();
      const merge = now - lastCommit.current < UNDO_MERGE_MS && cursor > 0 && cursor === history.length - 1;
      lastCommit.current = now;
      const kept = history.slice(0, merge ? cursor : cursor + 1);
      setHistory([...kept, next]);
      setCursor(kept.length);
      setDirty(true);
      setEstimate(null);
    },
    [cursor, history],
  );

  const stepsRef = useRef(steps);
  stepsRef.current = steps;
  const commitRef = useRef(commit);
  commitRef.current = commit;

  const undo = useCallback(() => {
    lastCommit.current = 0;
    setCursor((current) => Math.max(0, current - 1));
    setEstimate(null);
  }, []);

  const redo = useCallback(() => {
    lastCommit.current = 0;
    setCursor((current) => Math.min(history.length - 1, current + 1));
    setEstimate(null);
  }, [history.length]);

  const load = useCallback(
    async (path: string) => {
      setLoading(true);
      try {
        const opened = await api.editorOpen(path, PREVIEW_SIDE);
        const previous = sessionRef.current;
        if (previous) {
          void api.editorClose(previous.id);
        }
        cloudAgreed.current = false;
        setSession(opened);
        setHistory([[]]);
        setCursor(0);
        setSelected(null);
        setPainting(null);
        setRenderError(null);
        setRender(null);
        setEstimate(null);
        setDirty(false);
        setActual(false);
      } catch (error) {
        showError(error);
        // The editor stays with the picture it had.
        const previous = sessionRef.current;
        if (previous) {
          openInEditor(previous.path);
        }
      } finally {
        setLoading(false);
      }
    },
    [openInEditor, showError],
  );

  useEffect(() => {
    const current = sessionRef.current;
    if (!editorPath || editorPath === current?.path) {
      return;
    }
    void (async () => {
      if (current && dirty) {
        const agreed = await confirm({
          title: t("editor.discardTitle"),
          text: t("editor.discardText", { name: fileName(current.path) }),
          confirmLabel: t("editor.discard"),
          danger: true,
        });
        if (!agreed) {
          openInEditor(current.path);
          return;
        }
      }
      await load(editorPath);
    })();
    // Only a new path opens a picture; the other values are read as they are then.
  }, [editorPath]);

  // The session of the core lives as long as this page.
  useEffect(
    () => () => {
      const current = sessionRef.current;
      if (current) {
        void api.editorClose(current.id);
      }
    },
    [],
  );

  /** Steps as the panel changes them: a brushed step that was changed waits for its "run". */
  const changeSteps = useCallback(
    (next: Step[]) => {
      const added = next.find((step) => !steps.some((known) => known.id === step.id));
      if (added) {
        setPainting(isBrushed(added.op) && awaitsBrush(added.op) ? added.id : null);
      } else {
        const before = steps.find((step) => step.id === selected);
        const after = next.find((step) => step.id === selected);
        const reshaped =
          before && after && (after.op.type === "inpaint" || after.op.type === "aiEdit") && before.op !== after.op;
        if (reshaped) {
          setPainting(after.id);
        }
      }
      commit(next);
    },
    [commit, selected, steps],
  );

  const selectStep = useCallback((id: string | null) => {
    setSelected(id);
    const step = stepsRef.current.find((known) => known.id === id);
    if (step) {
      setPainting(isBrushed(step.op) && awaitsBrush(step.op) ? step.id : null);
    } else if (id === null) {
      setPainting(null);
    }
    // A step added a moment ago is not in the list yet; `changeSteps` has decided for it.
  }, []);

  const focusIndex = steps.findIndex(
    (step) =>
      step.id === selected &&
      step.enabled &&
      (shapedByHand(step.op) || (isBrushed(step.op) && painting === step.id)),
  );
  const focused = focusIndex >= 0 ? steps[focusIndex] : null;
  const focusedOp = focused ? asShaped(focused.op) : null;
  const renderSteps = useMemo(() => (focusIndex >= 0 ? steps.slice(0, focusIndex) : steps), [focusIndex, steps]);
  const renderKey = useMemo(() => JSON.stringify(renderSteps), [renderSteps]);

  useEffect(() => {
    if (!session) {
      return;
    }
    const id = session.id;
    const handle = window.setTimeout(() => {
      const count = ++renderCount.current;
      setRendering(true);
      setRenderError(null);
      const run = (confirmed: boolean) => api.editorRender(id, renderSteps, confirmed);
      const request = cloudAgreed.current
        ? run(true)
        : withCloud(async (confirmed) => {
            const result = await run(confirmed);
            cloudAgreed.current = cloudAgreed.current || confirmed;
            return result;
          });
      request
        .then((info) => {
          if (count === renderCount.current) {
            setRender(info);
          }
        })
        .catch((error: unknown) => {
          const failure = toAppError(error);
          if (failure.code !== "cancelled") {
            // The failure stays beside the picture until the next render: a toast for
            // every stroke of the brush would hide what went wrong.
            if (count === renderCount.current) {
              setRenderError(failure);
            }
          } else if (count === renderCount.current && !cloudAgreed.current) {
            // The upload was declined: the step that needs it is switched off.
            const current = stepsRef.current;
            const declined = current.map((step) => (usesGenerator(step.op) ? { ...step, enabled: false } : step));
            if (JSON.stringify(declined) !== JSON.stringify(current)) {
              commitRef.current(declined);
            }
          }
        })
        .finally(() => {
          if (count === renderCount.current) {
            setRendering(false);
          }
        });
    }, RENDER_DELAY_MS);
    return () => window.clearTimeout(handle);
    // The key stands for the steps to render; `attempt` repeats a failed render.
  }, [session, renderKey, attempt]);

  useEffect(() => {
    const element = stageRef.current;
    if (!element) {
      return;
    }
    const observer = new ResizeObserver(() =>
      setStage({ width: element.clientWidth, height: element.clientHeight }),
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [session !== null]);

  const pipeline = useCallback(
    (spec: OutputSpec): Pipeline => ({ version: PIPELINE_VERSION, steps, output: spec }),
    [steps],
  );

  const exportWith = useCallback(
    async (spec: OutputSpec, ask: boolean) => {
      if (!session) {
        return;
      }
      const request = pipeline(spec);
      if (ask && isDestructive(request) && settings.confirmOverwrite) {
        const agreed = await confirm({
          title: t("batch.overwriteTitle"),
          text: t("batch.overwriteText"),
          confirmLabel: t("common.continue"),
          danger: true,
        });
        if (!agreed) {
          return;
        }
      }
      setExporting(true);
      try {
        const id = await withCloud((confirmed) =>
          api.batchStart({
            title: fileName(session.path),
            inputs: [session.path],
            recursive: false,
            pipeline: request,
            confirmed,
          }),
        );
        const report = await jobResult<BatchReport>(id);
        const item = report.items[0];
        if (item?.error) {
          showError(item.error);
        } else if (item?.outcome) {
          setDirty(false);
          notify(
            item.outcome.status === "skipped"
              ? t("editor.exportSkipped")
              : t("editor.exported", { name: fileName(item.outcome.output), size: formatBytes(item.outcome.outputBytes) }),
          );
        }
      } catch (error) {
        showError(error);
      } finally {
        setExporting(false);
      }
    },
    [confirm, jobResult, notify, pipeline, session, settings.confirmOverwrite, showError, t, withCloud],
  );

  const saveAs = useCallback(async () => {
    if (!session) {
      return;
    }
    const sourceFormat = formatOfExtension(extensionOf(session.path));
    const format = output.format === "same" || output.format === "auto" ? (sourceFormat ?? "png") : output.format;
    const extension = extensionOfFormat(format);
    const chosen = await api.pickSave(`${stemOf(session.path)}.${extension}`, FORMAT_NAMES[format], [extension]);
    if (!chosen) {
      return;
    }
    const chosenFormat = formatOfExtension(extensionOf(chosen));
    if (!chosenFormat) {
      showError({ code: "unsupportedFormat", params: { path: chosen } });
      return;
    }
    // The dialog of the system has already asked about replacing the file.
    await exportWith(
      {
        ...output,
        format: chosenFormat,
        destination: { kind: "folder", path: folderOf(chosen) },
        naming: stemOf(chosen),
        conflict: "overwrite",
        replaceOriginal: false,
        keepStructure: false,
      },
      false,
    );
  }, [exportWith, output, session, showError]);

  const runEstimate = async () => {
    if (!session) {
      return;
    }
    setEstimating(true);
    try {
      setEstimate(await api.exportEstimate(session.path, pipeline(output)));
    } catch (error) {
      showError(error);
    } finally {
      setEstimating(false);
    }
  };

  const runComparison = async () => {
    if (!session) {
      return;
    }
    setComparison({ rows: null });
    try {
      const rows = await api.compareFormats(session.path, pipeline(output), [...COMPARED]);
      setComparison((current) => (current ? { rows } : current));
    } catch (error) {
      setComparison(null);
      showError(error);
    }
  };

  const saveRecipe = async () => {
    const name = await askName({ title: t("recipes.saveTitle"), label: t("recipes.name") });
    if (!name) {
      return;
    }
    try {
      await api.recipeSave({
        id: "",
        name,
        description: "",
        pipeline: pipeline(output),
        builtin: false,
        pinned: false,
        updated: 0,
      });
      refreshRecipes();
      notify(t("recipes.saved", { name }));
    } catch (error) {
      showError(error);
    }
  };

  const applyRecipe = (id: string) => {
    const recipe = recipes.find((item) => item.id === id);
    if (recipe) {
      lastCommit.current = 0;
      commit(recipe.pipeline.steps.map((step) => ({ ...step, id: newId() })));
      setOutput(recipe.pipeline.output);
    }
  };

  const addQuick = (op: Op) => {
    lastCommit.current = 0;
    commit([...steps, { id: newId(), enabled: true, op }]);
  };

  const chooseFile = useCallback(async () => {
    const [chosen] = await api.pickFiles(false);
    if (chosen) {
      openInEditor(chosen);
    }
  }, [openInEditor]);

  useEffect(() => {
    if (!active) {
      return;
    }
    const onDown = (event: KeyboardEvent) => {
      if (document.querySelector(".modal-backdrop")) {
        return;
      }
      if (event.ctrlKey && event.code === "KeyO") {
        event.preventDefault();
        void chooseFile();
      } else if (event.ctrlKey && event.code === "KeyS") {
        event.preventDefault();
        void (event.shiftKey ? saveAs() : exportWith(output, true));
      } else if (isTyping(event.target)) {
        return;
      } else if (event.ctrlKey && event.code === "KeyZ") {
        event.preventDefault();
        if (event.shiftKey) {
          redo();
        } else {
          undo();
        }
      } else if (event.ctrlKey && event.code === "KeyY") {
        event.preventDefault();
        redo();
      } else if (event.code === "Backslash") {
        setComparing(true);
      } else if (event.code === "Digit1") {
        setActual(true);
      } else if (event.code === "Digit0") {
        setActual(false);
      } else if (painting !== null && (event.code === "BracketLeft" || event.code === "BracketRight")) {
        // The keys beside P, whatever the layout prints on them.
        const grow = event.code === "BracketRight";
        setTool((current) => ({ ...current, brush: nextBrush(current.brush, grow) }));
      } else if (event.key === "Escape") {
        setSelected(null);
        setPainting(null);
      } else if (event.key === "Delete" && selected) {
        lastCommit.current = 0;
        commit(steps.filter((step) => step.id !== selected));
        setSelected(null);
        setPainting(null);
      }
    };
    const onUp = (event: KeyboardEvent) => {
      if (event.code === "Backslash") {
        setComparing(false);
      }
    };
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
    };
  }, [active, chooseFile, commit, exportWith, output, painting, redo, saveAs, selected, steps, undo]);

  if (!session) {
    return (
      <PageFrame title={t("nav.editor")}>
        <EmptyState
          title={loading ? t("common.working") : t("editor.emptyTitle")}
          text={loading ? undefined : t("editor.emptyText")}
          action={
            loading ? undefined : (
              <Button variant="primary" onClick={() => void chooseFile()}>
                {t("viewer.openFiles")}
              </Button>
            )
          }
        />
      </PageFrame>
    );
  }

  const shownWidth = comparing || !render ? session.width : render.width;
  const shownHeight = comparing || !render ? session.height : render.height;
  const fit = Math.min(1, (stage.width - 24) / shownWidth, (stage.height - 24) / shownHeight);
  const zoom = actual ? 1 : Math.max(0.02, fit);
  const source = comparing || !render ? editorBaseUrl(session.id) : editorRenderUrl(session.id, render.version);
  const fullWidth = Math.round(shownWidth / session.scale);
  const fullHeight = Math.round(shownHeight / session.scale);
  const failure = rendering ? null : renderError;
  const failureText = failure ? errorText(failure) : "";
  const failedKind = failure?.params?.step;
  const failedStep = failedKind ? td(`ops.${failedKind}`) : "";

  return (
    <PageFrame
      title={fileName(session.path)}
      actions={
        <>
          <Button small disabled={cursor === 0} onClick={undo} title="Ctrl+Z">
            {t("editor.undo")}
          </Button>
          <Button small disabled={cursor >= history.length - 1} onClick={redo} title="Ctrl+Y">
            {t("editor.redo")}
          </Button>
          <Button small onClick={() => addQuick({ type: "rotate", angle: -90, expand: true, background: "#00000000" })}>
            ⟲
          </Button>
          <Button small onClick={() => addQuick({ type: "rotate", angle: 90, expand: true, background: "#00000000" })}>
            ⟳
          </Button>
          <Button small onClick={() => addQuick({ type: "flip", horizontal: true, vertical: false })}>
            ⇋
          </Button>
          <Button
            small
            title={"\\"}
            onMouseDown={() => setComparing(true)}
            onMouseUp={() => setComparing(false)}
            onMouseLeave={() => setComparing(false)}
          >
            {t("editor.before")}
          </Button>
          <Button small onClick={() => setActual((current) => !current)} title="0 / 1">
            {actual ? t("editor.zoomFit") : "100%"}
          </Button>
          <Button small onClick={() => void chooseFile()} title="Ctrl+O">
            {t("editor.open")}
          </Button>
        </>
      }
    >
      <div className="editor">
        <div className="editor-view">
          <div className="editor-stage" ref={stageRef}>
            <div
              className="editor-canvas"
              style={{ width: Math.round(shownWidth * zoom), height: Math.round(shownHeight * zoom) }}
            >
              <img className="editor-image" src={source} alt="" draggable={false} />
              {focused && focusedOp && render && !comparing ? (
                <Overlay
                  width={render.width}
                  height={render.height}
                  zoom={zoom}
                  op={focusedOp}
                  tool={tool}
                  onChange={(op) => commit(steps.map((step) => (step.id === focused.id ? { ...step, op } : step)))}
                />
              ) : null}
            </div>
          </div>
          {rendering ? (
            <div className="editor-busy" role="status">
              <span className="editor-busy-mark" aria-hidden="true" />
              {t("common.working")}
            </div>
          ) : null}
          {failure ? (
            <div className="editor-failure" role="alert">
              <div className="editor-failure-text">
                {failedStep ? t("editor.stepFailed", { step: failedStep, error: failureText }) : failureText}
              </div>
              <Button small onClick={() => setAttempt((current) => current + 1)}>
                {t("editor.retry")}
              </Button>
            </div>
          ) : null}
          <div className="editor-status">
            {fullWidth} × {fullHeight}
            {comparing ? ` · ${t("editor.original")}` : ""}
          </div>
        </div>
        <aside className="editor-panel">
          <Tabs
            value={panel}
            options={[
              { value: "steps", label: t("editor.steps") },
              { value: "export", label: t("editor.export") },
            ]}
            onChange={setPanel}
          />
          <div className="editor-panel-body">
            {panel === "steps" ? (
              <>
                {render ? <HistogramView histogram={render.histogram} /> : null}
                <StepsEditor
                  steps={steps}
                  onChange={changeSteps}
                  selected={selected}
                  onSelect={selectStep}
                  interactive
                  extra={(step) => (
                    <ToolControls
                      step={step}
                      tool={tool}
                      painting={painting === step.id}
                      onTool={(patch) => setTool((current) => ({ ...current, ...patch }))}
                    />
                  )}
                  footer={(step) => {
                    const op = step.op;
                    return isBrushed(op) && step.enabled ? (
                      <RunControls
                        op={op}
                        painting={painting === step.id}
                        busy={rendering}
                        onPaint={(on) => setPainting(on ? step.id : null)}
                        onAgain={() => {
                          if (op.type === "aiEdit") {
                            lastCommit.current = 0;
                            commit(
                              steps.map((known) =>
                                known.id === step.id ? { ...known, op: { ...op, variant: op.variant + 1 } } : known,
                              ),
                            );
                          }
                        }}
                      />
                    ) : null;
                  }}
                />
                <div className="editor-recipes">
                  <Select
                    value=""
                    options={[
                      { value: "", label: t("editor.applyRecipe") },
                      ...recipes.map((recipe) => ({ value: recipe.id, label: recipeName(recipe, td) })),
                    ]}
                    onChange={applyRecipe}
                  />
                  <Button small disabled={steps.length === 0} onClick={() => void saveRecipe()}>
                    {t("recipes.saveAs")}
                  </Button>
                </div>
              </>
            ) : (
              <>
                <OutputEditor
                  value={output}
                  onChange={(next) => {
                    setOutput(next);
                    setEstimate(null);
                  }}
                />
                <div className="editor-estimate">
                  <Button small disabled={estimating} onClick={() => void runEstimate()}>
                    {estimating ? t("common.working") : t("editor.estimate")}
                  </Button>
                  <Button small onClick={() => void runComparison()}>
                    {t("editor.compare")}
                  </Button>
                  {estimate ? (
                    <div className="editor-estimate-text">
                      {t("editor.estimateText", {
                        size: formatBytes(estimate.bytes),
                        saving: formatSaving(estimate.sourceBytes, estimate.bytes),
                        width: estimate.width,
                        height: estimate.height,
                        format: FORMAT_NAMES[estimate.format],
                      })}
                      {estimate.targetMissed ? ` ${t("editor.targetMissed")}` : ""}
                    </div>
                  ) : null}
                </div>
              </>
            )}
          </div>
          <div className="editor-panel-foot">
            <Button disabled={exporting} onClick={() => void saveAs()} title="Ctrl+Shift+S">
              {t("editor.saveAs")}
            </Button>
            <Button variant="primary" disabled={exporting} onClick={() => void exportWith(output, true)} title="Ctrl+S">
              {exporting ? t("common.working") : t("editor.export")}
            </Button>
          </div>
        </aside>
      </div>
      {comparison ? (
        <CompareDialog
          rows={comparison.rows}
          onClose={() => setComparison(null)}
          onPick={(format) => {
            if (isWritable(format)) {
              setOutput({ ...output, format });
              setEstimate(null);
            }
            setComparison(null);
          }}
        />
      ) : null}
    </PageFrame>
  );
}
