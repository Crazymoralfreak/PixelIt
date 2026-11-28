// Pictures from the web: what a link leads to, search in the open
// catalogues, and downloads into the folder of the settings.
//
// The window has no access to the network: previews are downloaded by the
// core and shown through its own protocol.

import { useCallback, useEffect, useRef, useState } from "react";
import { api, toAppError } from "../api";
import { thumbUrl } from "../api/urls";
import { useApp } from "../app/context";
import { JobRow } from "../components/JobsPanel";
import { useT } from "../i18n";
import { isMac, keyLabel } from "../lib/platform";
import { WEB_SOURCES, type WebDownloaded, type WebImage, type WebSource } from "../types";
import { Button, cx, EmptyState, PageFrame, Section, Select, TextInput } from "../ui";

/** Names of the catalogues; they are the same in every language. */
const SOURCE_NAMES: Record<WebSource, string> = {
  openverse: "Openverse",
  commons: "Wikimedia Commons",
};

/** Where the text of a query goes. */
const SOURCE_HOSTS: Record<WebSource, string> = {
  openverse: "api.openverse.org",
  commons: "commons.wikimedia.org",
};

/** Side of a preview in the list, in pixels. */
const PREVIEW_SIDE = 320;

/** Previews downloaded at a time: one page may name two hundred pictures. */
const MAX_LOADS = 4;

const loads = new Map<string, Promise<string>>();
const waiting: Array<() => void> = [];
let running = 0;

function takeSlot(): Promise<void> {
  if (running < MAX_LOADS) {
    running += 1;
    return Promise.resolve();
  }
  return new Promise((resolve) => waiting.push(resolve));
}

function giveSlot(): void {
  const next = waiting.shift();
  if (next) {
    next();
  } else {
    running -= 1;
  }
}

/** The file of a preview. One address is downloaded once, however many tiles ask for it. */
function loadPreview(image: WebImage): Promise<string> {
  const address = image.preview || image.url;
  const known = loads.get(address);
  if (known) {
    return known;
  }
  const load = takeSlot()
    .then(() => api.webPreview(address, image.page || null))
    .finally(giveSlot);
  loads.set(address, load);
  // A failure is not remembered: the next look at the picture tries again.
  load.catch(() => loads.delete(address));
  return load;
}

/** The name of the file an address points at, or its host. */
function nameOf(address: string): string {
  try {
    const { pathname, hostname } = new URL(address);
    return decodeURIComponent(pathname.split("/").filter(Boolean).pop() ?? "") || hostname;
  } catch {
    return address;
  }
}

function WebTile({
  image,
  selected,
  onToggle,
  onBroken,
}: {
  image: WebImage;
  selected: boolean;
  onToggle: (url: string) => void;
  /** The address turned out not to be a picture. */
  onBroken: (url: string) => void;
}) {
  const { t } = useT();
  const tile = useRef<HTMLButtonElement>(null);
  const [preview, setPreview] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  // The preview is downloaded once the tile comes near the screen.
  useEffect(() => {
    const element = tile.current;
    if (!element) {
      return;
    }
    let alive = true;
    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((entry) => entry.isIntersecting)) {
          return;
        }
        observer.disconnect();
        loadPreview(image).then(
          (path) => {
            if (alive) {
              setPreview(path);
            }
          },
          (error: unknown) => {
            if (!alive) {
              return;
            }
            if (toAppError(error).code === "notPicture") {
              onBroken(image.url);
            } else {
              setFailed(true);
            }
          },
        );
      },
      { rootMargin: "200px" },
    );
    observer.observe(element);
    return () => {
      alive = false;
      observer.disconnect();
    };
  }, [image, onBroken]);

  const size = image.width > 0 && image.height > 0 ? `${image.width}×${image.height}` : "";
  const facts = [image.author, image.license, size].filter(Boolean).join(" · ");

  return (
    <button
      ref={tile}
      type="button"
      className={cx("thumb", "web-tile", selected && "thumb-selected")}
      aria-pressed={selected}
      title={image.page || image.url}
      onClick={() => onToggle(image.url)}
    >
      <span className="web-tile-picture">
        {preview ? (
          <img
            className="thumb-image"
            src={thumbUrl({ path: preview, modified: 0 }, PREVIEW_SIDE)}
            alt=""
            loading="lazy"
            draggable={false}
          />
        ) : (
          <span className="thumb-image web-tile-blank">{failed ? t("web.noPreview") : ""}</span>
        )}
        {image.main ? <span className="web-tile-mark">{t("web.main")}</span> : null}
      </span>
      <span className="thumb-name">{image.title || nameOf(image.url)}</span>
      {facts ? <span className="web-tile-facts">{facts}</span> : null}
    </button>
  );
}

interface Found {
  /** What the pictures were found by: the title of a page or a query. */
  title: string;
  images: WebImage[];
  /** For search results: what the next page is asked with. */
  search: { source: WebSource; query: string; page: number; more: boolean } | null;
}

type Busy = "link" | "search" | "more" | "download";

/** What happens to the files once they are downloaded. */
type After = "keep" | "edit" | "batch";

export default function WebPage() {
  const { t } = useT();
  const { webLink, jobs, jobResult, openInEditor, addToQueue, notify, showError } = useApp();
  const [link, setLink] = useState("");
  const [query, setQuery] = useState("");
  const [source, setSource] = useState<WebSource>("openverse");
  const [found, setFound] = useState<Found | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<string>>(() => new Set<string>());
  const [busy, setBusy] = useState<Busy | null>(null);
  const [jobId, setJobId] = useState<number | null>(null);
  const [saved, setSaved] = useState<string[]>([]);

  // A link pasted elsewhere in the application arrives filled in.
  useEffect(() => {
    if (webLink) {
      setLink(webLink.url);
    }
  }, [webLink]);

  const toggle = useCallback((url: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (!next.delete(url)) {
        next.add(url);
      }
      return next;
    });
  }, []);

  const drop = useCallback((url: string) => {
    setFound((current) =>
      current ? { ...current, images: current.images.filter((image) => image.url !== url) } : current,
    );
    setSelected((current) => {
      if (!current.has(url)) {
        return current;
      }
      const next = new Set(current);
      next.delete(url);
      return next;
    });
  }, []);

  const job = jobId === null ? undefined : jobs.find((item) => item.id === jobId);
  const chosen = found ? found.images.filter((image) => selected.has(image.url)) : [];

  const download = async (images: WebImage[], title: string, after: After) => {
    if (images.length === 0) {
      return;
    }
    setBusy("download");
    try {
      const id = await api.webDownload(images, title);
      setJobId(id);
      const { files, failed } = await jobResult<WebDownloaded>(id);
      setSaved((current) => [...files, ...current.filter((path) => !files.includes(path))]);
      if (failed.length > 0) {
        notify(t("web.someFailed", { count: failed.length }));
      }
      const first = files[0];
      switch (after) {
        case "keep":
          notify(t("web.saved", { count: files.length }));
          break;
        case "edit":
          if (first !== undefined) {
            openInEditor(first);
          }
          break;
        case "batch":
          addToQueue(files);
          notify(t("viewer.addedToBatch", { count: files.length }));
          break;
        default: {
          const unreachable: never = after;
          throw new Error(`Unknown action: ${String(unreachable)}`);
        }
      }
    } catch (error) {
      showError(error);
    } finally {
      setBusy(null);
      setJobId(null);
    }
  };

  const inspect = async () => {
    const address = link.trim();
    if (address === "" || busy !== null) {
      return;
    }
    setBusy("link");
    try {
      const result = await api.webInspect(address);
      if (result.direct) {
        // A picture itself: there is nothing to choose from.
        await download(result.images, result.title, "keep");
        return;
      }
      const first = result.images[0];
      setFound({ title: result.title || result.url, images: result.images, search: null });
      setSelected(new Set(first?.main ? [first.url] : []));
    } catch (error) {
      showError(error);
    } finally {
      setBusy(null);
    }
  };

  const search = async () => {
    const text = query.trim();
    if (text === "" || busy !== null) {
      return;
    }
    setBusy("search");
    try {
      const result = await api.webSearch(source, text, 1);
      setFound({
        title: text,
        images: result.images,
        search: { source, query: text, page: 1, more: result.more },
      });
      setSelected(new Set());
    } catch (error) {
      showError(error);
    } finally {
      setBusy(null);
    }
  };

  const more = async () => {
    const asked = found?.search;
    if (!asked || busy !== null) {
      return;
    }
    setBusy("more");
    try {
      const page = asked.page + 1;
      const result = await api.webSearch(asked.source, asked.query, page);
      setFound((current) => {
        if (!current || current.search !== asked) {
          // Another search was started meanwhile.
          return current;
        }
        const known = new Set(current.images.map((image) => image.url));
        return {
          ...current,
          images: [...current.images, ...result.images.filter((image) => !known.has(image.url))],
          search: { ...asked, page, more: result.more },
        };
      });
    } catch (error) {
      showError(error);
    } finally {
      setBusy(null);
    }
  };

  const idle = busy === null;

  return (
    <PageFrame title={t("nav.web")}>
      <Section title={t("web.byLink")}>
        <div className="web-ask">
          <TextInput
            // A link handed over from elsewhere puts the cursor into the field.
            key={webLink?.token ?? 0}
            autoFocus={webLink !== null}
            value={link}
            onChange={setLink}
            wide
            placeholder={t("web.linkHint")}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                void inspect();
              }
            }}
          />
          <Button variant="primary" disabled={!idle || link.trim() === ""} onClick={() => void inspect()}>
            {busy === "link" ? t("common.working") : t("web.find")}
          </Button>
        </div>
        <p className="tool-text">{t("web.linkAbout")}</p>
        {job && found === null ? <JobRow job={job} /> : null}
      </Section>

      <Section title={t("web.search")}>
        <div className="web-ask">
          <TextInput
            value={query}
            onChange={setQuery}
            wide
            placeholder={t("web.queryHint")}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                void search();
              }
            }}
          />
          <Select
            value={source}
            title={t("web.source")}
            options={WEB_SOURCES.map((value) => ({ value, label: SOURCE_NAMES[value] }))}
            onChange={setSource}
          />
          <Button variant="primary" disabled={!idle || query.trim() === ""} onClick={() => void search()}>
            {busy === "search" ? t("common.working") : t("web.searchStart")}
          </Button>
        </div>
        <p className="tool-text">{t("web.searchNote", { host: SOURCE_HOSTS[source] })}</p>
      </Section>

      {found === null ? (
        saved.length === 0 ? (
          <EmptyState title={t("web.emptyTitle")} text={t("web.emptyText", { keys: keyLabel("Mod+V") })} />
        ) : null
      ) : found.images.length === 0 ? (
        <EmptyState title={t("web.nothingTitle")} text={t("web.nothingText")} />
      ) : (
        <Section
          title={found.title}
          summary={t("web.counts", { found: found.images.length, selected: chosen.length })}
          actions={
            <>
              <Button small onClick={() => setSelected(new Set(found.images.map((image) => image.url)))}>
                {t("web.selectAll")}
              </Button>
              <Button small disabled={chosen.length === 0} onClick={() => setSelected(new Set())}>
                {t("viewer.deselect")}
              </Button>
            </>
          }
        >
          <div className="thumb-grid">
            {found.images.map((image) => (
              <WebTile
                key={image.url}
                image={image}
                selected={selected.has(image.url)}
                onToggle={toggle}
                onBroken={drop}
              />
            ))}
          </div>
          {found.search?.more ? (
            <div className="web-more">
              <Button disabled={!idle} onClick={() => void more()}>
                {busy === "more" ? t("common.working") : t("web.more")}
              </Button>
            </div>
          ) : null}
          <div className="tool-run">
            <Button
              disabled={!idle || chosen.length === 0}
              onClick={() => void download(chosen, found.title, "batch")}
            >
              {t("web.downloadBatch")}
            </Button>
            <Button
              disabled={!idle || chosen.length !== 1}
              title={chosen.length === 1 ? undefined : t("web.editOne")}
              onClick={() => void download(chosen, found.title, "edit")}
            >
              {t("web.downloadEdit")}
            </Button>
            <Button
              variant="primary"
              disabled={!idle || chosen.length === 0}
              onClick={() => void download(chosen, found.title, "keep")}
            >
              {busy === "download" ? t("common.working") : t("web.download")}
            </Button>
          </div>
          {job ? <JobRow job={job} /> : null}
          <p className="tool-text">{t("web.rights")}</p>
        </Section>
      )}

      {saved.length > 0 ? (
        <Section title={t("web.downloaded")}>
          <div className="gallery">
            {saved.map((path) => (
              <div key={path} className="gallery-item">
                <img
                  src={thumbUrl({ path, modified: 0 }, 512)}
                  alt=""
                  loading="lazy"
                  draggable={false}
                  onDoubleClick={() => openInEditor(path)}
                />
                <div className="gallery-actions">
                  <Button small onClick={() => openInEditor(path)}>
                    {t("viewer.edit")}
                  </Button>
                  <Button
                    small
                    onClick={() => {
                      addToQueue([path]);
                      notify(t("viewer.addedToBatch", { count: 1 }));
                    }}
                  >
                    {t("viewer.toBatch")}
                  </Button>
                  <Button small onClick={() => api.revealPath(path).catch(showError)}>
                    {isMac() ? t("common.revealMac") : t("common.reveal")}
                  </Button>
                </div>
              </div>
            ))}
          </div>
        </Section>
      ) : null}
    </PageFrame>
  );
}
