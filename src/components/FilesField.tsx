// The pictures a tool works on: chosen in a dialog or taken from the batch queue.

import { api } from "../api";
import { useApp } from "../app/context";
import { useT } from "../i18n";
import { fileName } from "../lib/format";
import { Button, Field } from "../ui";

/** Names shown under the field before "…". */
const NAMES_SHOWN = 6;

export function FilesField({ files, onChange }: { files: string[]; onChange: (files: string[]) => void }) {
  const { t } = useT();
  const { queue, showError } = useApp();
  const names = files.slice(0, NAMES_SHOWN).map(fileName).join(", ");

  return (
    <Field label={t("tools.pictures")} hint={files.length > NAMES_SHOWN ? `${names}…` : names}>
      <span>{t("batch.pictures", { count: files.length })}</span>
      <Button
        small
        onClick={() =>
          api
            .pickFiles(true)
            .then((chosen) => chosen.length > 0 && onChange(chosen))
            .catch(showError)
        }
      >
        {t("viewer.openFiles")}
      </Button>
      <Button
        small
        disabled={queue.length === 0}
        onClick={() =>
          api
            .filesExpand(queue, true)
            .then((list) => onChange(list.map((file) => file.path)))
            .catch(showError)
        }
      >
        {t("tools.fromQueue")}
      </Button>
    </Field>
  );
}
