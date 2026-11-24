// The viewer: folders, thumbnails, a full-window preview and the ways
// pictures get into PixelIt and out of it.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { api } from "../api";
import { useAppEvent } from "../api/events";
import { previewUrl, thumbUrl } from "../api/urls";
import { useApp } from "../app/context";
import { useT } from "../i18n";
import { blobToBase64, readClipboardPicture } from "../lib/clipboard";
import { folderOf, formatBytes, formatTime } from "../lib/format";
import { FORMAT_NAMES } from "../lib/pipeline";
import type { FileEntry, FileInfo } from "../types";
import { Button, cx, EmptyState, PageFrame, Select, TextInput } from "../ui";

type Sort = "name" | "date" | "size";

const SORTS: readonly Sort[] = ["name", "date", "size"];
const THUMB_SIZES = { small: 112, medium: 168, large: 248 } as const;
type ThumbSize = keyof typeof THUMB_SIZES;
/** Pixels the pointer travels before a press becomes a drag out of the window. */
const DRAG_DISTANCE = 8;
const RECENT_LIMIT = 12;

function isTyping(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement
  );
}

function sortFiles(files: FileEntry[], sort: Sort): FileEntry[] {
  const sorted = [...files];
  switch (sort) {
    case "name":
      sorted.sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" }));
      break;
    case "date":
      sorted.sort((a, b) => b.modified - a.modified);
      break;
    case "size":
      sorted.sort((a, b) => b.bytes - a.bytes);
      break;
    default: {
      const unreachable: never = sort;
      throw new Error(`Unknown order: ${String(unreachable)}`);
    }
  }
  return sorted;
}

interface LightboxProps {
  files: FileEntry[];
  index: number;
  onIndex: (index: number) => void;
  onClose: () => void;
  onEdit: (file: FileEntry) => void;
  onTrash: (file: FileEntry) => void;
}

function Lightbox({ files, index, onIndex, onClose, onEdit, onTrash }: LightboxProps) {
  const { t } = useT();
  const [actual, setActual] = useState(false);
  const file = files[index];

  useEffect(() => setActual(false), [index]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (isTyping(event.target)) {
        return;
      }
      if (event.key === "Escape") {
        onClose();
      } else if (event.key === "ArrowRight" && index < files.length - 1) {
        onIndex(index + 1);
      } else if (event.key === "ArrowLeft" && index > 0) {
        onIndex(index - 1);
      } else if (event.key === "Enter") {
        onEdit(file);
      } else if (event.key === "Delete") {
        onTrash(file);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [file, files.length, index, onClose, onEdit, onIndex, onTrash]);

  if (!file) {
    return null;
  }
  return (
    <div className="lightbox">
      <div className="lightbox-bar">
        <span className="lightbox-name">
          {file.name} · {index + 1} / {files.length}
        </span>
        <Button small onClick={() => onEdit(file)}>
          {t("viewer.edit")}
        </Button>
        <Button small onClick={() => api.revealPath(file.path).catch(() => undefined)}>
          {t("common.reveal")}
        </Button>
        <Button small onClick={() => onTrash(file)}>
          {t("common.delete")}
        </Button>
        <Button small variant="primary" onClick={onClose}>
          {t("common.close")}
        </Button>
      </div>
      <div className={cx("lightbox-stage", actual && "lightbox-stage-actual")}>
        <img
          key={file.path}
          className="lightbox-image"
          src={previewUrl(file, 2560)}
          alt={file.name}
          draggable={false}
          onClick={() => setActual((current) => !current)}
        />
      </div>
      <button
        type="button"
        className="lightbox-arrow lightbox-arrow-left"
        disabled={index === 0}
        onClick={() => onIndex(index - 1)}
      >
        ‹
      </button>
      <button
        type="button"
        className="lightbox-arrow lightbox-arrow-right"
        disabled={index >= files.length - 1}
        onClick={() => onIndex(index + 1)}
      >
        ›
      </button>
    </div>
  );
}

function InfoPanel({ path }: { path: string }) {
  const { t, language } = useT();
  const [info, setInfo] = useState<FileInfo | null>(null);

  useEffect(() => {
    let alive = true;
    setInfo(null);
    api
      .fileInfo(path)
      .then((next) => alive && setInfo(next))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [path]);

  if (!info) {
    return <aside className="info-panel" />;
  }
  const { file, info: facts } = info;
  const metadata = [facts.hasExif && "EXIF", facts.hasXmp && "XMP", facts.hasIcc && "ICC"].filter(Boolean).join(", ");
  const rows: Array<[string, string]> = [
    [t("viewer.info.size"), `${facts.width} × ${facts.height}`],
    [t("viewer.info.format"), facts.format ? FORMAT_NAMES[facts.format] : "—"],
    [t("viewer.info.bytes"), formatBytes(file.bytes)],
    [t("viewer.info.modified"), formatTime(file.modified / 1000, language)],
    [t("viewer.info.depth"), t("viewer.info.bits", { bits: facts.bits })],
    [t("viewer.info.alpha"), facts.hasAlpha ? t("common.yes") : t("common.no")],
    [t("viewer.info.metadata"), metadata || "—"],
  ];
  if (facts.frames > 1) {
    rows.push([t("viewer.info.frames"), String(facts.frames)]);
  }
  return (
    <aside className="info-panel">
      <img className="info-preview" src={thumbUrl(file, 512)} alt="" draggable={false} />
      <div className="info-name">{file.name}</div>
      {rows.map(([label, value]) => (
        <div key={label} className="info-row">
          <span className="info-label">{label}</span>
          <span>{value}</span>
        </div>
      ))}
    </aside>
  );
}

export default function ViewerPage({ active }: { active: boolean }) {
  const { t, td } = useT();
  const { settings, saveSettings, incoming, openInEditor, addToQueue, navigate, confirm, notify, showError } = useApp();

  const [folder, setFolder] = useState<string | null>(null);
  const [parent, setParent] = useState<string | null>(null);
  const [folders, setFolders] = useState<FileEntry[]>([]);
  const [files, setFiles] = useState<FileEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<Sort>("name");
  const [size, setSize] = useState<ThumbSize>("medium");
  const [selected, setSelected] = useState<string[]>([]);
  const [focus, setFocus] = useState<string | null>(null);
  const [lightbox, setLightbox] = useState<number | null>(null);
  const press = useRef<{ x: number; y: number; path: string } | null>(null);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  const shown = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    const matching = needle === "" ? files : files.filter((file) => file.name.toLowerCase().includes(needle));
    return sortFiles(matching, sort);
  }, [files, filter, sort]);

  const showFolder = useCallback(
    async (path: string, select?: string) => {
      setLoading(true);
      try {
        const listing = await api.folderList(path);
        setFolder(listing.folder);
        setParent(listing.parent);
        setFolders(listing.folders);
        setFiles(listing.files);
        setSelected(select ? [select] : []);
        setFocus(select ?? null);
        setFilter("");
        const current = settingsRef.current;
        if (current.recentFolders[0] !== listing.folder) {
          const recentFolders = [listing.folder, ...current.recentFolders.filter((item) => item !== listing.folder)];
          void saveSettings({ ...current, recentFolders: recentFolders.slice(0, RECENT_LIMIT) });
        }
        return listing;
      } catch (error) {
        showError(error);
        return null;
      } finally {
        setLoading(false);
      }
    },
    [saveSettings, showError],
  );

  /** Shows loose files, without a folder around them. */
  const showFiles = useCallback((list: FileEntry[]) => {
    setFolder(null);
    setParent(null);
    setFolders([]);
    setFiles(list);
    setSelected([]);
    setFocus(null);
    setFilter("");
  }, []);

  const open = useCallback(
    async (paths: string[]) => {
      if (paths.length === 1) {
        const listing = await api.folderList(paths[0]).catch(() => null);
        if (listing) {
          await showFolder(paths[0]);
          return;
        }
        // One file: its folder with the file opened, so the neighbours are a key press away.
        const shownFolder = await showFolder(folderOf(paths[0]), paths[0]);
        const index = sortFiles(shownFolder?.files ?? [], "name").findIndex((file) => file.path === paths[0]);
        if (index >= 0) {
          setSort("name");
          setLightbox(index);
        }
        return;
      }
      const list = await api.filesExpand(paths, true);
      if (list.length === 0) {
        notify(t("viewer.noPictures"));
        return;
      }
      showFiles(list);
    },
    [notify, showFiles, showFolder, t],
  );

  useEffect(() => {
    if (incoming) {
      void open(incoming.paths);
    }
    // A new token means a new hand-over, even of the same paths.
  }, [incoming?.token]);

  const reload = useCallback(() => {
    if (folder) {
      api
        .folderList(folder)
        .then((listing) => {
          setFolders(listing.folders);
          setFiles(listing.files);
        })
        .catch(() => undefined);
    }
  }, [folder]);

  useEffect(() => {
    if (active) {
      reload();
    }
  }, [active, reload]);
  useAppEvent("journal:changed", reload);

  const chooseFolder = async () => {
    const chosen = await api.pickFolder(folder ?? undefined);
    if (chosen) {
      await showFolder(chosen);
    }
  };

  const chooseFiles = async () => {
    const chosen = await api.pickFiles(true);
    if (chosen.length > 0) {
      await open(chosen);
    }
  };

  const paste = async () => {
    const picture = await readClipboardPicture();
    if (!picture) {
      notify(t("viewer.pasteHint"));
      return;
    }
    blobToBase64(picture).then(api.pasteImage).then(openInEditor).catch(showError);
  };

  const trash = useCallback(
    async (paths: string[]) => {
      if (paths.length === 0) {
        return;
      }
      if (settings.confirmDelete) {
        const agreed = await confirm({
          title: t("viewer.trashTitle"),
          text: t("viewer.trashText", { count: paths.length }),
          confirmLabel: t("common.delete"),
          danger: true,
        });
        if (!agreed) {
          return;
        }
      }
      try {
        await api.filesTrash(paths);
      } catch (error) {
        showError(error);
      }
      setFiles((current) => current.filter((file) => !paths.includes(file.path)));
      setSelected((current) => current.filter((path) => !paths.includes(path)));
      setFocus((current) => (current && paths.includes(current) ? null : current));
    },
    [confirm, settings.confirmDelete, showError, t],
  );

  const click = (event: ReactMouseEvent, file: FileEntry) => {
    if (event.shiftKey && focus) {
      const from = shown.findIndex((item) => item.path === focus);
      const to = shown.findIndex((item) => item.path === file.path);
      if (from >= 0 && to >= 0) {
        setSelected(shown.slice(Math.min(from, to), Math.max(from, to) + 1).map((item) => item.path));
        return;
      }
    }
    if (event.ctrlKey) {
      setSelected((current) =>
        current.includes(file.path) ? current.filter((path) => path !== file.path) : [...current, file.path],
      );
    } else {
      setSelected([file.path]);
    }
    setFocus(file.path);
  };

  // Dragging thumbnails out of the window hands the files to Explorer or another program.
  useEffect(() => {
    const onMove = (event: MouseEvent) => {
      const start = press.current;
      if (!start || event.buttons !== 1) {
        press.current = null;
        return;
      }
      if (Math.hypot(event.clientX - start.x, event.clientY - start.y) < DRAG_DISTANCE) {
        return;
      }
      press.current = null;
      const paths = selected.includes(start.path) ? selected : [start.path];
      api.dragFiles(paths).catch(showError);
    };
    window.addEventListener("mousemove", onMove);
    return () => window.removeEventListener("mousemove", onMove);
  }, [selected, showError]);

  useEffect(() => {
    if (!active || lightbox !== null) {
      return;
    }
    const onKey = (event: KeyboardEvent) => {
      if (isTyping(event.target) || document.querySelector(".modal-backdrop")) {
        return;
      }
      const index = focus ? shown.findIndex((file) => file.path === focus) : -1;
      if (event.ctrlKey && event.code === "KeyA") {
        event.preventDefault();
        setSelected(shown.map((file) => file.path));
      } else if (event.key === "Enter" && index >= 0) {
        setLightbox(index);
      } else if (event.key === "Delete") {
        void trash(selected);
      } else if ((event.key === "ArrowRight" || event.key === "ArrowLeft") && shown.length > 0) {
        const next = Math.min(shown.length - 1, Math.max(0, index + (event.key === "ArrowRight" ? 1 : -1)));
        setFocus(shown[next].path);
        setSelected([shown[next].path]);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active, focus, lightbox, selected, shown, trash]);

  const toBatch = (paths: string[]) => {
    addToQueue(paths);
    notify(t("viewer.addedToBatch", { count: paths.length }));
  };

  const empty = folder === null && files.length === 0;
  const lightboxFile = lightbox === null ? undefined : shown[lightbox];

  return (
    <PageFrame
      title={t("nav.viewer")}
      actions={
        <>
          <Button onClick={() => void chooseFolder()}>{t("viewer.openFolder")}</Button>
          <Button onClick={() => void chooseFiles()}>{t("viewer.openFiles")}</Button>
          <Button onClick={() => void paste()}>{t("viewer.paste")}</Button>
          <Button onClick={() => api.captureScreen().then(openInEditor).catch(showError)}>{t("viewer.capture")}</Button>
        </>
      }
    >
      {empty ? (
        <>
          <EmptyState
            title={t("viewer.emptyTitle")}
            text={t("viewer.emptyText")}
            action={
              <Button variant="primary" onClick={() => void chooseFolder()}>
                {t("viewer.openFolder")}
              </Button>
            }
          />
          {settings.recentFolders.length > 0 ? (
            <div className="recent">
              <div className="recent-title">{t("viewer.recent")}</div>
              {settings.recentFolders.map((item) => (
                <button key={item} type="button" className="recent-item" onClick={() => void showFolder(item)}>
                  {item}
                </button>
              ))}
            </div>
          ) : null}
        </>
      ) : (
        <div className="viewer">
          <div className="viewer-main">
            <div className="viewer-bar">
              <Button small disabled={!parent} onClick={() => parent && void showFolder(parent)}>
                {t("viewer.up")}
              </Button>
              <span className="viewer-path" title={folder ?? ""}>
                {folder ?? t("viewer.looseFiles", { count: files.length })}
              </span>
              <TextInput value={filter} onChange={setFilter} placeholder={t("viewer.filter")} />
              <Select
                value={sort}
                options={SORTS.map((value) => ({ value, label: td(`viewer.sort.${value}`) }))}
                onChange={setSort}
              />
              <Select
                value={size}
                options={(Object.keys(THUMB_SIZES) as ThumbSize[]).map((value) => ({
                  value,
                  label: td(`viewer.thumb.${value}`),
                }))}
                onChange={setSize}
              />
            </div>
            {selected.length > 0 ? (
              <div className="viewer-bar viewer-selection">
                <span>{t("viewer.selected", { count: selected.length })}</span>
                <Button small disabled={selected.length !== 1} onClick={() => openInEditor(selected[0])}>
                  {t("viewer.edit")}
                </Button>
                <Button small onClick={() => toBatch(selected)}>
                  {t("viewer.toBatch")}
                </Button>
                <Button small onClick={() => api.revealPath(selected[0]).catch(showError)}>
                  {t("common.reveal")}
                </Button>
                <Button small onClick={() => void trash(selected)}>
                  {t("common.delete")}
                </Button>
                <Button small variant="ghost" onClick={() => setSelected([])}>
                  {t("viewer.deselect")}
                </Button>
              </div>
            ) : folder ? (
              <div className="viewer-bar viewer-selection">
                <span>{t("viewer.count", { count: shown.length })}</span>
                <Button
                  small
                  disabled={files.length === 0}
                  onClick={() => {
                    toBatch([folder]);
                    navigate("batch");
                  }}
                >
                  {t("viewer.folderToBatch")}
                </Button>
              </div>
            ) : null}
            <div className="viewer-scroll">
              {folders.length > 0 ? (
                <div className="folder-row">
                  {folders.map((item) => (
                    <button key={item.path} type="button" className="folder-chip" onClick={() => void showFolder(item.path)}>
                      {item.name}
                    </button>
                  ))}
                </div>
              ) : null}
              {shown.length === 0 && !loading ? <div className="viewer-none">{t("viewer.noPictures")}</div> : null}
              <div className="thumb-grid" style={{ "--thumb": `${THUMB_SIZES[size]}px` } as CSSProperties}>
                {shown.map((file, index) => (
                  <div
                    key={file.path}
                    className={cx(
                      "thumb",
                      selected.includes(file.path) && "thumb-selected",
                      focus === file.path && "thumb-focus",
                    )}
                    title={file.name}
                    onMouseDown={(event) => {
                      if (event.button === 0) {
                        press.current = { x: event.clientX, y: event.clientY, path: file.path };
                      }
                    }}
                    onClick={(event) => click(event, file)}
                    onDoubleClick={() => setLightbox(index)}
                  >
                    <img
                      className="thumb-image"
                      src={thumbUrl(file, 256)}
                      alt=""
                      loading="lazy"
                      decoding="async"
                      draggable={false}
                    />
                    <span className="thumb-name">{file.name}</span>
                  </div>
                ))}
              </div>
            </div>
          </div>
          {focus ? <InfoPanel path={focus} /> : null}
        </div>
      )}
      {lightbox !== null && lightboxFile ? (
        <Lightbox
          files={shown}
          index={lightbox}
          onIndex={(index) => {
            setLightbox(index);
            setFocus(shown[index].path);
            setSelected([shown[index].path]);
          }}
          onClose={() => setLightbox(null)}
          onEdit={(file) => {
            setLightbox(null);
            openInEditor(file.path);
          }}
          onTrash={(file) => {
            void trash([file.path]).then(() => setLightbox(null));
          }}
        />
      ) : null}
    </PageFrame>
  );
}
