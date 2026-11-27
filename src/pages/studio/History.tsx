// Every picture the studio has kept, newest first, grouped by the request.

import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { thumbUrl } from "../../api/urls";
import { useAppEvent } from "../../api/events";
import { useApp } from "../../app/context";
import { useT } from "../../i18n";
import type { GeneratedRecord, Style } from "../../types";
import { Button, Checkbox, EmptyState, Field, TextInput } from "../../ui";
import { PictureDialog, type PictureItem } from "./PictureDialog";
import { useUpscale } from "./upscale";

function itemsOf(records: GeneratedRecord[]): PictureItem[] {
  return records.flatMap((record) => record.pictures.map((picture) => ({ record, picture })));
}

export function History({
  styles,
  onRepeat,
  onSeed,
  onVariants,
  onSimilar,
  onEdit,
}: {
  styles: Style[];
  onRepeat: (record: GeneratedRecord) => void;
  onSeed: (seed: number) => void;
  onVariants: (record: GeneratedRecord) => void;
  onSimilar: (record: GeneratedRecord, file: string) => void;
  onEdit: (file: string) => void;
}) {
  const { t, td } = useT();
  const { addToQueue, notify, showError, openInEditor, confirm } = useApp();
  const { upscale } = useUpscale();
  const [records, setRecords] = useState<GeneratedRecord[]>([]);
  const [query, setQuery] = useState("");
  const [favorites, setFavorites] = useState(false);
  const [picked, setPicked] = useState<string[]>([]);
  const [open, setOpen] = useState<number | null>(null);
  const anchor = useRef<string | null>(null);

  const reload = () => {
    api
      .generatedList()
      .then(setRecords)
      .catch(showError);
  };
  useEffect(() => {
    reload();
    // The list is asked for once, and again when the core says it changed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useAppEvent("generated:changed", reload);

  const needle = query.trim().toLowerCase();
  const shown = records
    .map((record) => ({
      ...record,
      pictures: record.pictures.filter((picture) => !favorites || picture.favorite),
    }))
    .filter((record) => record.pictures.length > 0)
    .filter((record) => needle === "" || record.prompt.toLowerCase().includes(needle) || record.finalPrompt.toLowerCase().includes(needle));
  const flat = itemsOf(shown);

  const toggle = (file: string, event: { ctrlKey: boolean; shiftKey: boolean; metaKey: boolean }) => {
    if (event.shiftKey && anchor.current) {
      const from = flat.findIndex((item) => item.picture.file === anchor.current);
      const to = flat.findIndex((item) => item.picture.file === file);
      if (from >= 0 && to >= 0) {
        const [start, end] = from < to ? [from, to] : [to, from];
        setPicked(flat.slice(start, end + 1).map((item) => item.picture.file));
        return;
      }
    }
    if (event.ctrlKey || event.metaKey) {
      setPicked((current) => (current.includes(file) ? current.filter((item) => item !== file) : [...current, file]));
      return;
    }
    setPicked([file]);
    anchor.current = file;
  };

  const save = async (files: string[]) => {
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

  const remove = async (files: string[]) => {
    const agreed = await confirm({ title: t("studio.delete"), text: t("studio.deleteText", { count: files.length }), danger: true });
    if (!agreed) {
      return;
    }
    try {
      await api.generatedDelete(files);
      setPicked((current) => current.filter((file) => !files.includes(file)));
      setOpen(null);
    } catch (error) {
      showError(error);
    }
  };

  const favorite = async (file: string, on: boolean) => {
    try {
      await api.generatedFavorite(file, on);
    } catch (error) {
      showError(error);
    }
  };

  const styleLabel = (id: string) => {
    const style = styles.find((item) => item.id === id);
    if (!style) {
      return id;
    }
    return style.builtin ? td(`studio.builtin.${style.id}`) : style.name;
  };

  return (
    <div className="column">
      <Field label={t("studio.historySearch")}>
        <TextInput value={query} onChange={setQuery} wide />
        <Checkbox checked={favorites} onChange={setFavorites} label={t("studio.favoritesOnly")} />
      </Field>
      {picked.length > 0 ? (
        <div className="tool-run">
          <Button small onClick={() => void save(picked)}>
            {t("studio.saveFolder")}
          </Button>
          <Button
            small
            onClick={() => {
              addToQueue(picked);
              notify(t("viewer.addedToBatch", { count: picked.length }));
            }}
          >
            {t("viewer.toBatch")}
          </Button>
          <Button small onClick={() => void remove(picked)}>
            {t("studio.delete")}
          </Button>
        </div>
      ) : null}
      {shown.length === 0 ? (
        <EmptyState title={t("studio.historyEmptyTitle")} text={t("studio.historyEmptyText")} />
      ) : (
        shown.map((record) => (
          <section key={record.id} className="studio-group">
            <header>
              <strong>{record.prompt || t("studio.noPrompt")}</strong>
              <span className="tool-text">
                {new Date(record.created).toLocaleString()}
                {record.model ? ` · ${record.model}` : ""}
                {record.styles.length > 0 ? ` · ${record.styles.map(styleLabel).join(", ")}` : ""}
              </span>
            </header>
            <div className="studio-grid">
              {record.pictures.map((picture) => {
                const index = flat.findIndex((item) => item.picture.file === picture.file);
                return (
                  <div key={picture.file} className="gallery-item">
                    <img
                      src={thumbUrl({ path: picture.file, modified: 0 }, 512)}
                      alt=""
                      draggable={false}
                      onClick={(event) => toggle(picture.file, event)}
                      onDoubleClick={() => setOpen(index)}
                    />
                    <div className="gallery-actions">
                      <Checkbox checked={picked.includes(picture.file)} onChange={() => toggle(picture.file, { ctrlKey: true, shiftKey: false, metaKey: false })} label="" />
                      <Button small onClick={() => void favorite(picture.file, !picture.favorite)}>
                        {picture.favorite ? t("studio.unfavorite") : t("studio.favorite")}
                      </Button>
                      <Button small onClick={() => setOpen(index)}>
                        {t("studio.open")}
                      </Button>
                    </div>
                  </div>
                );
              })}
            </div>
          </section>
        ))
      )}
      {open !== null ? (
        <PictureDialog
          items={flat}
          index={open}
          onIndex={setOpen}
          onClose={() => setOpen(null)}
          onRepeat={onRepeat}
          onSeed={onSeed}
          onVariants={onVariants}
          onSimilar={onSimilar}
          onEdit={onEdit}
          onUpscale={(file, factor) => {
            void upscale(file, factor).catch(showError);
          }}
          onEditor={openInEditor}
          onBatch={(files) => {
            addToQueue(files);
            notify(t("viewer.addedToBatch", { count: files.length }));
          }}
          onReveal={(file) => void api.revealPath(file).catch(showError)}
          onCopy={(text) => void navigator.clipboard.writeText(text).then(() => notify(t("common.copied")))}
          onSave={(files) => void save(files)}
          onDelete={(files) => void remove(files)}
          onFavorite={(file, on) => void favorite(file, on)}
        />
      ) : null}
    </div>
  );
}
