// The pictures of the current request, and the tiles that show a generation
// while it is still running.

import { thumbUrl } from "../../api/urls";
import { useT } from "../../i18n";
import type { GeneratePreview, GeneratedPicture, GeneratedRecord } from "../../types";
import { Button } from "../../ui";

export function Results({
  record,
  previews,
  busy,
  count,
  upscaleReady,
  onOpen,
  onVariants,
  onSimilar,
  onEdit,
  onUpscale,
  onEditor,
  onBatch,
  onReveal,
}: {
  record: GeneratedRecord | null;
  previews: GeneratePreview[];
  busy: boolean;
  count: number;
  upscaleReady: boolean;
  onOpen: (index: number) => void;
  onVariants: (record: GeneratedRecord) => void;
  onSimilar: (record: GeneratedRecord, file: string) => void;
  onEdit: (file: string) => void;
  onUpscale: (file: string, factor: number) => void;
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
            upscaleReady={upscaleReady}
            onOpen={() => onOpen(index)}
            onVariants={() => record && onVariants(record)}
            onSimilar={() => record && picture && onSimilar(record, picture.file)}
            onEdit={() => picture && onEdit(picture.file)}
            onUpscale={(factor) => picture && onUpscale(picture.file, factor)}
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
  upscaleReady,
  onOpen,
  onVariants,
  onSimilar,
  onEdit,
  onUpscale,
  onEditor,
  onBatch,
  onReveal,
}: {
  picture: GeneratedPicture | undefined;
  preview: GeneratePreview | undefined;
  pending: boolean;
  upscaleReady: boolean;
  onOpen: () => void;
  onVariants: () => void;
  onSimilar: () => void;
  onEdit: () => void;
  onUpscale: (factor: number) => void;
  onEditor: () => void;
  onBatch: () => void;
  onReveal: () => void;
}) {
  const { t } = useT();
  const previewUrl = preview?.image ? `data:image/png;base64,${preview.image}` : "";
  return (
    <div className="gallery-item">
      {picture ? (
        <img src={thumbUrl({ path: picture.file, modified: 0 }, 512)} alt="" draggable={false} onDoubleClick={onOpen} />
      ) : previewUrl ? (
        <img src={previewUrl} alt="" draggable={false} />
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
          <Button small disabled={!upscaleReady} title={upscaleReady ? undefined : t("studio.upscaleMissing")} onClick={() => onUpscale(2)}>
            {t("studio.upscale2")}
          </Button>
          <Button small disabled={!upscaleReady} title={upscaleReady ? undefined : t("studio.upscaleMissing")} onClick={() => onUpscale(4)}>
            {t("studio.upscale4")}
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
