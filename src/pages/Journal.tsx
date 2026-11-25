// The journal: what batches, watched folders and agents did, with undo.

import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import { useAppEvent } from "../api/events";
import { useApp } from "../app/context";
import { useT } from "../i18n";
import { fileName, formatBytes, formatDuration, formatSaving, formatTime } from "../lib/format";
import type { JournalEntry } from "../types";
import { Button, EmptyState, PageFrame, Tag } from "../ui";

export default function JournalPage({ active }: { active: boolean }) {
  const { t, td, error, language } = useT();
  const { confirm, notify, showError } = useApp();
  const [entries, setEntries] = useState<JournalEntry[]>([]);
  const [open, setOpen] = useState<string | null>(null);
  /** The download whose sources are listed. */
  const [sources, setSources] = useState<string | null>(null);

  const refresh = useCallback(() => {
    api.journalList().then(setEntries).catch(showError);
  }, [showError]);

  useEffect(() => {
    if (active) {
      refresh();
    }
  }, [active, refresh]);
  useAppEvent("journal:changed", refresh);

  const undo = async (entry: JournalEntry) => {
    if (!entry.undoId) {
      return;
    }
    const agreed = await confirm({
      title: t("journal.undoTitle"),
      text: t("journal.undoText"),
      confirmLabel: t("journal.undo"),
    });
    if (!agreed) {
      return;
    }
    try {
      const report = await api.journalUndo(entry.undoId);
      notify(t("journal.undone", { removed: report.removed, restored: report.restored }));
      if (report.changed + report.missing > 0) {
        notify(t("journal.undoneLeft", { changed: report.changed, missing: report.missing }));
      }
      refresh();
    } catch (failure) {
      showError(failure);
    }
  };

  const clear = async () => {
    const agreed = await confirm({
      title: t("journal.clearTitle"),
      text: t("journal.clearText"),
      confirmLabel: t("common.clear"),
      danger: true,
    });
    if (agreed) {
      api.journalClear().then(refresh).catch(showError);
    }
  };

  return (
    <PageFrame
      title={t("nav.journal")}
      actions={
        <Button disabled={entries.length === 0} onClick={() => void clear()}>
          {t("common.clear")}
        </Button>
      }
    >
      {entries.length === 0 ? (
        <EmptyState title={t("journal.emptyTitle")} text={t("journal.emptyText")} />
      ) : (
        <div className="journal">
          {entries.map((entry) => (
            <div key={entry.id} className="journal-entry">
              <div className="journal-head">
                <span className="journal-time">{formatTime(entry.time, language)}</span>
                <span className="journal-title">
                  {td(`jobs.kind.${entry.kind}`, undefined, entry.kind)}
                  {entry.title ? `: ${entry.title}` : ""}
                </span>
                {entry.cancelled ? <Tag>{t("jobs.state.cancelled")}</Tag> : null}
                {entry.failed > 0 ? <Tag tone="danger">{t("journal.failed", { count: entry.failed })}</Tag> : null}
              </div>
              {entry.kind === "download" ? (
                // A download has nothing to compare its size with.
                <div className="journal-text">
                  {t("journal.downloadText", {
                    done: entry.done,
                    failed: entry.failed,
                    size: formatBytes(entry.outputBytes),
                    time: formatDuration(entry.elapsedMs / 1000),
                  })}
                </div>
              ) : (
                <div className="journal-text">
                  {t("batch.reportCounts", { done: entry.done, skipped: entry.skipped, failed: entry.failed })}
                  {entry.done > 0
                    ? ` · ${t("batch.reportSize", {
                        before: formatBytes(entry.sourceBytes),
                        after: formatBytes(entry.outputBytes),
                        saving: formatSaving(entry.sourceBytes, entry.outputBytes),
                        time: formatDuration(entry.elapsedMs / 1000),
                      })}`
                    : ""}
                </div>
              )}
              {sources === entry.id
                ? (entry.origins ?? []).map((origin) => (
                    <div key={origin.path} className="journal-origin" title={origin.path}>
                      <div>
                        {[fileName(origin.path), origin.author, origin.license].filter(Boolean).join(" · ")}
                      </div>
                      <div className="journal-origin-page">{origin.page || origin.url}</div>
                    </div>
                  ))
                : null}
              {open === entry.id
                ? entry.errors.map((item) => (
                    <div key={item.path} className="report-error" title={item.path}>
                      {fileName(item.path)}: {item.error ? error(item.error) : ""}
                    </div>
                  ))
                : null}
              <div className="journal-actions">
                {entry.folder ? (
                  <Button small onClick={() => api.revealPath(entry.folder).catch(showError)}>
                    {t("batch.openFolder")}
                  </Button>
                ) : null}
                {entry.undoId ? (
                  <Button small onClick={() => void undo(entry)}>
                    {t("journal.undo")}
                  </Button>
                ) : null}
                {entry.origins && entry.origins.length > 0 ? (
                  <Button small variant="ghost" onClick={() => setSources(sources === entry.id ? null : entry.id)}>
                    {sources === entry.id ? t("journal.hideSources") : t("journal.showSources")}
                  </Button>
                ) : null}
                {entry.errors.length > 0 ? (
                  <Button small variant="ghost" onClick={() => setOpen(open === entry.id ? null : entry.id)}>
                    {open === entry.id ? t("journal.hideErrors") : t("journal.showErrors")}
                  </Button>
                ) : null}
              </div>
            </div>
          ))}
        </div>
      )}
    </PageFrame>
  );
}
