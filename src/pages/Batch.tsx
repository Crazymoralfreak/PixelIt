// The batch: a queue of files and folders, the steps, the output settings
// and the run with its report.

import { useEffect, useState } from "react";
import { api } from "../api";
import { useApp } from "../app/context";
import { JobRow } from "../components/JobsPanel";
import { OutputEditor } from "../components/OutputEditor";
import { StepsEditor } from "../components/StepsEditor";
import { useT } from "../i18n";
import { fileName, formatBytes, formatDuration, formatSaving, newId } from "../lib/format";
import { isDestructive, recipeName } from "../lib/pipeline";
import type { BatchReport } from "../types";
import { Button, Checkbox, EmptyState, PageFrame, Section, Select } from "../ui";

/** Errors of a report shown before "and more". */
const ERRORS_SHOWN = 30;

export function ReportView({ report, onUndone }: { report: BatchReport; onUndone?: () => void }) {
  const { t, error } = useT();
  const { notify, showError } = useApp();
  const failed = report.items.filter((item) => item.error);

  const undo = async () => {
    if (!report.undoId) {
      return;
    }
    try {
      const result = await api.journalUndo(report.undoId);
      notify(t("journal.undone", { removed: result.removed, restored: result.restored }));
      onUndone?.();
    } catch (failure) {
      showError(failure);
    }
  };

  return (
    <div className="report">
      <div className="report-line">
        {t("batch.reportCounts", { done: report.done, skipped: report.skipped, failed: report.failed })}
        {report.cancelled ? ` · ${t("jobs.state.cancelled")}` : ""}
      </div>
      {report.done > 0 ? (
        <div className="report-line">
          {t("batch.reportSize", {
            before: formatBytes(report.sourceBytes),
            after: formatBytes(report.outputBytes),
            saving: formatSaving(report.sourceBytes, report.outputBytes),
            time: formatDuration(report.elapsedMs / 1000),
          })}
        </div>
      ) : null}
      {failed.slice(0, ERRORS_SHOWN).map((item) => (
        <div key={item.source} className="report-error">
          {fileName(item.source)}: {item.error ? error(item.error) : ""}
        </div>
      ))}
      {failed.length > ERRORS_SHOWN ? (
        <div className="report-error">{t("batch.moreErrors", { count: failed.length - ERRORS_SHOWN })}</div>
      ) : null}
      <div className="report-actions">
        {report.folder ? (
          <Button small onClick={() => api.revealPath(report.folder).catch(showError)}>
            {t("batch.openFolder")}
          </Button>
        ) : null}
        {report.undoId ? (
          <Button small onClick={() => void undo()}>
            {t("journal.undo")}
          </Button>
        ) : null}
      </div>
    </div>
  );
}

export default function BatchPage() {
  const { t, td } = useT();
  const {
    settings,
    queue,
    setQueue,
    addToQueue,
    batchPipeline,
    setBatchPipeline,
    recipes,
    refreshRecipes,
    jobs,
    waitJob,
    confirm,
    askName,
    notify,
    showError,
    withCloud,
  } = useApp();

  const [recursive, setRecursive] = useState(true);
  const [count, setCount] = useState<number | null>(null);
  const [recipeId, setRecipeId] = useState("");
  const [jobId, setJobId] = useState<number | null>(null);
  const [report, setReport] = useState<BatchReport | null>(null);
  const [starting, setStarting] = useState(false);

  const job = jobId === null ? undefined : jobs.find((item) => item.id === jobId);
  const running = starting || (job !== undefined && job.state !== "done" && job.state !== "failed" && job.state !== "cancelled");

  useEffect(() => {
    let alive = true;
    setCount(null);
    if (queue.length === 0) {
      setCount(0);
      return;
    }
    const handle = window.setTimeout(() => {
      api
        .filesExpand(queue, recursive)
        .then((files) => alive && setCount(files.length))
        .catch(() => undefined);
    }, 200);
    return () => {
      alive = false;
      window.clearTimeout(handle);
    };
  }, [queue, recursive]);

  const addFiles = async () => addToQueue(await api.pickFiles(true));
  const addFolder = async () => {
    const folder = await api.pickFolder(settings.recentFolders[0]);
    if (folder) {
      addToQueue([folder]);
    }
  };

  const applyRecipe = (id: string) => {
    setRecipeId(id);
    const recipe = recipes.find((item) => item.id === id);
    if (recipe) {
      setBatchPipeline({
        ...recipe.pipeline,
        steps: recipe.pipeline.steps.map((step) => ({ ...step, id: newId() })),
      });
    }
  };

  const saveRecipe = async () => {
    const name = await askName({ title: t("recipes.saveTitle"), label: t("recipes.name") });
    if (!name) {
      return;
    }
    try {
      const saved = await api.recipeSave({
        id: "",
        name,
        description: "",
        pipeline: batchPipeline,
        builtin: false,
        pinned: false,
        updated: 0,
      });
      refreshRecipes();
      setRecipeId(saved.id);
      notify(t("recipes.saved", { name }));
    } catch (error) {
      showError(error);
    }
  };

  const start = async () => {
    if (isDestructive(batchPipeline) && settings.confirmOverwrite) {
      const agreed = await confirm({
        title: t("batch.overwriteTitle"),
        text: t("batch.overwriteText"),
        confirmLabel: t("common.continue"),
        danger: true,
      });
      if (!agreed) {
        return;
      }
    }
    const recipe = recipes.find((item) => item.id === recipeId);
    setStarting(true);
    setReport(null);
    try {
      const id = await withCloud((confirmed) =>
        api.batchStart({
          title: recipe ? recipeName(recipe, td) : "",
          inputs: queue,
          recursive,
          pipeline: batchPipeline,
          confirmed,
        }),
      );
      setJobId(id);
      setStarting(false);
      const finished = await waitJob(id);
      if (finished.state === "done") {
        setReport(finished.result as BatchReport);
      } else if (finished.error) {
        showError(finished.error);
      }
    } catch (error) {
      showError(error);
    } finally {
      setStarting(false);
    }
  };

  return (
    <PageFrame
      title={t("nav.batch")}
      actions={
        <>
          <Button onClick={() => void addFiles()}>{t("batch.addFiles")}</Button>
          <Button onClick={() => void addFolder()}>{t("batch.addFolder")}</Button>
          <Button variant="primary" disabled={running || queue.length === 0 || count === 0} onClick={() => void start()}>
            {running ? t("common.working") : t("batch.start")}
          </Button>
        </>
      }
    >
      <div className="columns">
        <div className="column">
          <Section
            title={t("batch.queue")}
            actions={
              <Button small variant="ghost" disabled={queue.length === 0} onClick={() => setQueue([])}>
                {t("common.clear")}
              </Button>
            }
          >
            {queue.length === 0 ? (
              <EmptyState title={t("batch.emptyTitle")} text={t("batch.emptyText")} />
            ) : (
              <>
                <div className="queue">
                  {queue.map((input) => (
                    <div key={input} className="queue-item" title={input}>
                      <span className="queue-name">{fileName(input)}</span>
                      <Button small variant="ghost" onClick={() => setQueue(queue.filter((item) => item !== input))}>
                        ✕
                      </Button>
                    </div>
                  ))}
                </div>
                <div className="queue-foot">
                  <Checkbox checked={recursive} onChange={setRecursive} label={t("batch.recursive")} />
                  <span>{count === null ? t("common.working") : t("batch.pictures", { count })}</span>
                </div>
              </>
            )}
          </Section>

          {job || report ? (
            <Section title={t("batch.result")}>
              {job && !report ? <JobRow job={job} /> : null}
              {report ? <ReportView report={report} onUndone={() => setReport(null)} /> : null}
            </Section>
          ) : null}

          <Section
            title={t("batch.steps")}
            actions={
              <>
                <Select
                  value={recipes.some((recipe) => recipe.id === recipeId) ? recipeId : ""}
                  options={[
                    { value: "", label: t("batch.chooseRecipe") },
                    ...recipes.map((recipe) => ({ value: recipe.id, label: recipeName(recipe, td) })),
                  ]}
                  onChange={applyRecipe}
                />
                <Button small onClick={() => void saveRecipe()}>
                  {t("recipes.saveAs")}
                </Button>
              </>
            }
          >
            <StepsEditor
              steps={batchPipeline.steps}
              onChange={(steps) => setBatchPipeline({ ...batchPipeline, steps })}
            />
          </Section>
        </div>
        <div className="column">
          <Section title={t("batch.output")}>
            <OutputEditor
              value={batchPipeline.output}
              onChange={(output) => setBatchPipeline({ ...batchPipeline, output })}
            />
          </Section>
        </div>
      </div>
    </PageFrame>
  );
}
