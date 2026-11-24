// The shell: boot, navigation, shared state, dialogs, toasts and everything
// that arrives from outside the window (drops, the clipboard, a second start).

import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, toAppError } from "./api";
import { useAppEvent } from "./api/events";
import {
  AppContext,
  PAGES,
  type AppContextValue,
  type AskNameOptions,
  type ConfirmOptions,
  type Incoming,
  type Page,
} from "./app/context";
import { parseChangelog, ReleaseNotes, type Release } from "./components/Changelog";
import { activeJobs, JobsDialog } from "./components/JobsPanel";
import { I18nProvider, useT } from "./i18n";
import { blobToBase64, pastedPicture } from "./lib/clipboard";
import { newPipeline } from "./lib/pipeline";
import AboutPage from "./pages/About";
import AiPage from "./pages/Ai";
import AssistantPage from "./pages/Assistant";
import BatchPage from "./pages/Batch";
import EditorPage from "./pages/Editor";
import JournalPage from "./pages/Journal";
import RecipesPage from "./pages/Recipes";
import SettingsPage from "./pages/Settings";
import StudioPage from "./pages/Studio";
import ToolsPage from "./pages/Tools";
import ViewerPage from "./pages/Viewer";
import type { AiStatus, AppError, AppInfo, JobInfo, Pipeline, Recipe, Settings, UpdateStatus } from "./types";
import { Button, cx, Modal, TextInput } from "./ui";

interface Toast {
  id: number;
  text: string;
  error: boolean;
}

interface Boot {
  info: AppInfo;
  settings: Settings;
  jobs: JobInfo[];
  recipes: Recipe[];
  update: UpdateStatus;
  opened: string[];
}

const TOAST_MS = 6000;

function isFinished(job: JobInfo): boolean {
  return job.state === "done" || job.state === "failed" || job.state === "cancelled";
}

function pageFromHash(): Page {
  const name = window.location.hash.replace(/^#\/?/, "");
  return (PAGES as readonly string[]).includes(name) ? (name as Page) : "viewer";
}

function ConfirmDialog({
  options,
  onDone,
}: {
  options: ConfirmOptions;
  onDone: (confirmed: boolean) => void;
}) {
  const { t } = useT();
  return (
    <Modal
      title={options.title}
      onClose={() => onDone(false)}
      footer={
        <>
          <Button onClick={() => onDone(false)}>{t("common.cancel")}</Button>
          <Button variant={options.danger ? "danger" : "primary"} autoFocus onClick={() => onDone(true)}>
            {options.confirmLabel ?? t("common.ok")}
          </Button>
        </>
      }
    >
      <div className="modal-text">{options.text}</div>
    </Modal>
  );
}

function NameDialog({ options, onDone }: { options: AskNameOptions; onDone: (name: string | null) => void }) {
  const { t } = useT();
  const [name, setName] = useState(options.initial ?? "");
  const trimmed = name.trim();
  return (
    <Modal
      title={options.title}
      onClose={() => onDone(null)}
      footer={
        <>
          <Button onClick={() => onDone(null)}>{t("common.cancel")}</Button>
          <Button variant="primary" disabled={trimmed === ""} onClick={() => onDone(trimmed)}>
            {options.confirmLabel ?? t("common.save")}
          </Button>
        </>
      }
    >
      <label className="modal-label">{options.label}</label>
      <TextInput
        value={name}
        onChange={setName}
        wide
        autoFocus
        onKeyDown={(event) => {
          if (event.key === "Enter" && trimmed !== "") {
            onDone(trimmed);
          }
        }}
      />
    </Modal>
  );
}

function PageView({ page, active }: { page: Page; active: boolean }) {
  switch (page) {
    case "viewer":
      return <ViewerPage active={active} />;
    case "editor":
      return <EditorPage active={active} />;
    case "batch":
      return <BatchPage />;
    case "recipes":
      return <RecipesPage />;
    case "tools":
      return <ToolsPage />;
    case "studio":
      return <StudioPage />;
    case "assistant":
      return <AssistantPage active={active} />;
    case "journal":
      return <JournalPage active={active} />;
    case "ai":
      return <AiPage active={active} />;
    case "settings":
      return <SettingsPage />;
    case "about":
      return <AboutPage />;
    default: {
      const unreachable: never = page;
      return unreachable;
    }
  }
}

function Shell({ boot, onInfo }: { boot: Boot; onInfo: (info: AppInfo) => void }) {
  const { t, td, error: errorText } = useT();
  const [settings, setSettings] = useState(boot.settings);
  const [update, setUpdate] = useState(boot.update);
  const [page, setPage] = useState<Page>(pageFromHash);
  const [visited, setVisited] = useState<Page[]>(() => [pageFromHash()]);
  const [jobs, setJobs] = useState(boot.jobs);
  const [ai, setAi] = useState<AiStatus | null>(null);
  const [recipes, setRecipes] = useState(boot.recipes);
  const [queue, setQueue] = useState<string[]>([]);
  const [batchPipeline, setBatchPipeline] = useState<Pipeline>(() => newPipeline(boot.settings.output));
  const [incoming, setIncoming] = useState<Incoming | null>(null);
  const [editorPath, setEditorPath] = useState<string | null>(null);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [confirming, setConfirming] = useState<{ options: ConfirmOptions; done: (ok: boolean) => void } | null>(
    null,
  );
  const [naming, setNaming] = useState<{ options: AskNameOptions; done: (name: string | null) => void } | null>(
    null,
  );
  const [jobsOpen, setJobsOpen] = useState(false);
  const [dropping, setDropping] = useState(false);
  const [news, setNews] = useState<Release | null>(null);

  const nextToast = useRef(1);
  const nextIncoming = useRef(1);
  const latestJobs = useRef(new Map(boot.jobs.map((job) => [job.id, job])));
  const waiters = useRef(new Map<number, Array<(job: JobInfo) => void>>());

  const navigate = useCallback((next: Page) => {
    window.location.hash = `/${next}`;
  }, []);

  useEffect(() => {
    const onHash = () => {
      const next = pageFromHash();
      setPage(next);
      setVisited((current) => (current.includes(next) ? current : [...current, next]));
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  useEffect(() => {
    if (settings.theme === "system") {
      delete document.documentElement.dataset.theme;
    } else {
      document.documentElement.dataset.theme = settings.theme;
    }
  }, [settings.theme]);

  const pushToast = useCallback((text: string, error: boolean) => {
    const id = nextToast.current++;
    setToasts((current) => [...current.slice(-3), { id, text, error }]);
    window.setTimeout(() => setToasts((current) => current.filter((toast) => toast.id !== id)), TOAST_MS);
  }, []);

  const notify = useCallback((text: string) => pushToast(text, false), [pushToast]);

  const showError = useCallback(
    (error: unknown) => {
      const appError = toAppError(error);
      if (appError.code !== "cancelled") {
        pushToast(errorText(appError), true);
      }
    },
    [errorText, pushToast],
  );

  const confirm = useCallback(
    (options: ConfirmOptions) =>
      new Promise<boolean>((resolve) => {
        setConfirming({
          options,
          done: (ok) => {
            setConfirming(null);
            resolve(ok);
          },
        });
      }),
    [],
  );

  const askName = useCallback(
    (options: AskNameOptions) =>
      new Promise<string | null>((resolve) => {
        setNaming({
          options,
          done: (name) => {
            setNaming(null);
            resolve(name);
          },
        });
      }),
    [],
  );

  const withCloud = useCallback(
    async <T,>(call: (confirmed: boolean) => Promise<T>): Promise<T> => {
      try {
        return await call(false);
      } catch (error) {
        if (toAppError(error).code !== "cloudConfirm") {
          throw error;
        }
      }
      const agreed = await confirm({
        title: t("cloud.confirmTitle"),
        text: t("cloud.confirmText"),
        confirmLabel: t("cloud.send"),
      });
      if (!agreed) {
        const cancelled: AppError = { code: "cancelled" };
        throw cancelled;
      }
      return call(true);
    },
    [confirm, t],
  );

  const saveSettings = useCallback(
    async (next: Settings) => {
      try {
        const saved = await api.settingsSet(next);
        setSettings(saved);
        if (saved.language !== settings.language) {
          onInfo(await api.appInfo());
        }
        return true;
      } catch (error) {
        showError(error);
        return false;
      }
    },
    [onInfo, settings.language, showError],
  );

  const waitJob = useCallback(
    (id: number) =>
      new Promise<JobInfo>((resolve) => {
        const known = latestJobs.current.get(id);
        if (known && isFinished(known)) {
          resolve(known);
          return;
        }
        waiters.current.set(id, [...(waiters.current.get(id) ?? []), resolve]);
      }),
    [],
  );

  const jobResult = useCallback(
    async <T,>(id: number): Promise<T> => {
      const job = await waitJob(id);
      if (job.state === "done") {
        return job.result as T;
      }
      const failure: AppError = job.error ?? { code: "cancelled" };
      throw failure;
    },
    [waitJob],
  );

  useAppEvent("jobs:changed", (job) => {
    latestJobs.current.set(job.id, job);
    setJobs((current) => {
      const index = current.findIndex((known) => known.id === job.id);
      return index < 0 ? [...current, job] : current.map((known) => (known.id === job.id ? job : known));
    });
    if (isFinished(job)) {
      waiters.current.get(job.id)?.forEach((resolve) => resolve(job));
      waiters.current.delete(job.id);
    }
  });

  const refreshAi = useCallback(() => {
    api.aiStatus().then(setAi).catch(showError);
  }, [showError]);

  const refreshRecipes = useCallback(() => {
    api.recipesList().then(setRecipes).catch(showError);
  }, [showError]);

  useEffect(refreshAi, [refreshAi]);
  useAppEvent("ai:changed", refreshAi);
  useAppEvent("settings:changed", setSettings);
  useAppEvent("update:status", setUpdate);

  const openPaths = useCallback(
    (paths: string[]) => {
      if (paths.length === 0) {
        return;
      }
      setIncoming({ paths, token: nextIncoming.current++ });
      navigate("viewer");
    },
    [navigate],
  );

  const openInEditor = useCallback(
    (path: string) => {
      setEditorPath(path);
      navigate("editor");
    },
    [navigate],
  );

  const addToQueue = useCallback((inputs: string[]) => {
    setQueue((current) => [...current, ...inputs.filter((input) => !current.includes(input))]);
  }, []);

  useAppEvent("open:files", openPaths);
  useAppEvent("capture:done", openInEditor);

  // Files the application was started with, and the news of a new version.
  useEffect(() => {
    openPaths(boot.opened);
    if (boot.settings.seenVersion === boot.info.version) {
      return;
    }
    const fresh = boot.settings.seenVersion === "";
    void api
      .changelog()
      .then((text) => {
        const release = parseChangelog(text).find((entry) => entry.version === boot.info.version);
        if (release && !fresh) {
          setNews(release);
        }
        return api.settingsSet({ ...boot.settings, seenVersion: boot.info.version });
      })
      .then(setSettings)
      .catch(() => undefined);
    // Runs once, with the state the application started in.
  }, []);

  // Files and folders dropped on the window.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        switch (payload.type) {
          case "enter":
            setDropping(payload.paths.length > 0);
            break;
          case "over":
            break;
          case "leave":
            setDropping(false);
            break;
          case "drop":
            setDropping(false);
            openPaths(payload.paths);
            break;
          default: {
            const unreachable: never = payload;
            throw new Error(`Unknown drop event: ${String(unreachable)}`);
          }
        }
      })
      .then((stop) => {
        if (cancelled) {
          stop();
        } else {
          unlisten = stop;
        }
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [openPaths]);

  // A picture pasted from the clipboard opens in the editor.
  useEffect(() => {
    const onPaste = (event: ClipboardEvent) => {
      const picture = pastedPicture(event.clipboardData);
      if (!picture) {
        return;
      }
      event.preventDefault();
      blobToBase64(picture).then(api.pasteImage).then(openInEditor).catch(showError);
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [openInEditor, showError]);

  const clearJobs = useCallback(() => {
    api
      .jobsClear()
      .then(api.jobsList)
      .then((list) => {
        latestJobs.current = new Map(list.map((job) => [job.id, job]));
        setJobs(list);
      })
      .catch(showError);
  }, [showError]);

  const context = useMemo<AppContextValue>(
    () => ({
      info: boot.info,
      settings,
      saveSettings,
      update,
      page,
      navigate,
      jobs,
      waitJob,
      jobResult,
      ai,
      refreshAi,
      recipes,
      refreshRecipes,
      queue,
      setQueue,
      addToQueue,
      batchPipeline,
      setBatchPipeline,
      incoming,
      openPaths,
      editorPath,
      openInEditor,
      confirm,
      askName,
      notify,
      showError,
      withCloud,
    }),
    [
      boot.info,
      settings,
      saveSettings,
      update,
      page,
      navigate,
      jobs,
      waitJob,
      jobResult,
      ai,
      refreshAi,
      recipes,
      refreshRecipes,
      queue,
      addToQueue,
      batchPipeline,
      incoming,
      openPaths,
      editorPath,
      openInEditor,
      confirm,
      askName,
      notify,
      showError,
      withCloud,
    ],
  );

  const running = activeJobs(jobs);
  const updateNote: ReactNode =
    update.state === "available" || update.state === "downloading" || update.state === "installing" ? (
      <button type="button" className="sidebar-note" onClick={() => navigate("settings")}>
        {td(`update.state.${update.state}`, { version: update.version ?? "" })}
      </button>
    ) : null;

  return (
    <AppContext.Provider value={context}>
      <div className="app">
        <nav className="sidebar">
          <div className="sidebar-brand">PixelIt</div>
          {PAGES.map((item) => (
            <button
              key={item}
              type="button"
              className={cx("sidebar-item", item === page && "sidebar-item-active")}
              onClick={() => navigate(item)}
            >
              {td(`nav.${item}`)}
              {item === "batch" && queue.length > 0 ? <span className="sidebar-count">{queue.length}</span> : null}
            </button>
          ))}
          <div className="sidebar-spacer" />
          {updateNote}
          <button type="button" className="sidebar-jobs" onClick={() => setJobsOpen(true)}>
            {running.length > 0 ? t("jobs.running", { count: running.length }) : t("jobs.title")}
          </button>
        </nav>
        <main className="app-main">
          {visited.map((item) => (
            <div key={item} className="app-page" hidden={item !== page}>
              <PageView page={item} active={item === page} />
            </div>
          ))}
        </main>
        {dropping ? <div className="drop-overlay">{t("viewer.dropHere")}</div> : null}
      </div>

      {jobsOpen ? <JobsDialog onClose={() => setJobsOpen(false)} onClear={clearJobs} /> : null}
      {news ? (
        <Modal
          title={t("about.whatsNewIn", { version: news.version })}
          onClose={() => setNews(null)}
          wide
          footer={
            <Button variant="primary" onClick={() => setNews(null)}>
              {t("common.close")}
            </Button>
          }
        >
          <ReleaseNotes release={news} />
        </Modal>
      ) : null}
      {confirming ? <ConfirmDialog options={confirming.options} onDone={confirming.done} /> : null}
      {naming ? <NameDialog options={naming.options} onDone={naming.done} /> : null}

      <div className="toasts">
        {toasts.map((toast) => (
          <div key={toast.id} className={cx("toast", toast.error && "toast-error")}>
            {toast.text}
          </div>
        ))}
      </div>
    </AppContext.Provider>
  );
}

export default function App() {
  const [boot, setBoot] = useState<Boot | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    Promise.all([
      api.appInfo(),
      api.settingsGet(),
      api.jobsList(),
      api.recipesList(),
      api.updateStatus(),
      api.openedFiles(),
    ])
      .then(([info, settings, jobs, recipes, update, opened]) =>
        setBoot({ info, settings, jobs, recipes, update, opened }),
      )
      .catch((error: unknown) => setFailure(toAppError(error).params?.message ?? toAppError(error).code));
  }, []);

  if (failure) {
    return <div className="boot-error">{failure}</div>;
  }
  if (!boot) {
    return null;
  }
  return (
    <I18nProvider language={boot.info.language}>
      <Shell boot={boot} onInfo={(info) => setBoot((current) => (current ? { ...current, info } : current))} />
    </I18nProvider>
  );
}
