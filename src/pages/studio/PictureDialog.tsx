// A large view of one generated picture, with the request that made it
// and the things that can be done with it.

import { useEffect } from "react";
import { useT } from "../../i18n";
import { fileName } from "../../lib/format";
import { modelTitle } from "../../lib/generation";
import type { GeneratedPicture, GeneratedRecord, Tuning } from "../../types";
import { Button, Modal } from "../../ui";
import { madeFrom } from "./format";
import { PictureStage } from "./PictureStage";
import { UpscaleMenu } from "./UpscaleOptions";

/** The longest side of the picture in the dialog: enough to zoom into the detail. */
const SIDE = 4096;

/** The sampler, the schedule, the VAE, the LoRA files and the embeddings as one line; empty when none was set. */
function tuningLine(tuning: Tuning): string {
  return [
    [tuning.sampler, tuning.scheduler].filter((part) => part !== "").join(" / "),
    tuning.vae ? `VAE ${modelTitle(tuning.vae)}` : "",
    ...tuning.loras.map((lora) => `${modelTitle(lora.name)} ${lora.weight}`),
    ...tuning.embeddings.map((embedding) => `${embedding.negative ? "−" : "+"}${modelTitle(embedding.name)}`),
  ]
    .filter((part) => part !== "")
    .join(" · ");
}

export interface PictureItem {
  record: GeneratedRecord;
  picture: GeneratedPicture;
}

export function PictureDialog({
  items,
  index,
  onIndex,
  onClose,
  onRepeat,
  onSeed,
  onVariants,
  onSimilar,
  onEdit,
  onUpscale,
  refining = false,
  onRefine,
  onEditor,
  onBatch,
  onReveal,
  onCopy,
  onSave,
  onDelete,
  onFavorite,
}: {
  items: PictureItem[];
  index: number;
  onIndex: (index: number) => void;
  onClose: () => void;
  onRepeat: (record: GeneratedRecord) => void;
  onSeed: (seed: number) => void;
  onVariants: (record: GeneratedRecord) => void;
  onSimilar: (record: GeneratedRecord, file: string) => void;
  onEdit: (file: string) => void;
  onUpscale: (file: string, factor: number) => void;
  /** A detail pass is running, so another is not started. */
  refining?: boolean;
  onRefine: (file: string) => void;
  onEditor: (file: string) => void;
  onBatch: (files: string[]) => void;
  onReveal: (file: string) => void;
  onCopy: (text: string) => void;
  onSave: (files: string[]) => void;
  onDelete: (files: string[]) => void;
  onFavorite: (file: string, on: boolean) => void;
}) {
  const { t, td } = useT();
  const item = items[index];

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        onIndex((index + items.length - 1) % items.length);
      } else if (event.key === "ArrowRight") {
        onIndex((index + 1) % items.length);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [index, items.length, onIndex]);

  if (!item) {
    return null;
  }
  const { record, picture } = item;
  const prompt = record.prompt || record.finalPrompt;
  // What the generator was told to avoid: the styles add to it, so the
  // request alone may show nothing.
  const negative = record.finalNegative || record.negative;
  const tuned = tuningLine(record.tuning);

  return (
    <Modal title={fileName(picture.file)} onClose={onClose} full closeLabel={t("common.close")}>
      <div className="studio-view">
        <PictureStage file={picture.file} before={madeFrom(record)} side={SIDE} zoom />
        <div className="studio-view-side">
          <p>{prompt || t("studio.noPrompt")}</p>
          {negative ? <p className="tool-text">{t("studio.negativeLine", { text: negative })}</p> : null}
          <p className="tool-text">
            {td(`studio.kind.${record.kind}`)}
            {record.model ? ` · ${record.model}` : ""}
            {picture.width > 0 ? ` · ${picture.width}×${picture.height}` : ""}
            {` · ${t("studio.seed")} ${picture.seed}`}
          </p>
          {tuned ? <p className="tool-text">{tuned}</p> : null}
          <p className="tool-text">{t("studio.zoomHint")}</p>
          <div className="studio-view-actions">
            <Button small disabled={index === 0 && items.length < 2} onClick={() => onIndex((index + items.length - 1) % items.length)}>
              {t("studio.previous")}
            </Button>
            <Button small onClick={() => onIndex((index + 1) % items.length)}>
              {t("studio.next")}
            </Button>
            <Button small onClick={() => onFavorite(picture.file, !picture.favorite)}>
              {picture.favorite ? t("studio.unfavorite") : t("studio.favorite")}
            </Button>
            <Button small disabled={prompt === ""} onClick={() => onRepeat(record)}>
              {t("studio.repeat")}
            </Button>
            <Button small onClick={() => onSeed(picture.seed)}>
              {t("studio.takeSeed")}
            </Button>
            <Button small disabled={prompt === ""} onClick={() => onVariants(record)}>
              {t("studio.more")}
            </Button>
            <Button small disabled={prompt === ""} onClick={() => onSimilar(record, picture.file)}>
              {t("studio.similar")}
            </Button>
            <Button small onClick={() => onEdit(picture.file)}>
              {t("studio.editThis")}
            </Button>
            <Button small onClick={() => onUpscale(picture.file, 2)}>
              {t("studio.upscale2")}
            </Button>
            <Button small onClick={() => onUpscale(picture.file, 4)}>
              {t("studio.upscale4")}
            </Button>
            <UpscaleMenu />
            <Button small disabled={refining || prompt === ""} title={t("studio.refineHint")} onClick={() => onRefine(picture.file)}>
              {refining ? t("common.working") : t("studio.refine")}
            </Button>
            <Button small onClick={() => onEditor(picture.file)}>
              {t("viewer.edit")}
            </Button>
            <Button small onClick={() => onBatch([picture.file])}>
              {t("viewer.toBatch")}
            </Button>
            <Button small onClick={() => onReveal(picture.file)}>
              {t("common.reveal")}
            </Button>
            <Button small disabled={prompt === ""} onClick={() => onCopy(prompt)}>
              {t("studio.copyPrompt")}
            </Button>
            <Button small onClick={() => onSave([picture.file])}>
              {t("studio.saveFolder")}
            </Button>
            <Button small onClick={() => onDelete([picture.file])}>
              {t("studio.delete")}
            </Button>
          </div>
        </div>
      </div>
    </Modal>
  );
}
