// The pictures of the current request, and the tiles that show a generation
// while it is still running.

import { previewUrl } from "../../api/urls";
import { useT } from "../../i18n";
import type { GeneratePreview, GeneratedPicture, GeneratedRecord } from "../../types";
import { Button } from "../../ui";
import { madeFrom } from "./format";
import { PictureStage } from "./PictureStage";
import { UpscaleMenu } from "./UpscaleOptions";

/**
 * The longest side asked for a picture that fills the column, and for one of
 * several. A cached thumbnail is 640 pixels at most, which looks soft here.
 */
const FULL = 2048;
const HALF = 1024;

export function Results({
  record,
  previews,
  busy,
  count,
  onOpen,
  onVariants,
  onSimilar,
  onEdit,
  onUpscale,
  refining,
  onRefine,
  onEditor,
  onBatch,
  onReveal,
}: {
  record: GeneratedRecord | null;
  previews: GeneratePreview[];
  busy: boolean;
  count: number;
  onOpen: (index: number) => void;
  onVariants: (record: GeneratedRecord) => void;
  onSimilar: (record: GeneratedRecord, file: string) => void;
  onEdit: (file: string) => void;
  onUpscale: (file: string, factor: number) => void;
  /** A detail pass is running, so another is not started. */
  refining: boolean;
  onRefine: (file: string) => void;
  onEditor: (file: string) => void;
  onBatch: (files: string[]) => void;
  onReveal: (file: string) => void;
}) {
  if (!busy && (!record || record.pictures.length === 0)) {
    return null;
  }
  const tiles = busy ? Math.max(count, 1) : (record?.pictures.length ?? 0);
  return (
    <div className={tiles > 1 ? "studio-grid" : "studio-grid studio-grid-one"}>
      {Array.from({ length: tiles }, (_, index) => {
        const picture = record?.pictures[index];
        const preview = previews.find((item) => item.index === index);
        return (
          <ResultTile
            key={picture?.file ?? `pending-${index}`}
            picture={picture}
            preview={preview}
            pending={busy && !picture}
            alone={tiles === 1}
            before={record ? madeFrom(record) : undefined}
            onOpen={() => onOpen(index)}
            onVariants={() => record && onVariants(record)}
            onSimilar={() => record && picture && onSimilar(record, picture.file)}
            onEdit={() => picture && onEdit(picture.file)}
            onUpscale={(factor) => picture && onUpscale(picture.file, factor)}
            refining={refining}
            onRefine={() => picture && onRefine(picture.file)}
            onEditor={() => picture && onEditor(picture.file)}
            onBatch={() => picture && onBatch([picture.file])}
            onReveal={() => picture && onReveal(picture.file)}
          />
        );
      })}
    </div>
  );
}

function ResultTile({
  picture,
  preview,
  pending,
  alone,
  before,
  onOpen,
  onVariants,
  onSimilar,
  onEdit,
  onUpscale,
  refining,
  onRefine,
  onEditor,
  onBatch,
  onReveal,
}: {
  picture: GeneratedPicture | undefined;
  preview: GeneratePreview | undefined;
  pending: boolean;
  /** The only tile is as wide as the column, so it needs more pixels. */
  alone: boolean;
  /** The picture this one was made from, when there is one to compare with. */
  before: string | undefined;
  onOpen: () => void;
  onVariants: () => void;
  onSimilar: () => void;
  onEdit: () => void;
  onUpscale: (factor: number) => void;
  refining: boolean;
  onRefine: () => void;
  onEditor: () => void;
  onBatch: () => void;
  onReveal: () => void;
}) {
  const { t } = useT();
  const stepUrl = preview?.image ? `data:image/png;base64,${preview.image}` : "";
  return (
    <div className="gallery-item">
      {picture && alone ? (
        // The only picture takes its own shape, and a change shows what it started from.
        <PictureStage
          file={picture.file}
          before={before}
          side={FULL}
          ratio={picture.width > 0 && picture.height > 0 ? picture.width / picture.height : 1}
          onOpen={onOpen}
        />
      ) : picture ? (
        <img src={previewUrl({ path: picture.file, modified: 0 }, HALF)} alt="" draggable={false} onDoubleClick={onOpen} />
      ) : stepUrl ? (
        <img src={stepUrl} alt="" draggable={false} />
      ) : (
        <div className="studio-pending">{pending ? t("studio.pending", { step: preview?.step ?? 0, steps: preview?.steps ?? 0 }) : ""}</div>
      )}
      {picture ? (
        <div className="gallery-actions">
          <Button small onClick={onOpen}>
            {t("studio.open")}
          </Button>
          <Button small onClick={onVariants}>
            {t("studio.more")}
          </Button>
          <Button small onClick={onSimilar}>
            {t("studio.similar")}
          </Button>
          <Button small onClick={onEdit}>
            {t("studio.editThis")}
          </Button>
          <Button small onClick={() => onUpscale(2)}>
            {t("studio.upscale2")}
          </Button>
          <Button small onClick={() => onUpscale(4)}>
            {t("studio.upscale4")}
          </Button>
          <UpscaleMenu />
          <Button small disabled={refining} title={t("studio.refineHint")} onClick={onRefine}>
            {refining ? t("common.working") : t("studio.refine")}
          </Button>
          <Button small onClick={onEditor}>
            {t("viewer.edit")}
          </Button>
          <Button small onClick={onBatch}>
            {t("viewer.toBatch")}
          </Button>
          <Button small onClick={onReveal}>
            {t("common.reveal")}
          </Button>
        </div>
      ) : null}
    </div>
  );
}
