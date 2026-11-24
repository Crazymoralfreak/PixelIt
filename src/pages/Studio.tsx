// The studio: pictures from a prompt, and a model that looks at pictures to
// describe, tag, name and read them.

import { useState } from "react";
import { api } from "../api";
import { thumbUrl } from "../api/urls";
import { useApp } from "../app/context";
import { FilesField } from "../components/FilesField";
import { JobRow } from "../components/JobsPanel";
import { PathField } from "../components/PathField";
import { useT } from "../i18n";
import { copyText } from "../lib/clipboard";
import { fileName } from "../lib/format";
import type { GenerateRequest, VisionResult, VisionTask } from "../types";
import {
  Button,
  Checkbox,
  EmptyState,
  Field,
  NumberInput,
  PageFrame,
  Section,
  Select,
  Slider,
  Tabs,
  Tag,
  TextArea,
  TextInput,
} from "../ui";

const SIZES: ReadonlyArray<[number, number]> = [
  [512, 512],
  [768, 768],
  [1024, 1024],
  [1216, 832],
  [832, 1216],
];
const TASKS: readonly VisionTask[] = ["describe", "altText", "tags", "name", "ocr"];

function GenerateTab() {
  const { t, td } = useT();
  const { ai, jobs, jobResult, navigate, openInEditor, addToQueue, notify, showError, withCloud } = useApp();
  const defaults = ai?.settings.generation;
  const [request, setRequest] = useState<GenerateRequest>({
    prompt: "",
    negative: "",
    width: 0,
    height: 0,
    steps: 0,
    cfg: 0,
    seed: null,
    count: 1,
    strength: 0.6,
  });
  const [source, setSource] = useState("");
  const [jobId, setJobId] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [results, setResults] = useState<string[]>([]);

  if (!ai) {
    return null;
  }
  if (!ai.capabilities.generation || !defaults) {
    return (
      <EmptyState
        title={t("studio.noProviderTitle")}
        text={t("studio.noProviderText")}
        action={
          <Button variant="primary" onClick={() => navigate("ai")}>
            {t("studio.setUp")}
          </Button>
        }
      />
    );
  }

  const set = (patch: Partial<GenerateRequest>) => setRequest({ ...request, ...patch });
  const width = request.width || defaults.width;
  const height = request.height || defaults.height;
  const job = jobId === null ? undefined : jobs.find((item) => item.id === jobId);

  const generate = async () => {
    setBusy(true);
    try {
      const id = await withCloud((confirmed) => api.generateStart(request, source || null, confirmed));
      setJobId(id);
      const { files } = await jobResult<{ files: string[] }>(id);
      setResults((current) => [...files, ...current]);
    } catch (error) {
      showError(error);
    } finally {
      setBusy(false);
      setJobId(null);
    }
  };

  return (
    <div className="columns">
      <div className="column">
        <Section>
          <Field label={t("studio.prompt")}>
            <TextArea value={request.prompt} onChange={(prompt) => set({ prompt })} rows={4} placeholder={t("studio.promptHint")} />
          </Field>
          <Field label={t("studio.negative")}>
            <TextInput
              value={request.negative}
              onChange={(negative) => set({ negative })}
              placeholder={defaults.negative}
              wide
            />
          </Field>
          <Field label={t("op.size")}>
            <NumberInput value={width} onChange={(value) => set({ width: value })} min={64} max={4096} step={8} suffix="×" wide />
            <NumberInput value={height} onChange={(value) => set({ height: value })} min={64} max={4096} step={8} suffix="px" wide />
          </Field>
          <Field label="">
            {SIZES.map(([presetWidth, presetHeight]) => (
              <Button
                key={`${presetWidth}x${presetHeight}`}
                small
                onClick={() => set({ width: presetWidth, height: presetHeight })}
              >
                {presetWidth}×{presetHeight}
              </Button>
            ))}
          </Field>
          <Field label={t("studio.steps")}>
            <Slider value={request.steps || defaults.steps} onChange={(steps) => set({ steps })} min={1} max={150} reset={defaults.steps} />
          </Field>
          <Field label={t("studio.cfg")} hint={t("studio.cfgHint")}>
            <Slider value={request.cfg || defaults.cfg} onChange={(cfg) => set({ cfg })} min={0} max={30} step={0.5} reset={defaults.cfg} />
          </Field>
          <Field label={t("studio.seed")}>
            <Checkbox
              checked={request.seed === null}
              onChange={(random) => set({ seed: random ? null : 1 })}
              label={t("studio.seedRandom")}
            />
            {request.seed === null ? null : (
              <NumberInput value={request.seed} onChange={(seed) => set({ seed })} min={0} max={4294967295} wide />
            )}
          </Field>
          <Field label={t("studio.count")}>
            <NumberInput value={request.count} onChange={(count) => set({ count })} min={1} max={8} />
          </Field>
          <Field label={t("studio.source")} hint={t("studio.sourceHint")}>
            <PathField kind="picture" value={source} onChange={setSource} />
          </Field>
          {source ? (
            <Field label={t("studio.strength")} hint={t("studio.strengthHint")}>
              <Slider value={request.strength} onChange={(strength) => set({ strength })} min={0} max={1} step={0.05} reset={0.6} />
            </Field>
          ) : null}
          <div className="tool-run">
            <Tag tone={ai.generationRemote ? "warning" : "ok"}>
              {td(`ai.provider.${defaults.provider}`)} · {ai.generationRemote ? t("ai.remote") : t("ai.local")}
            </Tag>
            <Button variant="primary" disabled={busy || request.prompt.trim() === ""} onClick={() => void generate()}>
              {busy ? t("common.working") : t("studio.generate")}
            </Button>
          </div>
          {job ? <JobRow job={job} /> : null}
          {ai.advice.slowGeneration && !ai.generationRemote ? <p className="tool-text">{t("studio.slow")}</p> : null}
        </Section>
      </div>
      <div className="column">
        {results.length === 0 ? (
          <EmptyState title={t("studio.emptyTitle")} text={t("studio.emptyText")} />
        ) : (
          <div className="gallery">
            {results.map((path) => (
              <div key={path} className="gallery-item">
                <img
                  src={thumbUrl({ path, modified: 0 }, 512)}
                  alt=""
                  loading="lazy"
                  draggable={false}
                  onDoubleClick={() => openInEditor(path)}
                />
                <div className="gallery-actions">
                  <Button small onClick={() => openInEditor(path)}>
                    {t("viewer.edit")}
                  </Button>
                  <Button
                    small
                    onClick={() => {
                      addToQueue([path]);
                      notify(t("viewer.addedToBatch", { count: 1 }));
                    }}
                  >
                    {t("viewer.toBatch")}
                  </Button>
                  <Button small onClick={() => api.revealPath(path).catch(showError)}>
                    {t("common.reveal")}
                  </Button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function VisionTab() {
  const { t, td, error } = useT();
  const { ai, jobs, jobResult, navigate, notify, showError, withCloud } = useApp();
  const [files, setFiles] = useState<string[]>([]);
  const [task, setTask] = useState<VisionTask>("describe");
  const [apply, setApply] = useState(false);
  const [jobId, setJobId] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<VisionResult | null>(null);

  if (!ai) {
    return null;
  }
  if (!ai.capabilities.vision) {
    return (
      <EmptyState
        title={t("vision.noModelTitle")}
        text={t("vision.noModelText")}
        action={
          <Button variant="primary" onClick={() => navigate("ai")}>
            {t("studio.setUp")}
          </Button>
        }
      />
    );
  }

  const job = jobId === null ? undefined : jobs.find((item) => item.id === jobId);
  const canApply = task !== "ocr";

  const start = async () => {
    setBusy(true);
    setResult(null);
    try {
      const id = await withCloud((confirmed) => api.visionStart(files, task, apply && canApply, confirmed));
      setJobId(id);
      setResult(await jobResult<VisionResult>(id));
    } catch (failure) {
      showError(failure);
    } finally {
      setBusy(false);
      setJobId(null);
    }
  };

  return (
    <>
      <Section>
        <FilesField files={files} onChange={setFiles} />
        <Field label={t("vision.task")} hint={td(`vision.about.${task}`)}>
          <Select
            value={task}
            options={TASKS.map((value) => ({ value, label: td(`vision.taskKind.${value}`) }))}
            onChange={setTask}
          />
        </Field>
        {canApply ? (
          <Field label="" hint={t("vision.applyHint")}>
            <Checkbox
              checked={apply}
              onChange={setApply}
              label={task === "name" ? t("vision.applyRename") : t("vision.applyMetadata")}
            />
          </Field>
        ) : null}
        <div className="tool-run">
          <Tag tone={ai.visionRemote ? "warning" : "ok"}>
            {ai.settings.vision.model} · {ai.visionRemote ? t("ai.remote") : t("ai.local")}
          </Tag>
          <Button variant="primary" disabled={busy || files.length === 0} onClick={() => void start()}>
            {busy ? t("common.working") : t("vision.start")}
          </Button>
        </div>
        {job ? <JobRow job={job} /> : null}
      </Section>
      {result ? (
        <Section title={t("tools.result")}>
          {result.answers.map((item) => (
            <div key={item.source} className="answer">
              <img src={thumbUrl({ path: item.answer?.path ?? item.source, modified: 0 }, 256)} alt="" loading="lazy" draggable={false} />
              <div className="answer-body">
                <div className="answer-name">
                  {fileName(item.answer?.path ?? item.source)}
                  {item.answer?.applied ? <Tag tone="ok">{t("vision.applied")}</Tag> : null}
                </div>
                {item.error ? (
                  <div className="report-error">{error(item.error)}</div>
                ) : (
                  <div className="answer-text">{item.answer?.text}</div>
                )}
              </div>
              {item.answer ? (
                <Button
                  small
                  onClick={() =>
                    void copyText(item.answer?.text ?? "").then((copied) => copied && notify(t("common.copied")))
                  }
                >
                  {t("common.copy")}
                </Button>
              ) : null}
            </div>
          ))}
        </Section>
      ) : null}
    </>
  );
}

export default function StudioPage() {
  const { t } = useT();
  const [tab, setTab] = useState<"generate" | "vision">("generate");
  return (
    <PageFrame title={t("nav.studio")}>
      <Tabs
        value={tab}
        options={[
          { value: "generate", label: t("studio.tabGenerate") },
          { value: "vision", label: t("studio.tabVision") },
        ]}
        onChange={setTab}
      />
      <div hidden={tab !== "generate"}>
        <GenerateTab />
      </div>
      <div className="column" hidden={tab !== "vision"}>
        <VisionTab />
      </div>
    </PageFrame>
  );
}
