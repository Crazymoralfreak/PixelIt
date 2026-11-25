// The styles switched on under the prompt, and the window that keeps them.

import { useEffect, useState } from "react";
import { api } from "../../api";
import { useApp } from "../../app/context";
import { useT } from "../../i18n";
import type { Style } from "../../types";
import { Button, Checkbox, Field, Modal, TextArea, TextInput } from "../../ui";

function styleName(style: Style, td: (key: string) => string): string {
  return style.builtin ? td(`studio.builtin.${style.id}`) : style.name;
}

export function StyleBar({
  selected,
  styles,
  showFinal,
  finalText,
  onToggle,
  onShowFinal,
  onChanged,
}: {
  selected: string[];
  styles: Style[];
  showFinal: boolean;
  finalText: string;
  onToggle: (id: string) => void;
  onShowFinal: (show: boolean) => void;
  onChanged: () => void;
}) {
  const { t, td } = useT();
  const [open, setOpen] = useState(false);
  const chosen = styles.filter((style) => selected.includes(style.id));

  return (
    <div className="studio-styles">
      <div className="studio-style-row">
        {chosen.map((style) => (
          <Button key={style.id} small variant="primary" onClick={() => onToggle(style.id)}>
            {styleName(style, td)}
          </Button>
        ))}
        <Button small onClick={() => setOpen(true)}>
          {t("studio.stylesOpen")}
        </Button>
        <Checkbox checked={showFinal} onChange={onShowFinal} label={t("studio.showFinal")} />
      </div>
      {showFinal ? <p className="tool-text">{finalText || t("studio.finalEmpty")}</p> : null}
      {open ? (
        <StyleDialog
          styles={styles}
          selected={selected}
          onToggle={onToggle}
          onClose={() => setOpen(false)}
          onChanged={onChanged}
        />
      ) : null}
    </div>
  );
}

function StyleDialog({
  styles,
  selected,
  onToggle,
  onClose,
  onChanged,
}: {
  styles: Style[];
  selected: string[];
  onToggle: (id: string) => void;
  onClose: () => void;
  onChanged: () => void;
}) {
  const { t, td } = useT();
  const { showError, notify } = useApp();
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState<Style | null>(null);
  const needle = query.trim().toLowerCase();
  const shown = styles.filter((style) => styleName(style, td).toLowerCase().includes(needle));

  const save = async () => {
    if (!draft) {
      return;
    }
    try {
      await api.styleSave(draft);
      onChanged();
      setDraft(null);
    } catch (error) {
      showError(error);
    }
  };

  const remove = async (style: Style) => {
    try {
      await api.styleRemove(style.id);
      onChanged();
    } catch (error) {
      showError(error);
    }
  };

  const duplicate = (style: Style) => {
    setDraft({ ...style, id: "", name: `${styleName(style, td)} 2`, builtin: false });
  };

  const importFile = async () => {
    const path = await api.pickFile(t("studio.styleFile"), ["json"]);
    if (!path) {
      return;
    }
    try {
      const added = await api.stylesImport(path);
      onChanged();
      notify(t("studio.stylesImported", { count: added.length }));
    } catch (error) {
      showError(error);
    }
  };

  const exportFile = async () => {
    const path = await api.pickSave("styles.json", t("studio.styleFile"), ["json"]);
    if (!path) {
      return;
    }
    try {
      await api.stylesExport(path, selected.length > 0 ? selected : styles.map((style) => style.id));
      notify(t("studio.stylesExported"));
    } catch (error) {
      showError(error);
    }
  };

  return (
    <Modal
      title={t("studio.stylesTitle")}
      onClose={onClose}
      wide
      footer={
        <>
          <Button onClick={() => void importFile()}>{t("studio.styleImport")}</Button>
          <Button onClick={() => void exportFile()}>{t("studio.styleExport")}</Button>
          <Button variant="primary" onClick={() => setDraft({ id: "", name: "", prompt: "", negative: "", builtin: false })}>
            {t("studio.styleNew")}
          </Button>
        </>
      }
    >
      <Field label={t("studio.styleSearch")}>
        <TextInput value={query} onChange={setQuery} wide />
      </Field>
      <div className="studio-style-list">
        {shown.map((style) => (
          <div key={style.id} className="studio-style-item">
            <Checkbox checked={selected.includes(style.id)} onChange={() => onToggle(style.id)} label={styleName(style, td)} />
            {style.builtin ? null : (
              <Button small onClick={() => setDraft(style)}>
                {t("studio.styleEdit")}
              </Button>
            )}
            <Button small onClick={() => duplicate(style)}>
              {t("studio.styleDuplicate")}
            </Button>
            {style.builtin ? null : (
              <Button small onClick={() => void remove(style)}>
                {t("studio.styleDelete")}
              </Button>
            )}
          </div>
        ))}
      </div>
      {draft ? (
        <div className="studio-style-edit">
          <Field label={t("studio.styleName")}>
            <TextInput value={draft.name} onChange={(name) => setDraft({ ...draft, name })} wide />
          </Field>
          <Field label={t("studio.stylePrompt")} hint={t("studio.stylePromptHint")}>
            <TextArea value={draft.prompt} onChange={(prompt) => setDraft({ ...draft, prompt })} rows={3} />
          </Field>
          <Field label={t("studio.styleNegative")}>
            <TextInput value={draft.negative} onChange={(negative) => setDraft({ ...draft, negative })} wide />
          </Field>
          <Button variant="primary" disabled={draft.name.trim() === ""} onClick={() => void save()}>
            {t("common.save")}
          </Button>
        </div>
      ) : null}
    </Modal>
  );
}

export function useStyles(): { styles: Style[]; reload: () => void } {
  const [styles, setStyles] = useState<Style[]>([]);
  const reload = () => {
    api
      .stylesList()
      .then(setStyles)
      .catch(() => undefined);
  };
  useEffect(() => {
    reload();
  }, []);
  return { styles, reload };
}
