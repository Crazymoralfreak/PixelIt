// Background work of the core: batches, downloads, generation.

import { api } from "../api";
import { useApp } from "../app/context";
import { useT } from "../i18n";
import { formatBytes, formatDuration } from "../lib/format";
import type { JobInfo } from "../types";
import { Button, EmptyState, Modal, Progress, Tag } from "../ui";

function isActive(job: JobInfo): boolean {
  return job.state === "queued" || job.state === "running" || job.state === "paused";
}

export function activeJobs(jobs: JobInfo[]): JobInfo[] {
  return jobs.filter(isActive);
}

/** Progress of a job in words: files for most jobs, bytes for downloads. */
export function jobAmount(job: JobInfo): string {
  if (job.total <= 0) {
    return "";
  }
  return job.kind === "install"
    ? `${formatBytes(job.done)} / ${formatBytes(job.total)}`
    : `${job.done} / ${job.total}`;
}

export function JobRow({ job }: { job: JobInfo }) {
  const { t, td, error } = useT();
  const { showError } = useApp();
  const active = isActive(job);
  const tone = job.state === "failed" ? "danger" : job.state === "done" ? "ok" : "plain";

  return (
    <div className="job">
      <div className="job-head">
        <span className="job-title">
          {td(`jobs.kind.${job.kind}`, undefined, job.kind)}
          {job.title ? `: ${job.title}` : ""}
        </span>
        <Tag tone={tone}>{td(`jobs.state.${job.state}`)}</Tag>
      </div>
      {active ? <Progress done={job.done} total={job.state === "queued" ? 1 : job.total} /> : null}
      <div className="job-foot">
        <span className="job-current">
          {job.error ? error(job.error) : [jobAmount(job), job.current].filter(Boolean).join(" · ")}
        </span>
        {active && job.etaSeconds !== null ? (
          <span className="job-eta">{t("jobs.eta", { time: formatDuration(job.etaSeconds) })}</span>
        ) : null}
        {active ? (
          <span className="job-actions">
            {job.state === "queued" ? null : (
              <Button
                small
                onClick={() => api.jobPause(job.id, job.state !== "paused").catch(showError)}
              >
                {job.state === "paused" ? t("jobs.resume") : t("jobs.pause")}
              </Button>
            )}
            <Button small onClick={() => api.jobCancel(job.id).catch(showError)}>
              {t("common.cancel")}
            </Button>
          </span>
        ) : null}
      </div>
    </div>
  );
}

export function JobsDialog({ onClose, onClear }: { onClose: () => void; onClear: () => void }) {
  const { t } = useT();
  const { jobs } = useApp();
  const ordered = [...jobs].sort((a, b) => b.id - a.id);

  return (
    <Modal
      title={t("jobs.title")}
      onClose={onClose}
      wide
      footer={
        <>
          <Button onClick={onClear} disabled={jobs.every(isActive)}>
            {t("jobs.clear")}
          </Button>
          <Button variant="primary" onClick={onClose}>
            {t("common.close")}
          </Button>
        </>
      }
    >
      {ordered.length === 0 ? (
        <EmptyState title={t("jobs.empty")} />
      ) : (
        <div className="job-list">
          {ordered.map((job) => (
            <JobRow key={job.id} job={job} />
          ))}
        </div>
      )}
    </Modal>
  );
}
