// Enlarging with the local model. When the model is not installed, the user is
// asked once and it is downloaded; a button never stays grey without a reason.
// The detail pass at the same size lives here too.

import { api } from "../../api";
import { useApp } from "../../app/context";
import { useT } from "../../i18n";
import { formatBytes } from "../../lib/format";
import type { GeneratedRecord } from "../../types";
import { tellUpscale } from "./UpscaleOptions";

export function useUpscale() {
  const { ai, confirm, notify, jobResult, refreshAi, withCloud } = useApp();
  const { t } = useT();

  /** True when the chosen model can run, or the built-in one was installed just now. */
  const ensure = async (): Promise<boolean> => {
    const chosen = ai?.settings.generation.upscaler ?? "";
    if (chosen !== "" && chosen !== "builtin") {
      return true;
    }
    if (ai?.capabilities.upscale) {
      return true;
    }
    const model = ai?.components.find((item) => item.kind === "upscale");
    if (!ai || !model) {
      notify(t("studio.upscaleMissing"));
      return false;
    }
    const needed = [model, ...ai.components.filter((item) => model.requires.includes(item.id))].filter(
      (item) => !item.installed,
    );
    if (needed.length === 0) {
      return true;
    }
    const bytes = needed.reduce((sum, item) => sum + item.bytes, 0);
    const agreed = await confirm({
      title: t("studio.upscaleInstallTitle"),
      text: t("studio.upscaleInstallText", { name: model.name, size: formatBytes(bytes) }),
      confirmLabel: t("studio.upscaleInstall"),
    });
    if (!agreed) {
      return false;
    }
    notify(t("studio.upscaleInstalling"));
    await jobResult(await api.componentInstall(model.id));
    refreshAi();
    return true;
  };

  /** The enlarged picture as a new record; `null` when the user declined the model. */
  const upscale = async (file: string, factor: number): Promise<GeneratedRecord | null> => {
    if (!(await ensure())) {
      return null;
    }
    const record = await jobResult<GeneratedRecord>(await api.generatedUpscale(file, factor));
    tellUpscale(record.notice, t, notify);
    return record;
  };

  /**
   * The same picture with detail painted in by the generator, as a new record.
   * `onJob` is told the job, so a form can show how far it is.
   */
  const refine = async (file: string, onJob?: (id: number) => void): Promise<GeneratedRecord> => {
    const id = await withCloud((confirmed) => api.generatedRefine(file, confirmed));
    onJob?.(id);
    return jobResult<GeneratedRecord>(id);
  };

  return { ensure, upscale, refine };
}
