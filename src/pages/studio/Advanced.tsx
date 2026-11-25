// The folded part of the generation form: the negative prompt, the exact
// size, the sampler and the seed.

import { useT } from "../../i18n";
import type { GenerateRequest } from "../../types";
import { Button, Checkbox, Field, NumberInput, Section, Slider, TextInput } from "../../ui";

export function Advanced({
  request,
  width,
  height,
  steps,
  cfg,
  defaults,
  embed,
  onRequest,
  onSize,
  onEmbed,
}: {
  request: GenerateRequest;
  width: number;
  height: number;
  steps: number;
  cfg: number;
  defaults: { steps: number; cfg: number };
  embed: boolean;
  onRequest: (patch: Partial<GenerateRequest>) => void;
  onSize: (width: number, height: number) => void;
  onEmbed: (embed: boolean) => void;
}) {
  const { t } = useT();
  const pin = () => {
    const seed = request.seed ?? Math.floor(Math.random() * 0xffffffff);
    onRequest({ seed });
  };

  return (
    <Section title={t("studio.advanced")} collapsible defaultOpen={false}>
      <Field label={t("studio.negative")}>
        <TextInput value={request.negative} onChange={(negative) => onRequest({ negative })} wide />
      </Field>
      <Field label={t("op.size")}>
        <NumberInput value={width} onChange={(value) => onSize(value, height)} min={64} max={4096} step={64} suffix="×" wide />
        <NumberInput value={height} onChange={(value) => onSize(width, value)} min={64} max={4096} step={64} suffix="px" wide />
      </Field>
      <Field label={t("studio.steps")}>
        <Slider value={steps} onChange={(value) => onRequest({ steps: value })} min={1} max={150} reset={defaults.steps} />
      </Field>
      <Field label={t("studio.cfg")} hint={t("studio.cfgHint")}>
        <Slider value={cfg} onChange={(value) => onRequest({ cfg: value })} min={0} max={30} step={0.5} reset={defaults.cfg} />
      </Field>
      <Field label={t("studio.seed")}>
        <Button small variant={request.seed === null ? "primary" : "default"} onClick={() => onRequest({ seed: null })}>
          {t("studio.seedRandom")}
        </Button>
        <Button small variant={request.seed === null ? "default" : "primary"} onClick={pin}>
          {t("studio.seedPin")}
        </Button>
        {request.seed === null ? null : (
          <NumberInput value={request.seed} onChange={(seed) => onRequest({ seed })} min={0} max={4294967295} wide />
        )}
      </Field>
      <Field label="">
        <Checkbox checked={embed} onChange={onEmbed} label={t("studio.embedParameters")} />
      </Field>
    </Section>
  );
}
