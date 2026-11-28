// The upscaler, and whether enlarging also paints detail back in.
// The same settings serve the generate form and the buttons on a result.

import { useEffect, useState } from "react";
import { api } from "../../api";
import { useApp } from "../../app/context";
import { useT } from "../../i18n";
import type { Generation, UpscaleEngine, UpscaleNotice, Upscaler } from "../../types";
import { Button, Field, Modal, Segmented, Select, Slider } from "../../ui";

function engineMark(engine: UpscaleEngine): string {
  switch (engine) {
    case "builtin":
    case "file":
      return "";
    case "onnx":
      return "ONNX";
    case "comfy":
      return "ComfyUI";
    case "forge":
      return "Forge";
    default: {
      const unreachable: never = engine;
      return unreachable;
    }
  }
}

function bare(id: string): string {
  const colon = id.indexOf(":");
  return colon === -1 ? id : id.slice(colon + 1);
}

export function UpscaleFields({
  generation,
  refresh = 0,
  onChange,
}: {
  generation: Generation;
  /** A new value asks for the list again: a generator that has started brings its own models. */
  refresh?: number;
  onChange: (patch: Partial<Generation>) => void;
}) {
  const { t } = useT();
  const [models, setModels] = useState<Upscaler[]>([]);
  const [busy, setBusy] = useState(false);

  const load = () => {
    setBusy(true);
    api
      .upscalersList()
      .then(setModels)
      .catch(() => setModels([]))
      .finally(() => setBusy(false));
  };

  useEffect(() => {
    load();
  }, [refresh]);

  const selected = generation.upscaler || "builtin";
  const known = models.some((item) => item.id === selected);
  const options = [
    ...(known || selected === "builtin" ? [] : [{ value: selected, label: bare(selected), disabled: false }]),
    ...models.map((item) => {
      const mark = engineMark(item.engine);
      const name = mark ? `${item.name} · ${mark}` : item.name;
      const blocked = !item.available && item.engine !== "builtin";
      return {
        value: item.id,
        label: blocked ? `${name} — ${t("studio.upscalerNeedsServer")}` : name,
        disabled: blocked,
      };
    }),
  ];
  if (options.length === 0) {
    options.push({ value: "builtin", label: t("studio.upscalerBuiltin"), disabled: false });
  }
  const refine = generation.upscaleRefine !== false;

  return (
    <>
      <Field label={t("studio.upscaler")} hint={known || selected === "builtin" ? undefined : t("studio.upscalerMissing")}>
        <Select
          value={options.some((item) => item.value === selected) ? selected : "builtin"}
          options={options}
          disabled={busy && models.length === 0}
          onChange={(upscaler) => onChange({ upscaler: upscaler === "builtin" ? "" : upscaler })}
        />
        <Button small disabled={busy} onClick={load}>
          {t("studio.refreshModels")}
        </Button>
      </Field>
      <Field label={t("studio.upscaleMode")} hint={refine ? t("studio.upscaleRefineHint") : t("studio.upscaleFastHint")}>
        <Segmented
          value={refine ? "refine" : "fast"}
          options={[
            { value: "fast", label: t("studio.upscaleFast") },
            { value: "refine", label: t("studio.upscaleRefine") },
          ]}
          onChange={(value) => onChange({ upscaleRefine: value === "refine" })}
        />
      </Field>
      {refine ? (
        <Field label={t("studio.upscaleStrength")} hint={t("studio.upscaleStrengthHint")}>
          <Slider
            value={generation.upscaleStrength || 0.35}
            min={0.05}
            max={1}
            step={0.05}
            reset={0.35}
            onChange={(upscaleStrength) => onChange({ upscaleStrength })}
          />
        </Field>
      ) : null}
    </>
  );
}

/** The same fields, opened from a result instead of the generate form. */
export function UpscaleMenu() {
  const { t } = useT();
  const { ai, refreshAi, showError } = useApp();
  const [open, setOpen] = useState(false);
  if (!ai) {
    return null;
  }
  const generation = ai.settings.generation;
  const save = (patch: Partial<Generation>) => {
    void api
      .aiSettingsSet({ ...ai.settings, generation: { ...generation, ...patch } })
      .then(() => refreshAi())
      .catch(showError);
  };
  return (
    <>
      <Button small onClick={() => setOpen(true)}>
        {t("studio.upscaleOptions")}
      </Button>
      {open ? (
        <Modal title={t("studio.upscaleOptions")} onClose={() => setOpen(false)}>
          <UpscaleFields generation={generation} onChange={save} />
        </Modal>
      ) : null}
    </>
  );
}

export function tellUpscale(
  notice: UpscaleNotice | undefined,
  t: (key: "studio.noticeNeedsGenerator" | "studio.noticeNoPrompt" | "studio.noticeRefineFailed") => string,
  notify: (text: string) => void,
) {
  const code = notice ?? "";
  switch (code) {
    case "":
      return;
    case "needsGenerator":
      notify(t("studio.noticeNeedsGenerator"));
      return;
    case "noPrompt":
      notify(t("studio.noticeNoPrompt"));
      return;
    case "refineFailed":
      notify(t("studio.noticeRefineFailed"));
      return;
    default: {
      const unreachable: never = code;
      return unreachable;
    }
  }
}
