// The generator and its model, shared by the studio and the AI-edit step.
// An empty provider means "whatever the settings say".

import { useEffect, useState } from "react";
import { api } from "../api";
import { useT } from "../i18n";
import type { GenerationModel, GenerationProvider } from "../types";
import { Button, Field, Select } from "../ui";

const PROVIDERS: readonly GenerationProvider[] = ["comfy", "forge", "sdCpp", "cloud"];

export function GeneratorPicker({
  provider,
  model,
  allowDefault,
  onChange,
}: {
  provider: GenerationProvider | "";
  model: string;
  /** The step can stay on the generator from the settings. */
  allowDefault?: boolean;
  onChange: (provider: GenerationProvider | "", model: string) => void;
}) {
  const { t, td } = useT();
  const [models, setModels] = useState<GenerationModel[]>([]);
  const [selectable, setSelectable] = useState(true);
  const [busy, setBusy] = useState(false);

  const load = (next: GenerationProvider | "") => {
    if (next === "") {
      setModels([]);
      setSelectable(true);
      return;
    }
    setBusy(true);
    api
      .generationModels(next)
      .then((list) => {
        setModels(list.models);
        setSelectable(list.selectable);
      })
      .catch(() => {
        setModels([]);
        setSelectable(true);
      })
      .finally(() => setBusy(false));
  };

  useEffect(() => {
    load(provider);
  }, [provider]);

  const providerOptions = [
    ...(allowDefault ? [{ value: "" as const, label: t("op.editModelDefault") }] : []),
    ...PROVIDERS.map((value) => ({ value, label: td(`ai.provider.${value}`) })),
  ];

  const modelOptions = [
    { value: "", label: selectable ? t("studio.modelCurrent") : t("studio.modelLocked") },
    ...models.map((item) => ({ value: item.id, label: item.name })),
  ];

  return (
    <>
      <Field label={t("studio.provider")}>
        <Select
          value={provider}
          options={providerOptions}
          onChange={(next) => {
            onChange(next, "");
          }}
        />
      </Field>
      <Field label={t("studio.model")} hint={selectable ? undefined : t("studio.modelLockedHint")}>
        <Select
          value={models.some((item) => item.id === model) ? model : ""}
          options={modelOptions}
          disabled={provider === "" || !selectable || busy}
          onChange={(next) => onChange(provider, next)}
        />
        <Button small disabled={provider === "" || busy} onClick={() => load(provider)}>
          {t("studio.refreshModels")}
        </Button>
      </Field>
    </>
  );
}
