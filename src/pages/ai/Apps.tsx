// ComfyUI and Forge: install, start, stop, shared model folders and catalogue search.

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../../api";
import { useApp } from "../../app/context";
import { JobRow, jobAmount } from "../../components/JobsPanel";
import { PathField } from "../../components/PathField";
import { useT } from "../../i18n";
import { copyText } from "../../lib/clipboard";
import { formatDuration } from "../../lib/format";
import type {
  AiSettings,
  AiStatus,
  AppCard,
  AppKind,
  AppLog,
  AppsStatus,
  JobInfo,
  ModelHit,
  ModelKind,
  SearchSource,
  UpdateInfo,
} from "../../types";
import { Button, Checkbox, Field, Modal, Progress, Section, Segmented, Select, Tag, TextInput, cx } from "../../ui";

type ResultView = "list" | "grid";
type SortId = "downloads" | "rating" | "newest";
type PeriodId = "all" | "year" | "month" | "week" | "day";

const previewCache = new Map<string, string>();
const WINDOWS = navigator.userAgent.includes("Windows");

function worksLocally(hit: ModelHit): boolean {
  return WINDOWS && hit.kind === "upscale" && (hit.files ?? []).some((file) => file.format === "onnx");
}

function withFile(hit: ModelHit, index: number): ModelHit {
  const file = hit.files?.[index];
  if (!file) {
    return hit;
  }
  return { ...hit, fileName: file.fileName, url: file.url, bytes: file.bytes, sha256: file.sha256 };
}

function knownView(view: ResultView): ResultView {
  switch (view) {
    case "list":
    case "grid":
      return view;
    default: {
      const unreachable: never = view;
      return unreachable;
    }
  }
}

function knownSort(sort: SortId): SortId {
  switch (sort) {
    case "downloads":
    case "rating":
    case "newest":
      return sort;
    default: {
      const unreachable: never = sort;
      return unreachable;
    }
  }
}

function knownPeriod(period: PeriodId): PeriodId {
  switch (period) {
    case "all":
    case "year":
    case "month":
    case "week":
    case "day":
      return period;
    default: {
      const unreachable: never = period;
      return unreachable;
    }
  }
}

function ModelPicture({ url, frame }: { url: string; frame: "card" | "stage" | "film" }) {
  const [src, setSrc] = useState(() => (url ? previewCache.get(url) ?? "" : ""));

  useEffect(() => {
    if (!url) {
      setSrc("");
      return;
    }
    const known = previewCache.get(url);
    if (known !== undefined) {
      setSrc(known);
      return;
    }
    let gone = false;
    api
      .appsPreview(url)
      .then((image) => {
        const next = `data:${image.mime};base64,${image.data}`;
        previewCache.set(url, next);
        if (!gone) {
          setSrc(next);
        }
      })
      .catch(() => {
        previewCache.set(url, "");
        if (!gone) {
          setSrc("");
        }
      });
    return () => {
      gone = true;
    };
  }, [url]);

  return (
    <div className={`model-picture model-picture-${frame}`}>
      {src ? <img src={src} alt="" draggable={false} /> : <div className="model-picture-empty" />}
    </div>
  );
}

const PROGRAMS: readonly AppKind[] = ["comfy", "forge"];
const KINDS: readonly ModelKind[] = ["checkpoints", "loras", "vaes", "embeddings", "controlNet", "upscale"];
const SOURCES: readonly SearchSource[] = ["civitai", "civarchive", "huggingface", "openmodeldb"];

function knownProgram(kind: AppKind): AppKind {
  switch (kind) {
    case "comfy":
    case "forge":
      return kind;
    default: {
      const unreachable: never = kind;
      return unreachable;
    }
  }
}

function knownState(state: AppCard["state"]): AppCard["state"] {
  switch (state) {
    case "missing":
    case "stopped":
    case "running":
    case "foreign":
      return state;
    default: {
      const unreachable: never = state;
      return unreachable;
    }
  }
}

function stateTag(state: AppCard["state"]): string {
  switch (state) {
    case "running":
    case "foreign":
      return "tag tag-ok";
    case "stopped":
      return "tag tag-warning";
    case "missing":
      return "tag";
    default: {
      const unreachable: never = state;
      return unreachable;
    }
  }
}

function knownOrigin(origin: AppCard["origin"]): AppCard["origin"] {
  switch (origin) {
    case "missing":
    case "ours":
    case "folder":
    case "stabilityMatrix":
      return origin;
    default: {
      const unreachable: never = origin;
      return unreachable;
    }
  }
}

function knownKind(kind: ModelKind): ModelKind {
  switch (kind) {
    case "checkpoints":
    case "loras":
    case "vaes":
    case "embeddings":
    case "controlNet":
    case "upscale":
      return kind;
    default: {
      const unreachable: never = kind;
      return unreachable;
    }
  }
}

function knownSource(source: SearchSource): SearchSource {
  switch (source) {
    case "civitai":
    case "civarchive":
    case "huggingface":
    case "openmodeldb":
      return source;
    default: {
      const unreachable: never = source;
      return unreachable;
    }
  }
}

function jobFor(jobs: JobInfo[], title: string) {
  return jobs.find(
    (item) =>
      item.kind === "install" &&
      item.title === title &&
      (item.state === "queued" || item.state === "running" || item.state === "paused"),
  );
}

/** A download as a card or a row can hold it: the bar, the share, the amount and the time left. */
function LoadState({ job }: { job: JobInfo }) {
  const { t, td } = useT();
  const share = job.total > 0 ? `${Math.min(100, Math.floor((job.done / job.total) * 100))}%` : td(`jobs.state.${job.state}`);
  const amount = jobAmount(job);
  return (
    <div className="model-load">
      <Progress done={job.done} total={job.state === "queued" ? 1 : job.total} />
      <div className="model-load-text">
        <span>{share}</span>
        {amount ? <span>{amount}</span> : null}
      </div>
      {job.etaSeconds !== null && job.state === "running" ? (
        <div className="model-load-text">{t("jobs.eta", { time: formatDuration(job.etaSeconds) })}</div>
      ) : null}
    </div>
  );
}

/** "Download", or "Cancel" while the file of this model is on its way. */
function LoadButton({ hit, job, primary }: { hit: ModelHit; job: JobInfo | undefined; primary?: boolean }) {
  const { t } = useT();
  const { showError } = useApp();
  return (
    <Button
      small
      variant={job || !primary ? "default" : "primary"}
      onClick={(event) => {
        event.stopPropagation();
        if (job) {
          api.jobCancel(job.id).catch(showError);
        } else {
          api.appsDownload(hit).catch(showError);
        }
      }}
    >
      {job ? t("common.cancel") : t("ai.apps.download")}
    </Button>
  );
}

/** How often the log is read again while the program runs, in milliseconds. */
const LOG_EVERY = 1500;
/** How often a running program is asked whether it still runs, in milliseconds. */
const STATUS_EVERY = 3000;

const LOG_ERROR = /\b(error|exception|traceback|failed|fatal)\b/i;
const LOG_WARNING = /\bwarn(ing)?\b/i;

function logLineClass(line: string): string {
  if (line.startsWith("====") || line.startsWith("> ")) {
    return "app-log-line app-log-head";
  }
  if (LOG_ERROR.test(line)) {
    return "app-log-line app-log-error";
  }
  if (LOG_WARNING.test(line)) {
    return "app-log-line app-log-warning";
  }
  return "app-log-line";
}

/** What the program wrote since its last start: read, copy, clear, show the file. */
function LogPanel({
  kind,
  state,
  open,
  onToggle,
}: {
  kind: AppKind;
  state: AppCard["state"];
  open: boolean;
  onToggle: () => void;
}) {
  const { t } = useT();
  const { notify, showError } = useApp();
  const [log, setLog] = useState<AppLog | null>(null);
  const body = useRef<HTMLDivElement | null>(null);
  // The view follows new lines until the user scrolls up to read.
  const follow = useRef(true);

  const load = useCallback(() => {
    api
      .appsLog(kind)
      .then((next) =>
        setLog((current) =>
          current && current.text === next.text && current.bytes === next.bytes && current.path === next.path
            ? current
            : next,
        ),
      )
      .catch(() => undefined);
  }, [kind]);

  useEffect(() => {
    load();
  }, [load, state, open]);

  useEffect(() => {
    if (!open || state !== "running") {
      return undefined;
    }
    const timer = window.setInterval(load, LOG_EVERY);
    return () => window.clearInterval(timer);
  }, [open, state, load]);

  const text = log?.text ?? "";
  useLayoutEffect(() => {
    const box = body.current;
    if (box && follow.current) {
      box.scrollTop = box.scrollHeight;
    }
  }, [text, open]);

  const lines = text ? text.split("\n") : [];
  const last = lines.length > 0 ? lines[lines.length - 1] : "";

  return (
    <div className="app-log">
      <div className="app-log-bar">
        <button type="button" className="section-toggle app-log-toggle" aria-expanded={open} onClick={onToggle}>
          <span className={cx("section-mark", open && "section-mark-open")} aria-hidden="true" />
          <span>{t("ai.apps.log")}</span>
          {!open && last ? <span className="app-log-last">{last}</span> : null}
        </button>
        <Button
          small
          disabled={!text}
          onClick={() => {
            void copyText(text).then((copied) => {
              if (copied) {
                notify(t("common.copied"));
              }
            });
          }}
        >
          {t("common.copy")}
        </Button>
        <Button
          small
          disabled={!log || log.bytes === 0}
          onClick={() => {
            api.appsLogClear(kind).then(load).catch(showError);
          }}
        >
          {t("common.clear")}
        </Button>
        <Button
          small
          disabled={!log?.path}
          onClick={() => {
            if (log?.path) {
              api.revealPath(log.path).catch(showError);
            }
          }}
        >
          {t("ai.apps.logFile")}
        </Button>
      </div>
      {open ? (
        <>
          <div
            ref={body}
            className="app-log-body"
            onScroll={(event) => {
              const box = event.currentTarget;
              follow.current = box.scrollHeight - box.scrollTop - box.clientHeight < 24;
            }}
          >
            {lines.length === 0 ? (
              <div className="app-log-line app-log-head">{t("ai.apps.logEmpty")}</div>
            ) : (
              lines.map((line, index) => (
                <div key={index} className={logLineClass(line)}>
                  {line}
                </div>
              ))
            )}
          </div>
          {log?.cut ? <div className="app-log-note">{t("ai.apps.logCut")}</div> : null}
        </>
      ) : null}
    </div>
  );
}

function ProgramCard({
  card,
  status,
  onChanged,
}: {
  card: AppCard;
  status: AppsStatus;
  onChanged: () => void;
}) {
  const { t, td } = useT();
  const { jobs, showError } = useApp();
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [logOpen, setLogOpen] = useState(card.state === "running");
  const job = jobFor(jobs, card.kind);
  const stabilityHome = card.kind === "comfy" ? status.stabilityComfy : status.stabilityForge;

  const run = (action: () => Promise<unknown>) => {
    action().then(onChanged).catch(showError);
  };

  return (
    <div className="app-card">
      <div className="component">
        <div className="component-main">
          <div className="component-name">
            {td(`ai.apps.program.${knownProgram(card.kind)}`)}
            <span className={stateTag(card.state)}>{td(`ai.apps.state.${knownState(card.state)}`)}</span>
          </div>
          <div className="component-facts">
            {card.home || t("ai.apps.noHome")}
            {card.release ? ` · ${card.release}` : ""}
            {card.nvidia ? ` · ${t("ai.apps.nvidia")}` : ""}
          </div>
          {card.origin !== "missing" ? (
            <div className="component-facts">{td(`ai.apps.origin.${knownOrigin(card.origin)}`)}</div>
          ) : null}
          {card.home && card.state === "missing" ? (
            <div className="component-facts">{t("ai.apps.notInFolder")}</div>
          ) : null}
          {update?.owned && update.newer ? (
            <div className="tool-text">{t("ai.apps.updateReady", { version: update.release })}</div>
          ) : null}
          {update && !update.owned ? <div className="tool-text">{t("ai.apps.foreignUpdate")}</div> : null}
          {job ? <JobRow job={job} /> : null}
        </div>
        <Button small disabled={!card.canInstall || Boolean(job)} onClick={() => run(() => api.appsInstall(card.kind))}>
          {t("ai.apps.install")}
        </Button>
        <PathField
          kind="folder"
          value={card.home}
          onChange={(home) => run(() => api.appsSetHome(card.kind, home))}
        />
        {status.stability && stabilityHome ? (
          <Button small onClick={() => run(() => api.appsUseStability(card.kind))}>
            {t("ai.apps.useStability")}
          </Button>
        ) : null}
        <Button
          small
          disabled={card.state === "missing" || card.state === "running" || card.state === "foreign"}
          onClick={() => {
            setLogOpen(true);
            run(() => api.appsStart(card.kind));
          }}
        >
          {t("ai.apps.start")}
        </Button>
        <Button small disabled={card.state !== "running"} onClick={() => run(() => api.appsStop(card.kind))}>
          {t("ai.apps.stop")}
        </Button>
        <Button small disabled={card.state === "missing"} onClick={() => api.appsOpen(card.kind).catch(showError)}>
          {t("ai.apps.open")}
        </Button>
        <Button
          small
          onClick={() => {
            api
              .appsCheckUpdate(card.kind)
              .then(setUpdate)
              .catch(showError);
          }}
        >
          {t("ai.apps.checkUpdates")}
        </Button>
        {update?.newer ? (
          <Button small variant="primary" disabled={Boolean(job)} onClick={() => run(() => api.appsUpdate(card.kind))}>
            {t("ai.apps.update")}
          </Button>
        ) : null}
      </div>
      {card.home ? (
        <LogPanel
          kind={card.kind}
          state={card.state}
          open={logOpen}
          onToggle={() => setLogOpen((current) => !current)}
        />
      ) : null}
    </div>
  );
}

function Folders({
  ai,
  status,
  onSettings,
  onChanged,
}: {
  ai: AiStatus;
  status: AppsStatus;
  onSettings: (settings: AiSettings) => Promise<boolean>;
  onChanged: () => void;
}) {
  const { t, td } = useT();
  const { showError } = useApp();
  const models = ai.settings.apps?.models ?? status.models;

  const add = (kind: ModelKind, folder: string) => {
    const next = structuredClone(ai.settings);
    const list = next.apps.models[kind];
    if (!list.includes(folder)) {
      list.push(folder);
    }
    void onSettings(next);
  };
  const remove = (kind: ModelKind, folder: string) => {
    const next = structuredClone(ai.settings);
    next.apps.models[kind] = next.apps.models[kind].filter((item) => item !== folder);
    void onSettings(next);
  };
  const browse = (kind: ModelKind) => {
    void api
      .pickFolder(models[kind][0])
      .then((folder) => {
        if (folder) {
          add(kind, folder);
        }
      })
      .catch(showError);
  };

  return (
    <Section title={t("ai.apps.models")} collapsible defaultOpen>
      <p className="tool-text">{t("ai.apps.modelsText")}</p>
      {status.stability ? (
        <Button
          small
          onClick={() => {
            api.appsFillModels().then(onChanged).catch(showError);
          }}
        >
          {t("ai.apps.fillModels")}
        </Button>
      ) : null}
      {KINDS.map((kind) => (
        <Field key={kind} label={td(`ai.apps.kind.${knownKind(kind)}`)}>
          <div className="model-folders">
            {models[kind].length === 0 ? (
              <PathField kind="folder" value="" onChange={(folder) => add(kind, folder)} />
            ) : (
              models[kind].map((folder, index) => (
                <div key={folder} className="model-folder">
                  <span className="model-folder-path">{folder}</span>
                  <Button small onClick={() => remove(kind, folder)}>
                    {t("common.delete")}
                  </Button>
                  {index === models[kind].length - 1 ? (
                    <Button small onClick={() => browse(kind)}>
                      {t("common.browse")}
                    </Button>
                  ) : null}
                </div>
              ))
            )}
          </div>
        </Field>
      ))}
    </Section>
  );
}

function Search({ ai, onSettings }: { ai: AiStatus; onSettings: (settings: AiSettings) => Promise<boolean> }) {
  const { t, td } = useT();
  const { jobs, showError } = useApp();
  const [source, setSource] = useState<SearchSource>("civitai");
  const [text, setText] = useState("");
  const [kind, setKind] = useState<ModelKind>("checkpoints");
  const [sort, setSort] = useState<SortId>("downloads");
  const [period, setPeriod] = useState<PeriodId>("all");
  const [baseModel, setBaseModel] = useState("");
  const [hideInstalled, setHideInstalled] = useState(true);
  const [hits, setHits] = useState<ModelHit[]>([]);
  const [cursor, setCursor] = useState("");
  const [loading, setLoading] = useState(false);
  const [view, setView] = useState<ResultView>("grid");
  const [opened, setOpened] = useState<ModelHit | null>(null);
  const [shot, setShot] = useState(0);
  const [fileIndex, setFileIndex] = useState(0);
  const showNsfw = ai.settings.apps?.showNsfw ?? false;
  const loadingRef = useRef(false);
  const cursorRef = useRef("");
  const sentinel = useRef<HTMLDivElement>(null);
  cursorRef.current = cursor;
  const chosen = opened ? withFile(opened, fileIndex) : null;
  const openedJob = chosen ? jobFor(jobs, chosen.fileName) : undefined;
  const shots = opened?.previews ?? [];
  const shownShot = shots[Math.min(shot, Math.max(shots.length - 1, 0))] ?? "";

  const openHit = (hit: ModelHit) => {
    setOpened(hit);
    setShot(0);
    setFileIndex(0);
  };

  const ask = (nextCursor: string, append: boolean) => {
    if (loadingRef.current) {
      return;
    }
    loadingRef.current = true;
    setLoading(true);
    api
      .appsSearch(source, text, kind, sort, period, baseModel, hideInstalled, nextCursor)
      .then((page) => {
        setHits((current) => {
          if (!append) {
            return page.hits;
          }
          const seen = new Set(current.map((hit) => `${hit.source}:${hit.url}`));
          return [...current, ...page.hits.filter((hit) => !seen.has(`${hit.source}:${hit.url}`))];
        });
        if (!append) {
          setOpened(null);
          setShot(0);
        }
        setCursor(page.cursor === nextCursor ? "" : page.cursor);
      })
      .catch(showError)
      .finally(() => {
        loadingRef.current = false;
        setLoading(false);
      });
  };

  useEffect(() => {
    const node = sentinel.current;
    if (!node || !cursor) {
      return;
    }
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        ask(cursorRef.current, true);
      }
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, [cursor, source, text, kind, sort, period, baseModel, hideInstalled, showNsfw]);

  return (
    <Section title={t("ai.apps.search")} collapsible defaultOpen={false}>
      <p className="tool-text">{t("ai.apps.searchText")}</p>
      <Field label={t("ai.apps.source")}>
        <Select
          value={source}
          options={SOURCES.map((item) => ({ value: item, label: td(`ai.apps.sources.${knownSource(item)}`) }))}
          onChange={setSource}
        />
      </Field>
      <Field label={t("ai.apps.query")}>
        <TextInput value={text} onChange={setText} wide />
      </Field>
      <Field label={t("ai.apps.modelKind")}>
        <Select
          value={kind}
          options={KINDS.map((item) => ({ value: item, label: td(`ai.apps.kind.${knownKind(item)}`) }))}
          onChange={setKind}
        />
      </Field>
      <Field label={t("ai.apps.sort")}>
        <Select
          value={sort}
          options={(["downloads", "rating", "newest"] as const).map((item) => ({
            value: item,
            label: td(`ai.apps.sorts.${knownSort(item)}`),
          }))}
          onChange={setSort}
        />
      </Field>
      <Field label={t("ai.apps.period")}>
        <Select
          value={period}
          options={(["all", "year", "month", "week", "day"] as const).map((item) => ({
            value: item,
            label: td(`ai.apps.periods.${knownPeriod(item)}`),
          }))}
          onChange={setPeriod}
        />
      </Field>
      <Field label={t("ai.apps.baseModel")}>
        <TextInput value={baseModel} onChange={setBaseModel} wide />
      </Field>
      <Checkbox checked={hideInstalled} onChange={setHideInstalled} label={t("ai.apps.hideInstalled")} />
      <Checkbox
        checked={showNsfw}
        onChange={(value) => {
          const next = structuredClone(ai.settings);
          next.apps.showNsfw = value;
          void onSettings(next);
        }}
        label={t("ai.apps.showNsfw")}
      />
      <Button onClick={() => ask("", false)}>{loading ? t("common.working") : t("ai.apps.searchButton")}</Button>
      {hits.length > 0 ? (
        <div className="model-views">
          <Segmented
            value={view}
            options={(["grid", "list"] as const).map((item) => ({
              value: item,
              label: td(`ai.apps.view.${knownView(item)}`),
            }))}
            onChange={setView}
          />
        </div>
      ) : null}
      {view === "list"
        ? hits.map((hit) => {
            const job = jobFor(jobs, hit.fileName);
            return (
              <div key={`${hit.source}-${hit.url}`} className="component model-row" onClick={() => openHit(hit)}>
                <div className="component-main">
                  <div className="component-name">{hit.name}</div>
                  <div className="component-facts">
                    {hit.fileName}
                    {hit.baseModel ? ` · ${hit.baseModel}` : ""}
                    {worksLocally(hit) ? <Tag tone="ok">{t("ai.apps.worksLocally")}</Tag> : null}
                  </div>
                  {job ? <LoadState job={job} /> : null}
                </div>
                <LoadButton hit={hit} job={job} primary />
              </div>
            );
          })
        : null}
      {view === "grid" ? (
        <div className="model-grid">
          {hits.map((hit) => {
            const job = jobFor(jobs, hit.fileName);
            return (
              <div key={`${hit.source}-${hit.url}`} className="model-card" onClick={() => openHit(hit)}>
                <ModelPicture url={hit.previews[0] ?? ""} frame="card" />
                <div className="model-card-name" title={hit.name}>
                  {hit.name}
                </div>
                <div className="component-facts">
                  {hit.baseModel || hit.fileName}
                  {worksLocally(hit) ? <Tag tone="ok">{t("ai.apps.worksLocally")}</Tag> : null}
                </div>
                {job ? <LoadState job={job} /> : null}
                <LoadButton hit={hit} job={job} primary />
              </div>
            );
          })}
        </div>
      ) : null}
      {opened ? (
        <Modal
          wide
          title={opened.name}
          onClose={() => setOpened(null)}
          footer={
            <Button
              variant="primary"
              disabled={Boolean(openedJob) || !chosen}
              onClick={() => chosen && api.appsDownload(chosen).catch(showError)}
            >
              {t("ai.apps.download")}
            </Button>
          }
        >
          <div className="model-shot">
            {shownShot ? <ModelPicture url={shownShot} frame="stage" /> : <div className="model-picture model-picture-stage"><div className="model-picture-empty" /></div>}
            {shots.length > 1 ? (
              <div className="model-film">
                {shots.map((url, index) => (
                  <button
                    key={url}
                    type="button"
                    className={index === shot ? "model-film-item model-film-on" : "model-film-item"}
                    onClick={() => setShot(index)}
                  >
                    <ModelPicture url={url} frame="film" />
                  </button>
                ))}
              </div>
            ) : null}
            <div className="component-facts">
              {chosen?.fileName}
              {opened.baseModel ? ` · ${opened.baseModel}` : ""}
              {worksLocally(opened) ? <Tag tone="ok">{t("ai.apps.worksLocally")}</Tag> : null}
            </div>
            {(opened.files?.length ?? 0) > 1 ? (
              <Field label={t("ai.apps.file")}>
                <Select
                  value={String(fileIndex)}
                  options={(opened.files ?? []).map((file, index) => ({
                    value: String(index),
                    label: file.format ? `${file.fileName} · ${file.format}` : file.fileName,
                  }))}
                  onChange={(value) => setFileIndex(Number(value))}
                />
              </Field>
            ) : null}
            {openedJob ? <JobRow job={openedJob} /> : null}
          </div>
        </Modal>
      ) : null}
      <div ref={sentinel} className="model-more">
        {loading && hits.length > 0 ? t("common.working") : ""}
      </div>
    </Section>
  );
}

export function AppsTab({
  ai,
  onSettings,
}: {
  ai: AiStatus;
  onSettings: (settings: AiSettings) => Promise<boolean>;
}) {
  const { t } = useT();
  const [status, setStatus] = useState<AppsStatus | null>(null);

  const refresh = () => {
    api
      .appsStatus()
      .then(setStatus)
      .catch(() => setStatus(null));
  };

  useEffect(() => {
    refresh();
  }, [ai.settings]);

  // A program that stops by itself is noticed without a click.
  const running = status ? PROGRAMS.some((program) => status[program].state === "running") : false;
  useEffect(() => {
    if (!running) {
      return undefined;
    }
    const timer = window.setInterval(refresh, STATUS_EVERY);
    return () => window.clearInterval(timer);
  }, [running]);

  return (
    <>
      <Section title={t("ai.apps.title")}>
        <p className="tool-text">{t("ai.apps.text")}</p>
        {status
          ? PROGRAMS.map((program) => (
              <ProgramCard key={program} card={status[program]} status={status} onChanged={refresh} />
            ))
          : null}
      </Section>
      {status ? <Folders ai={ai} status={status} onSettings={onSettings} onChanged={refresh} /> : null}
      <Search ai={ai} onSettings={onSettings} />
    </>
  );
}
