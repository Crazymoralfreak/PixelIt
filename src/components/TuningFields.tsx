// The sampler, its schedule, a separate VAE, LoRA files and embeddings.
// ComfyUI and Forge take these; the lists are the ones the running generator
// gives. Shared by the studio and the "AI detail" step.

import { useEffect, useState } from "react";
import { api } from "../api";
import { useT } from "../i18n";
import { modelTitle, NO_TUNING, takesTuning, tuningEmpty } from "../lib/generation";
import type { Embedding, GenerationOptions, GenerationProvider, Lora, Tuning } from "../types";
import { Button, Field, Section, Select, Slider } from "../ui";

const MAX_LORAS = 8;
const MAX_EMBEDDINGS = 8;

/** Where the word of an embedding is written. */
type Side = "prompt" | "negative";

/** The names of a list, and the chosen one even when the list does not have it. */
function withChosen(names: readonly string[], chosen: string): string[] {
  return chosen === "" || names.includes(chosen) ? [...names] : [chosen, ...names];
}

/** Most embeddings say what a picture must not have, and their files are named so. */
function soundsNegative(name: string): boolean {
  return /neg|bad|worst|ugly/i.test(modelTitle(name));
}

export function TuningFields({
  provider,
  tuning,
  refresh = 0,
  onChange,
}: {
  provider: GenerationProvider;
  tuning: Tuning;
  /** A new value asks the generator for its lists again. */
  refresh?: number;
  onChange: (tuning: Tuning) => void;
}) {
  const { t } = useT();
  const [options, setOptions] = useState<GenerationOptions | null>(null);
  const [silent, setSilent] = useState(false);
  const [busy, setBusy] = useState(false);
  const wanted = takesTuning(provider);

  const load = () => {
    if (!wanted) {
      setOptions(null);
      setSilent(false);
      return;
    }
    setBusy(true);
    api
      .generationOptions(provider)
      .then((found) => {
        setOptions(found);
        setSilent(false);
      })
      .catch(() => {
        setOptions(null);
        setSilent(true);
      })
      .finally(() => setBusy(false));
  };

  useEffect(() => {
    load();
    // Asked again for another generator, and when the host says it has started.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [provider, refresh]);

  // A workflow of the user's own decides all of this itself.
  if (!wanted || (options !== null && !options.supported)) {
    return null;
  }

  const samplers = withChosen(options?.samplers ?? [], tuning.sampler);
  const schedulers = withChosen(options?.schedulers ?? [], tuning.scheduler);
  const vaes = withChosen(options?.vaes ?? [], tuning.vae);
  const freeLoras = (options?.loras ?? []).filter((name) => !tuning.loras.some((lora) => lora.name === name));
  const freeEmbeddings = (options?.embeddings ?? []).filter(
    (name) => !tuning.embeddings.some((embedding) => embedding.name === name),
  );
  const usual = (name: string) => (name ? t("studio.tuningUsualNamed", { name }) : t("studio.tuningUsual"));
  const setLora = (index: number, patch: Partial<Lora>) => {
    onChange({ ...tuning, loras: tuning.loras.map((lora, at) => (at === index ? { ...lora, ...patch } : lora)) });
  };
  const setEmbedding = (index: number, patch: Partial<Embedding>) => {
    onChange({
      ...tuning,
      embeddings: tuning.embeddings.map((embedding, at) => (at === index ? { ...embedding, ...patch } : embedding)),
    });
  };
  const sides: Array<{ value: Side; label: string }> = [
    { value: "prompt", label: t("studio.embeddingPrompt") },
    { value: "negative", label: t("studio.embeddingNegative") },
  ];
  const summary = tuningEmpty(tuning)
    ? undefined
    : [
        tuning.sampler,
        tuning.scheduler,
        tuning.vae ? "VAE" : "",
        tuning.loras.length > 0 ? `LoRA × ${tuning.loras.length}` : "",
        tuning.embeddings.length > 0 ? `${t("studio.embedding")} × ${tuning.embeddings.length}` : "",
      ]
        .filter((part) => part !== "")
        .join(" · ");

  return (
    <Section title={t("studio.tuning")} collapsible defaultOpen={!tuningEmpty(tuning)} summary={summary}>
      {silent ? <p className="tool-text">{t("studio.tuningSilent")}</p> : null}
      <Field label={t("studio.sampler")} hint={t("studio.samplerHint")}>
        <Select
          value={tuning.sampler}
          options={[{ value: "", label: usual(options?.sampler ?? "") }, ...samplers.map((name) => ({ value: name, label: name }))]}
          onChange={(sampler) => onChange({ ...tuning, sampler })}
        />
        <Button small disabled={busy} onClick={load}>
          {t("studio.refreshModels")}
        </Button>
      </Field>
      {schedulers.length > 0 ? (
        <Field label={t("studio.scheduler")}>
          <Select
            value={tuning.scheduler}
            options={[{ value: "", label: usual(options?.scheduler ?? "") }, ...schedulers.map((name) => ({ value: name, label: name }))]}
            onChange={(scheduler) => onChange({ ...tuning, scheduler })}
          />
        </Field>
      ) : null}
      {vaes.length > 0 ? (
        <Field label={t("studio.vae")} hint={t("studio.vaeHint")}>
          <Select
            value={tuning.vae}
            options={[{ value: "", label: t("studio.vaeOfModel") }, ...vaes.map((name) => ({ value: name, label: modelTitle(name) }))]}
            onChange={(vae) => onChange({ ...tuning, vae })}
          />
        </Field>
      ) : null}
      <Field label={t("studio.lora")} hint={tuning.loras.length > 0 ? t("studio.loraHint") : undefined}>
        <div className="lora-list">
          {tuning.loras.map((lora, index) => (
            <div key={lora.name} className="lora-row">
              <span className="lora-name" title={lora.name}>
                {modelTitle(lora.name)}
              </span>
              <Slider value={lora.weight} min={-1} max={2} step={0.05} reset={1} onChange={(weight) => setLora(index, { weight })} />
              <Button
                small
                title={t("common.remove")}
                aria-label={t("common.remove")}
                onClick={() => onChange({ ...tuning, loras: tuning.loras.filter((_, at) => at !== index) })}
              >
                ×
              </Button>
            </div>
          ))}
          {tuning.loras.length < MAX_LORAS ? (
            <Select
              value=""
              disabled={freeLoras.length === 0}
              options={[
                { value: "", label: freeLoras.length === 0 && !silent ? t("studio.loraNone") : t("studio.loraAdd") },
                ...freeLoras.map((name) => ({ value: name, label: modelTitle(name) })),
              ]}
              onChange={(name) => name !== "" && onChange({ ...tuning, loras: [...tuning.loras, { name, weight: 1 }] })}
            />
          ) : null}
        </div>
      </Field>
      <Field label={t("studio.embedding")} hint={t("studio.embeddingHint")}>
        <div className="lora-list">
          {tuning.embeddings.map((embedding, index) => (
            <div key={embedding.name} className="lora-row embedding-row">
              <span className="lora-name" title={embedding.name}>
                {modelTitle(embedding.name)}
              </span>
              <Select<Side>
                value={embedding.negative ? "negative" : "prompt"}
                options={sides}
                onChange={(side) => setEmbedding(index, { negative: side === "negative" })}
              />
              <Slider value={embedding.weight} min={0.1} max={2} step={0.05} reset={1} onChange={(weight) => setEmbedding(index, { weight })} />
              <Button
                small
                title={t("common.remove")}
                aria-label={t("common.remove")}
                onClick={() => onChange({ ...tuning, embeddings: tuning.embeddings.filter((_, at) => at !== index) })}
              >
                ×
              </Button>
            </div>
          ))}
          {tuning.embeddings.length < MAX_EMBEDDINGS ? (
            <Select
              value=""
              disabled={freeEmbeddings.length === 0}
              options={[
                { value: "", label: freeEmbeddings.length === 0 && !silent ? t("studio.embeddingNone") : t("studio.loraAdd") },
                ...freeEmbeddings.map((name) => ({ value: name, label: modelTitle(name) })),
              ]}
              onChange={(name) =>
                name !== "" &&
                onChange({ ...tuning, embeddings: [...tuning.embeddings, { name, weight: 1, negative: soundsNegative(name) }] })
              }
            />
          ) : null}
        </div>
      </Field>
      {tuningEmpty(tuning) ? null : (
        <div className="tool-run">
          <Button small onClick={() => onChange(NO_TUNING)}>
            {t("studio.tuningReset")}
          </Button>
        </div>
      )}
    </Section>
  );
}
