// Every picture the studio has kept, newest first: grouped by the request,
// as a plain grid, or as a list with the request beside each picture.

import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { thumbUrl } from "../../api/urls";
import { useAppEvent } from "../../api/events";
import { useApp } from "../../app/context";
import { useT } from "../../i18n";
import { copyText } from "../../lib/clipboard";
import { hasMod, isMac } from "../../lib/platform";
import type { GeneratedPicture, GeneratedRecord, HistoryView, Style } from "../../types";
import { Button, Checkbox, EmptyState, Field, Segmented, TextInput } from "../../ui";
import { PictureDialog, type PictureItem } from "./PictureDialog";
import { useUpscale } from "./upscale";

const VIEWS: readonly HistoryView[] = ["groups", "grid", "list"];

/** The side of a thumbnail asked for a tile and for a row of the list. */
const TILE = 640;
const ROW = 160;

function itemsOf(records: GeneratedRecord[]): PictureItem[] {
  return records.flatMap((record) => record.pictures.map((picture) => ({ record, picture })));
}

function promptOf(record: GeneratedRecord): string {
  return record.prompt || record.finalPrompt;
}

type Keys = { ctrlKey: boolean; shiftKey: boolean; metaKey: boolean };

/** The modifier a click on the checkbox stands for: Command on a Mac, Ctrl elsewhere. */
function addKeys(): Keys {
  return isMac()
    ? { ctrlKey: false, shiftKey: false, metaKey: true }
    : { ctrlKey: true, shiftKey: false, metaKey: false };
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
  const { ai, refreshAi, addToQueue, notify, showError, openInEditor, confirm } = useApp();
  const { upscale, refine } = useUpscale();
  const [records, setRecords] = useState<GeneratedRecord[]>([]);
  const [query, setQuery] = useState("");
  const [favorites, setFavorites] = useState(false);
  const [picked, setPicked] = useState<string[]>([]);
  const [open, setOpen] = useState<number | null>(null);
  const [refining, setRefining] = useState(false);
  const anchor = useRef<string | null>(null);
  const saved: HistoryView = ai?.settings.generation.historyView ?? "groups";
  const [view, setView] = useState<HistoryView>(saved);
  useEffect(() => setView(saved), [saved]);

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

  const changeView = (next: HistoryView) => {
    setView(next);
    if (!ai) {
      return;
    }
    void api
      .aiSettingsSet({ ...ai.settings, generation: { ...ai.settings.generation, historyView: next } })
      .then(() => refreshAi())
      .catch(showError);
  };

  const toggle = (file: string, event: Keys) => {
    if (event.shiftKey && anchor.current) {
      const from = flat.findIndex((item) => item.picture.file === anchor.current);
      const to = flat.findIndex((item) => item.picture.file === file);
      if (from >= 0 && to >= 0) {
        const [start, end] = from < to ? [from, to] : [to, from];
        setPicked(flat.slice(start, end + 1).map((item) => item.picture.file));
        return;
      }
    }
    if (hasMod(event)) {
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

  /**
   * A picture made from the open one is the newest, so the dialog moves to
   * it. Not while only favourites are shown: the new one is not among them.
   */
  const showNewest = () => {
    setOpen((current) => (current === null || favorites ? current : 0));
  };

  const copy = (text: string) => {
    void copyText(text).then((copied) => copied && notify(t("common.copied")));
  };

  const styleLabel = (id: string) => {
    const style = styles.find((item) => item.id === id);
    if (!style) {
      return id;
    }
    return style.builtin ? td(`studio.builtin.${style.id}`) : style.name;
  };

  const about = (record: GeneratedRecord) =>
    [
      new Date(record.created).toLocaleString(),
      record.model,
      record.styles.length > 0 ? record.styles.map(styleLabel).join(", ") : "",
    ]
      .filter((part) => part !== "")
      .join(" · ");

  const copyButton = (record: GeneratedRecord) => (
    <Button small disabled={promptOf(record) === ""} onClick={() => copy(promptOf(record))}>
      {t("studio.copyPrompt")}
    </Button>
  );

  const favoriteButton = (picture: GeneratedPicture) => (
    <Button small onClick={() => void favorite(picture.file, !picture.favorite)}>
      {picture.favorite ? t("studio.unfavorite") : t("studio.favorite")}
    </Button>
  );

  const check = (picture: GeneratedPicture) => (
    <Checkbox checked={picked.includes(picture.file)} onChange={() => toggle(picture.file, addKeys())} label="" />
  );

  /** A picture with its buttons; `record` adds the button that copies the request. */
  const tile = (picture: GeneratedPicture, record?: GeneratedRecord) => {
    const index = flat.findIndex((item) => item.picture.file === picture.file);
    return (
      <div key={picture.file} className="gallery-item" title={record ? promptOf(record) : undefined}>
        <img
          src={thumbUrl({ path: picture.file, modified: 0 }, TILE)}
          alt=""
          draggable={false}
          onClick={(event) => toggle(picture.file, event)}
          onDoubleClick={() => setOpen(index)}
        />
        <div className="gallery-actions">
          {check(picture)}
          {favoriteButton(picture)}
          <Button small onClick={() => setOpen(index)}>
            {t("studio.open")}
          </Button>
          {record ? copyButton(record) : null}
        </div>
      </div>
    );
  };

  const body = () => {
    switch (view) {
      case "groups":
        return shown.map((record) => (
          <section key={record.id} className="studio-group">
            <header>
              <strong>{record.prompt || t("studio.noPrompt")}</strong>
              <span className="studio-group-side">
                <span className="tool-text">{about(record)}</span>
                {copyButton(record)}
              </span>
            </header>
            <div className="studio-grid">{record.pictures.map((picture) => tile(picture))}</div>
          </section>
        ));
      case "grid":
        return <div className="studio-tiles">{flat.map((item) => tile(item.picture, item.record))}</div>;
      case "list":
        return (
          <div className="studio-rows">
            {flat.map((item, index) => (
              <div key={item.picture.file} className="studio-row">
                {check(item.picture)}
                <img
                  src={thumbUrl({ path: item.picture.file, modified: 0 }, ROW)}
                  alt=""
                  draggable={false}
                  onClick={(event) => toggle(item.picture.file, event)}
                  onDoubleClick={() => setOpen(index)}
                />
                <div className="studio-row-text">
                  <strong title={promptOf(item.record)}>{promptOf(item.record) || t("studio.noPrompt")}</strong>
                  <span>
                    {td(`studio.kind.${item.record.kind}`)}
                    {item.picture.width > 0 ? ` · ${item.picture.width}×${item.picture.height}` : ""}
                    {` · ${about(item.record)}`}
                  </span>
                </div>
                <div className="studio-row-actions">
                  {copyButton(item.record)}
                  <Button small disabled={promptOf(item.record) === ""} onClick={() => onRepeat(item.record)}>
                    {t("studio.repeat")}
                  </Button>
                  {favoriteButton(item.picture)}
                  <Button small onClick={() => setOpen(index)}>
                    {t("studio.open")}
                  </Button>
                </div>
              </div>
            ))}
          </div>
        );
      default: {
        const unreachable: never = view;
        return unreachable;
      }
    }
  };

  return (
    <div className="column">
      <Field label={t("studio.historySearch")}>
        <TextInput value={query} onChange={setQuery} wide />
        <Checkbox checked={favorites} onChange={setFavorites} label={t("studio.favoritesOnly")} />
        <Segmented value={view} options={VIEWS.map((value) => ({ value, label: td(`studio.historyView.${value}`) }))} onChange={changeView} />
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
      {shown.length === 0 ? <EmptyState title={t("studio.historyEmptyTitle")} text={t("studio.historyEmptyText")} /> : body()}
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
            void upscale(file, factor)
              .then((made) => made && showNewest())
              .catch(showError);
          }}
          refining={refining}
          onRefine={(file) => {
            setRefining(true);
            void refine(file)
              .then(showNewest)
              .catch(showError)
              .finally(() => setRefining(false));
          }}
          onEditor={openInEditor}
          onBatch={(files) => {
            addToQueue(files);
            notify(t("viewer.addedToBatch", { count: files.length }));
          }}
          onReveal={(file) => void api.revealPath(file).catch(showError)}
          onCopy={copy}
          onSave={(files) => void save(files)}
          onDelete={(files) => void remove(files)}
          onFavorite={(file, on) => void favorite(file, on)}
        />
      ) : null}
    </div>
  );
}
