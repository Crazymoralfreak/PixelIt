// A model that looks at pictures: description, alt text, tags, a file name
// or the text written in the picture.

import { useState } from "react";
import { api } from "../../api";
import { thumbUrl } from "../../api/urls";
import { useApp } from "../../app/context";
import { FilesField } from "../../components/FilesField";
import { JobRow } from "../../components/JobsPanel";
import { useT } from "../../i18n";
import { copyText } from "../../lib/clipboard";
import { fileName } from "../../lib/format";
import type { VisionResult, VisionTask } from "../../types";
import { Button, Checkbox, EmptyState, Field, Section, Select, Tag } from "../../ui";

const TASKS: readonly VisionTask[] = ["describe", "altText", "tags", "name", "ocr"];

export function Vision() {
  const { t, td, error } = useT();
  const { ai, jobs, jobResult, navigate, notify, showError, withCloud } = useApp();
  const [files, setFiles] = useState<string[]>([]);
  const [task, setTask] = useState<VisionTask>("describe");
  const [apply, setApply] = useState(false);
  const [jobId, setJobId] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<VisionResult | null>(null);

  if (!ai) {
    return null;
  }
  if (!ai.capabilities.vision) {
    return (
      <EmptyState
        title={t("vision.noModelTitle")}
        text={t("vision.noModelText")}
        action={
          <Button variant="primary" onClick={() => navigate("ai")}>
            {t("studio.setUp")}
          </Button>
        }
      />
    );
  }

  const job = jobId === null ? undefined : jobs.find((item) => item.id === jobId);
  const canApply = task !== "ocr";

  const start = async () => {
    setBusy(true);
    setResult(null);
    try {
      const id = await withCloud((confirmed) => api.visionStart(files, task, apply && canApply, confirmed));
      setJobId(id);
      setResult(await jobResult<VisionResult>(id));
    } catch (failure) {
      showError(failure);
    } finally {
      setBusy(false);
      setJobId(null);
    }
  };

  return (
    <>
      <Section>
        <FilesField files={files} onChange={setFiles} />
        <Field label={t("vision.task")} hint={td(`vision.about.${task}`)}>
          <Select
            value={task}
            options={TASKS.map((value) => ({ value, label: td(`vision.taskKind.${value}`) }))}
            onChange={setTask}
          />
        </Field>
        {canApply ? (
          <Field label="" hint={t("vision.applyHint")}>
            <Checkbox
              checked={apply}
              onChange={setApply}
              label={task === "name" ? t("vision.applyRename") : t("vision.applyMetadata")}
            />
          </Field>
        ) : null}
        <div className="tool-run">
          <Tag tone={ai.visionRemote ? "warning" : "ok"}>
            {ai.settings.vision.model} · {ai.visionRemote ? t("ai.remote") : t("ai.local")}
          </Tag>
          <Button variant="primary" disabled={busy || files.length === 0} onClick={() => void start()}>
            {busy ? t("common.working") : t("vision.start")}
          </Button>
        </div>
        {job ? <JobRow job={job} /> : null}
      </Section>
      {result ? (
        <Section title={t("tools.result")}>
          {result.answers.map((item) => (
            <div key={item.source} className="answer">
              <img src={thumbUrl({ path: item.answer?.path ?? item.source, modified: 0 }, 256)} alt="" loading="lazy" draggable={false} />
              <div className="answer-body">
                <div className="answer-name">
                  {fileName(item.answer?.path ?? item.source)}
                  {item.answer?.applied ? <Tag tone="ok">{t("vision.applied")}</Tag> : null}
                </div>
                {item.error ? (
                  <div className="report-error">{error(item.error)}</div>
                ) : (
                  <div className="answer-text">{item.answer?.text}</div>
                )}
              </div>
              {item.answer ? (
                <Button
                  small
                  onClick={() => void copyText(item.answer?.text ?? "").then((copied) => copied && notify(t("common.copied")))}
                >
                  {t("common.copy")}
                </Button>
              ) : null}
            </div>
          ))}
        </Section>
      ) : null}
    </>
  );
}
