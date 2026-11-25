// The generation form: the generator, the prompt, the styles, and either a
// new picture or a change of one that is already here.

import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { useAppEvent } from "../../api/events";
import { useApp } from "../../app/context";
import { GeneratorPicker } from "../../components/GeneratorPicker";
import { JobRow } from "../../components/JobsPanel";
import { PathField } from "../../components/PathField";
import { useT } from "../../i18n";
import { copyText } from "../../lib/clipboard";
import type { Expand, GeneratePreview, GenerateRequest, GeneratedRecord, GenerationProvider, Stroke } from "../../types";
import { Button, EmptyState, Field, Section, Segmented, Slider, Tag, TextArea } from "../../ui";
import { Advanced } from "./Advanced";
import { EditSource } from "./EditSource";
import { ASPECTS, NO_EXPAND, applyStyles, defaultsKey, expandEmpty, fitRatio, modelOf, withModel } from "./format";
import type { StudioHandoff } from "./handoff";
import { PictureDialog, type PictureItem } from "./PictureDialog";
import { Results } from "./Results";
import { StyleBar, useStyles } from "./Styles";

const CLOUD_SIZES: ReadonlyArray<readonly [number, number, number, number]> = [
  [1, 1, 1024, 1024],
  [3, 2, 1536, 1024],
  [2, 3, 1024, 1536],
  [16, 9, 1792, 1024],
  [9, 16, 1024, 1792],
];

function AutoRun({ token, onRun }: { token: number; onRun: () => void }) {
  const seen = useRef(0);
  useEffect(() => {
    if (token === 0 || token === seen.current) {
      return;
    }
    seen.current = token;
    onRun();
  }, [token, onRun]);
  return null;
}

export function Generate({ handoff }: { handoff: StudioHandoff | null }) {
  const { t, td } = useT();
  const { ai, jobs, jobResult, navigate, openInEditor, addToQueue, notify, showError, withCloud, refreshAi } = useApp();
  const { styles, reload } = useStyles();
  const generation = ai?.settings.generation;
  const [mode, setMode] = useState<"create" | "edit">("create");
  const [request, setRequest] = useState<GenerateRequest | null>(null);
  const [source, setSource] = useState("");
  const [strokes, setStrokes] = useState<Stroke[]>([]);
  const [expand, setExpand] = useState<Expand>(NO_EXPAND);
  const [previousPrompt, setPreviousPrompt] = useState<string | null>(null);
  const [showFinal, setShowFinal] = useState(false);
  const [jobId, setJobId] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [previews, setPreviews] = useState<GeneratePreview[]>([]);
  const [record, setRecord] = useState<GeneratedRecord | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const [seen, setSeen] = useState(0);
  const [launch, setLaunch] = useState<{ token: number; patch: Partial<GenerateRequest>; source: string | null } | null>(null);

  useEffect(() => {
    if (!generation || request) {
      return;
    }
    setRequest({
      prompt: "",
      negative: generation.negative,
      width: generation.width,
      height: generation.height,
      steps: generation.steps,
      cfg: generation.cfg,
      seed: null,
      count: 1,
      strength: 0.6,
      provider: generation.provider,
      model: modelOf(generation),
      styles: generation.styles,
    });
  }, [generation, request]);

  useEffect(() => {
    if (!handoff || handoff.token === seen || !request) {
      return;
    }
    setSeen(handoff.token);
    switch (handoff.kind) {
      case "repeat":
        setMode("create");
        setRequest({
          ...request,
          prompt: handoff.record.prompt,
          negative: handoff.record.negative,
          width: handoff.record.width || request.width,
          height: handoff.record.height || request.height,
          steps: handoff.record.steps || request.steps,
          cfg: handoff.record.cfg || request.cfg,
          seed: null,
          styles: handoff.record.styles,
          provider: handoff.record.provider === "none" ? request.provider : handoff.record.provider,
          model: handoff.record.model,
        });
        break;
      case "seed":
        setRequest({ ...request, seed: handoff.seed });
        break;
      case "edit":
        setMode("edit");
        setSource(handoff.path);
        setStrokes([]);
        setExpand(NO_EXPAND);
        break;
      case "similar":
        setMode("edit");
        setSource(handoff.path);
        setStrokes([]);
        setExpand(NO_EXPAND);
        setRequest({ ...request, prompt: handoff.record.prompt, strength: 0.35, seed: null, count: 1 });
        setLaunch({
          token: handoff.token,
          patch: { prompt: handoff.record.prompt, strength: 0.35, seed: null, count: 1 },
          source: handoff.path,
        });
        break;
      case "variants":
        setMode("create");
        setRequest({
          ...request,
          prompt: handoff.record.prompt,
          negative: handoff.record.negative,
          seed: null,
          count: 1,
          styles: handoff.record.styles,
        });
        setLaunch({
          token: handoff.token,
          patch: { prompt: handoff.record.prompt, negative: handoff.record.negative, seed: null, count: 1, styles: handoff.record.styles },
          source: null,
        });
        break;
      case "drop":
        setMode("create");
        void api
          .pictureParameters(handoff.path)
          .then((parameters) => {
            if (!parameters) {
              setMode("edit");
              setSource(handoff.path);
              return;
            }
            setRequest((current) =>
              current
                ? {
                    ...current,
                    prompt: parameters.prompt,
                    negative: parameters.negative,
                    width: parameters.width,
                    height: parameters.height,
                    steps: parameters.steps || current.steps,
                    cfg: parameters.cfg || current.cfg,
                    seed: parameters.seed,
                  }
                : current,
            );
          })
          .catch(() => {
            setMode("edit");
            setSource(handoff.path);
          });
        break;
      default: {
        const unreachable: never = handoff;
        throw new Error(`Unknown handoff: ${String(unreachable)}`);
      }
    }
  }, [handoff, request, seen]);

  useAppEvent("generate:preview", (preview) => {
    setPreviews((current) => {
      const rest = current.filter((item) => item.index !== preview.index);
      return [...rest, preview];
    });
  });

  if (!ai || !generation || !request) {
    return null;
  }
  if (!ai.capabilities.generation || generation.provider === "none") {
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

  const provider: GenerationProvider = request.provider === "none" ? generation.provider : request.provider;
  const cloud = provider === "cloud";
  const width = request.width || generation.width;
  const height = request.height || generation.height;
  const steps = request.steps || generation.steps;
  const cfg = request.cfg || generation.cfg;
  const chosenStyles = request.styles
    .map((id) => styles.find((style) => style.id === id))
    .filter((style): style is NonNullable<typeof style> => style !== undefined);
  const [finalPrompt] = applyStyles(request.prompt, request.negative || generation.negative, chosenStyles);
  const job = jobId === null ? undefined : jobs.find((item) => item.id === jobId);
  const items: PictureItem[] = record ? record.pictures.map((picture) => ({ record, picture })) : [];

  const set = (patch: Partial<GenerateRequest>) => setRequest({ ...request, ...patch });

  const rememberModel = async (nextProvider: GenerationProvider, nextModel: string, patch: Partial<GenerateRequest> = {}) => {
    const kept = nextModel || modelOf({ ...generation, provider: nextProvider });
    const currentKey = defaultsKey(provider, request.model || modelOf(generation));
    const nextKey = defaultsKey(nextProvider, kept);
    const recalled = generation.modelDefaults[nextKey];
    const nextGeneration = withModel(
      {
        ...generation,
        width: recalled?.width ?? width,
        height: recalled?.height ?? height,
        steps: recalled?.steps ?? steps,
        cfg: recalled?.cfg ?? cfg,
        modelDefaults: {
          ...generation.modelDefaults,
          [currentKey]: { width, height, steps, cfg },
        },
      },
      nextProvider,
      kept,
    );
    set({
      ...patch,
      provider: nextProvider,
      model: kept,
      width: nextGeneration.width,
      height: nextGeneration.height,
      steps: nextGeneration.steps,
      cfg: nextGeneration.cfg,
    });
    try {
      await api.aiSettingsSet({ ...ai.settings, generation: nextGeneration });
      refreshAi();
    } catch (error) {
      showError(error);
    }
  };

  const run = async (override?: Partial<GenerateRequest>, sourceOverride?: string | null) => {
    const next = { ...request, width, height, steps, cfg, ...override };
    const usedSource = sourceOverride === undefined ? (mode === "edit" ? source : "") : sourceOverride;
    if (next.prompt.trim() === "") {
      return;
    }
    setRequest(next);
    setBusy(true);
    setPreviews([]);
    setRecord(null);
    try {
      const key = defaultsKey(next.provider === "none" ? provider : next.provider, next.model);
      await api.aiSettingsSet({
        ...ai.settings,
        generation: {
          ...generation,
          width: next.width,
          height: next.height,
          steps: next.steps,
          cfg: next.cfg,
          styles: next.styles,
          modelDefaults: {
            ...generation.modelDefaults,
            [key]: { width: next.width, height: next.height, steps: next.steps, cfg: next.cfg },
          },
        },
      });
      const id = await withCloud((confirmed) =>
        api.generateStart(
          next,
          usedSource || null,
          mode === "edit" && sourceOverride === undefined ? strokes : [],
          mode === "edit" && sourceOverride === undefined && !expandEmpty(expand) ? expand : NO_EXPAND,
          confirmed,
        ),
      );
      setJobId(id);
      setRecord(await jobResult<GeneratedRecord>(id));
    } catch (error) {
      showError(error);
    } finally {
      setBusy(false);
      setJobId(null);
      setPreviews([]);
    }
  };

  const improve = async () => {
    try {
      const improved = await withCloud((confirmed) => api.promptImprove(request.prompt, confirmed));
      setPreviousPrompt(request.prompt);
      set({ prompt: improved });
    } catch (error) {
      showError(error);
    }
  };

  const saveTo = async (files: string[]) => {
    const folder = await api.pickFolder();
    if (!folder) {
      return;
    }
    try {
      const written = await api.generatedExport(files, folder);
      notify(t("studio.saved", { count: written.length }));
    } catch (error) {
      showError(error);
    }
  };

  return (
    <div className="columns">
      <div className="column">
        <Section>
          <Segmented
            value={mode}
            options={[
              { value: "create", label: t("studio.modeCreate") },
              { value: "edit", label: t("studio.modeEdit") },
            ]}
            onChange={setMode}
          />
          <GeneratorPicker
            provider={provider}
            model={request.model}
            onChange={(nextProvider, nextModel) => {
              if (nextProvider === "") {
                return;
              }
              void rememberModel(nextProvider, nextModel);
            }}
          />
          <Field label={mode === "edit" ? t("studio.editPrompt") : t("studio.prompt")}>
            <TextArea
              value={request.prompt}
              onChange={(prompt) => {
                setPreviousPrompt(null);
                set({ prompt });
              }}
              rows={4}
              placeholder={t("studio.promptHint")}
            />
          </Field>
          <div className="tool-run">
            <Button small disabled={!ai.capabilities.assistant || request.prompt.trim() === ""} title={ai.capabilities.assistant ? undefined : t("studio.improveMissing")} onClick={() => void improve()}>
              {t("studio.improve")}
            </Button>
            {previousPrompt !== null ? (
              <Button
                small
                onClick={() => {
                  set({ prompt: previousPrompt });
                  setPreviousPrompt(null);
                }}
              >
                {t("studio.improveUndo")}
              </Button>
            ) : null}
          </div>
          <StyleBar
            selected={request.styles}
            styles={styles}
            showFinal={showFinal}
            finalText={finalPrompt}
            onToggle={(id) => {
              const stylesOn = request.styles.includes(id) ? request.styles.filter((item) => item !== id) : [...request.styles, id];
              set({ styles: stylesOn });
            }}
            onShowFinal={setShowFinal}
            onChanged={reload}
          />
          <Field label={t("studio.aspect")}>
            {(cloud ? CLOUD_SIZES : ASPECTS.map(([ratioW, ratioH]) => [ratioW, ratioH, ...fitRatio(width, height, ratioW, ratioH)] as const)).map(
              ([ratioW, ratioH, nextWidth, nextHeight]) => (
                <Button
                  key={`${ratioW}:${ratioH}`}
                  small
                  variant={nextWidth === width && nextHeight === height ? "primary" : "default"}
                  onClick={() => set({ width: nextWidth, height: nextHeight })}
                >
                  {ratioW}:{ratioH}
                </Button>
              ),
            )}
          </Field>
          {cloud ? <p className="tool-text">{t("studio.cloudSizeHint")}</p> : null}
          <Field label={t("studio.count")}>
            <Segmented
              value={String(Math.min(4, Math.max(1, request.count)))}
              options={[1, 2, 3, 4].map((count) => ({ value: String(count), label: String(count) }))}
              onChange={(count) => set({ count: Number(count) })}
            />
          </Field>
          {mode === "edit" ? (
            <>
              <Field label={t("studio.source")} hint={t("studio.sourceHint")}>
                <PathField kind="picture" value={source} onChange={setSource} />
              </Field>
              {source ? (
                <EditSource path={source} strokes={strokes} expand={expand} onStrokes={setStrokes} onExpand={setExpand} />
              ) : null}
              <Field label={t("studio.strength")} hint={t("studio.strengthHint")}>
                <Slider value={request.strength} onChange={(strength) => set({ strength })} min={0} max={1} step={0.05} reset={0.6} />
              </Field>
            </>
          ) : null}
          <Advanced
            request={request}
            width={width}
            height={height}
            steps={steps}
            cfg={cfg}
            defaults={generation}
            embed={generation.embedParameters}
            onRequest={set}
            onSize={(nextWidth, nextHeight) => set({ width: nextWidth, height: nextHeight })}
            onEmbed={(embedParameters) => {
              void api
                .aiSettingsSet({ ...ai.settings, generation: { ...generation, embedParameters } })
                .then(() => refreshAi())
                .catch(showError);
            }}
          />
          <div className="tool-run">
            <Tag tone={cloud ? "warning" : "ok"}>
              {td(`ai.provider.${provider}`)} · {cloud ? t("ai.remote") : t("ai.local")}
            </Tag>
            <Button variant="primary" disabled={busy || request.prompt.trim() === "" || (mode === "edit" && source === "")} onClick={() => void run()}>
              {busy ? t("common.working") : mode === "edit" ? t("studio.editRun") : t("studio.generate")}
            </Button>
          </div>
          {job ? <JobRow job={job} /> : null}
          {ai.advice.slowGeneration && !cloud ? <p className="tool-text">{t("studio.slow")}</p> : null}
        </Section>
      </div>
      <div className="column">
        {record || busy ? (
          <Results
            record={record}
            previews={previews}
            busy={busy}
            count={request.count}
            upscaleReady={ai.capabilities.upscale}
            onOpen={setOpen}
            onVariants={(item) => void run({ prompt: item.prompt, negative: item.negative, seed: null, count: 1, styles: item.styles }, null)}
            onSimilar={(item, file) => {
              setMode("edit");
              setSource(file);
              void run({ prompt: item.prompt, strength: 0.35, seed: null, count: 1 }, file);
            }}
            onEdit={(path) => {
              setMode("edit");
              setSource(path);
              setStrokes([]);
              setExpand(NO_EXPAND);
            }}
            onUpscale={(file, factor) => {
              void api
                .generatedUpscale(file, factor)
                .then((id) => jobResult<GeneratedRecord>(id))
                .then(setRecord)
                .catch(showError);
            }}
            onEditor={openInEditor}
            onBatch={(files) => {
              addToQueue(files);
              notify(t("viewer.addedToBatch", { count: files.length }));
            }}
            onReveal={(file) => void api.revealPath(file).catch(showError)}
          />
        ) : (
          <EmptyState title={t("studio.emptyTitle")} text={t("studio.emptyText")} />
        )}
      </div>
      {open !== null && record ? (
        <PictureDialog
          items={items}
          index={open}
          upscaleReady={ai.capabilities.upscale}
          onIndex={setOpen}
          onClose={() => setOpen(null)}
          onRepeat={(item) => set({ prompt: item.prompt, negative: item.negative, styles: item.styles, seed: null })}
          onSeed={(seed) => set({ seed })}
          onVariants={(item) => void run({ prompt: item.prompt, seed: null, count: 1 }, null)}
          onSimilar={(item, file) => void run({ prompt: item.prompt, strength: 0.35, seed: null, count: 1 }, file)}
          onEdit={(path) => {
            setMode("edit");
            setSource(path);
            setStrokes([]);
            setExpand(NO_EXPAND);
          }}
          onUpscale={(file, factor) => {
            void api
              .generatedUpscale(file, factor)
              .then((id) => jobResult<GeneratedRecord>(id))
              .then(setRecord)
              .catch(showError);
          }}
          onEditor={openInEditor}
          onBatch={(files) => {
            addToQueue(files);
            notify(t("viewer.addedToBatch", { count: files.length }));
          }}
          onReveal={(file) => void api.revealPath(file).catch(showError)}
          onCopy={(text) => void copyText(text).then((copied) => copied && notify(t("common.copied")))}
          onSave={(files) => void saveTo(files)}
          onDelete={(files) => {
            void api.generatedDelete(files).then(() => setRecord(null)).catch(showError);
          }}
          onFavorite={(file, on) => {
            void api.generatedFavorite(file, on).catch(showError);
          }}
        />
      ) : null}
      <AutoRun token={launch?.token ?? 0} onRun={() => launch && void run(launch.patch, launch.source)} />
    </div>
  );
}
