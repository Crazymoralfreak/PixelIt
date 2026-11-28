// Settings of the application. Every change is saved and applied at once.

import { useEffect, useState } from "react";
import { api } from "../api";
import { useApp } from "../app/context";
import { OutputEditor } from "../components/OutputEditor";
import { PathField } from "../components/PathField";
import { useT } from "../i18n";
import { formatBytes } from "../lib/format";
import { isMac, keyLabel } from "../lib/platform";
import { LANGUAGES, type Settings } from "../types";
import { Button, Checkbox, Field, NumberInput, PageFrame, Progress, Section, Segmented, Select, Tag } from "../ui";

const MODIFIER_CODES = /^(Control|Shift|Alt|Meta)(Left|Right)$/;
/** Keys that may be a global shortcut without a modifier. */
const ALONE = /^(F\d{1,2}|PrintScreen)$/;

/** `Ctrl+Shift+KeyS` as people read it: `Ctrl+Shift+S`, and `⌃⇧S` on a Mac. */
function hotkeyLabel(hotkey: string): string {
  return keyLabel(hotkey.replace(/\b(Key|Digit)(?=\w$)/, ""));
}

function HotkeyField({ value, onChange }: { value: string; onChange: (hotkey: string) => void }) {
  const { t } = useT();
  const [recording, setRecording] = useState(false);

  useEffect(() => {
    if (!recording) {
      return;
    }
    const onKey = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        setRecording(false);
        return;
      }
      if (MODIFIER_CODES.test(event.code)) {
        return;
      }
      const parts = [
        event.ctrlKey && "Ctrl",
        event.altKey && "Alt",
        event.shiftKey && "Shift",
        event.metaKey && "Super",
      ].filter((part): part is string => Boolean(part));
      if (parts.length === 0 && !ALONE.test(event.code)) {
        return;
      }
      setRecording(false);
      onChange([...parts, event.code].join("+"));
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onChange, recording]);

  return (
    <>
      <span className="hotkey">{recording ? t("settings.hotkeyPress") : value ? hotkeyLabel(value) : t("settings.hotkeyNone")}</span>
      <Button small onClick={() => setRecording(!recording)}>
        {recording ? t("common.cancel") : t("common.change")}
      </Button>
      {value && !recording ? (
        <Button small onClick={() => onChange("")}>
          {t("common.clear")}
        </Button>
      ) : null}
    </>
  );
}

export default function SettingsPage() {
  const { t, td } = useT();
  const { info, settings, saveSettings, update, showError } = useApp();
  const [cache, setCache] = useState<number | null>(null);
  const set = (patch: Partial<Settings>) => void saveSettings({ ...settings, ...patch });

  useEffect(() => {
    api
      .cacheInfo()
      .then((answer) => setCache(answer.bytes))
      .catch(() => undefined);
  }, []);

  const busyUpdate =
    update.state === "checking" ||
    update.state === "downloading" ||
    update.state === "ready" ||
    update.state === "installing";
  // A download or an installer that failed leaves the version known, so it can be asked for again.
  const installFailed = update.state === "failed" && update.version !== null;
  const canInstall = !update.portable && (update.state === "available" || installFailed);
  // A closed group still tells what matters: the language in use, and an update that is waiting or on its way.
  const languageName =
    LANGUAGES.find((language) => language.code === settings.language)?.name ?? t("settings.languageSystem");
  const updatePending =
    update.state === "available" || update.state === "downloading" || update.state === "ready" || update.state === "installing";

  return (
    <PageFrame title={t("nav.settings")}>
      <Section title={t("settings.interface")} summary={<span>{languageName}</span>} collapsible defaultOpen={false}>
        <Field label={t("settings.language")}>
          <Select
            value={settings.language}
            options={[
              { value: "", label: t("settings.languageSystem") },
              ...LANGUAGES.map((language) => ({ value: language.code as string, label: language.name })),
            ]}
            onChange={(language) => set({ language })}
          />
        </Field>
        <Field label={t("settings.theme")}>
          <Segmented
            value={settings.theme}
            options={[
              { value: "system", label: t("settings.themeSystem") },
              { value: "light", label: t("settings.themeLight") },
              { value: "dark", label: t("settings.themeDark") },
            ]}
            onChange={(theme) => set({ theme })}
          />
        </Field>
      </Section>

      <Section title={t("settings.performance")} collapsible defaultOpen={false}>
        <Field label={t("settings.threads")} hint={t("settings.threadsHint")}>
          <NumberInput value={settings.threads} onChange={(threads) => set({ threads })} min={0} max={256} />
        </Field>
        <Field label={t("settings.memory")} hint={t("settings.memoryHint")}>
          <NumberInput
            value={settings.memoryLimitMb}
            onChange={(memoryLimitMb) => set({ memoryLimitMb })}
            min={256}
            max={262144}
            step={256}
            suffix="MB"
            wide
          />
        </Field>
        <Field label={t("settings.thumbCache")} hint={cache === null ? undefined : t("settings.thumbCacheUsed", { size: formatBytes(cache) })}>
          <NumberInput
            value={settings.thumbCacheMb}
            onChange={(thumbCacheMb) => set({ thumbCacheMb })}
            min={16}
            max={65536}
            step={64}
            suffix="MB"
            wide
          />
          <Button
            small
            onClick={() =>
              api
                .cacheClear()
                .then((answer) => setCache(answer.bytes))
                .catch(showError)
            }
          >
            {t("common.clear")}
          </Button>
        </Field>
      </Section>

      <Section title={t("settings.files")} collapsible defaultOpen={false}>
        <Field label="">
          <div className="stack">
            <Checkbox
              checked={settings.confirmOverwrite}
              onChange={(confirmOverwrite) => set({ confirmOverwrite })}
              label={t("settings.confirmOverwrite")}
            />
            <Checkbox
              checked={settings.confirmDelete}
              onChange={(confirmDelete) => set({ confirmDelete })}
              label={t("settings.confirmDelete")}
            />
          </div>
        </Field>
        <Field label={t("settings.downloadFolder")} hint={t("settings.downloadFolderHint")}>
          <PathField
            kind="folder"
            value={settings.downloadFolder}
            onChange={(downloadFolder) => set({ downloadFolder })}
          />
        </Field>
        <Field label={t("settings.pdfium")} hint={t("settings.pdfiumHint")}>
          <PathField kind="folder" value={settings.pdfiumFolder} onChange={(pdfiumFolder) => set({ pdfiumFolder })} />
        </Field>
      </Section>

      <Section title={t("settings.defaultOutput")} collapsible defaultOpen={false}>
        <OutputEditor value={settings.output} onChange={(output) => set({ output })} />
      </Section>

      <Section title={isMac() ? "macOS" : t("settings.system")} collapsible defaultOpen={false}>
        <Field label="">
          <div className="stack">
            <Checkbox
              checked={settings.closeToTray}
              onChange={(closeToTray) => set({ closeToTray })}
              label={isMac() ? t("settings.closeToTrayMac") : t("settings.closeToTray")}
            />
            <Checkbox
              checked={settings.autostart}
              onChange={(autostart) => set({ autostart })}
              label={isMac() ? t("settings.autostartMac") : t("settings.autostart")}
            />
            <Checkbox
              checked={settings.startMinimized}
              onChange={(startMinimized) => set({ startMinimized })}
              label={t("settings.startMinimized")}
            />
            {/* The menu of Explorer exists in Windows only; on a Mac files open through their associations. */}
            {info.platform === "windows" ? (
              <Checkbox
                checked={settings.contextMenu}
                onChange={(contextMenu) => set({ contextMenu })}
                label={t("settings.contextMenu")}
              />
            ) : null}
          </div>
        </Field>
        <Field label={t("settings.captureHotkey")} hint={t("settings.captureHotkeyHint")}>
          <HotkeyField value={settings.captureHotkey} onChange={(captureHotkey) => set({ captureHotkey })} />
        </Field>
      </Section>

      <Section
        title={t("settings.updates")}
        summary={
          updatePending ? (
            <Tag tone="ok">{td(`update.state.${update.state}`, { version: update.version ?? "" })}</Tag>
          ) : (
            <span>{info.version}</span>
          )
        }
        collapsible
        defaultOpen={false}
      >
        <Field label="">
          <Checkbox
            checked={settings.checkUpdates}
            onChange={(checkUpdates) => set({ checkUpdates })}
            label={t("settings.checkUpdates")}
          />
        </Field>
        <Field label={t("settings.channel")} hint={t("settings.channelHint")}>
          <Segmented
            value={settings.updateChannel}
            options={[
              { value: "stable", label: t("settings.channelStable") },
              { value: "beta", label: t("settings.channelBeta") },
            ]}
            onChange={(updateChannel) => set({ updateChannel })}
          />
        </Field>
        <Field
          label={t("settings.version", { version: info.version })}
          hint={
            !update.portable
              ? t("settings.updateAsk")
              : isMac()
                ? t("settings.updatePortableMac")
                : t("settings.updatePortable")
          }
        >
          <span>
            {installFailed
              ? t("update.installFailed", { version: update.version ?? "" })
              : td(`update.state.${update.state}`, { version: update.version ?? "" })}
          </span>
          {canInstall ? (
            <Button small variant="primary" onClick={() => api.updateInstall().catch(showError)}>
              {t("update.install")}
            </Button>
          ) : null}
          <Button small disabled={busyUpdate} onClick={() => api.updateCheck().catch(showError)}>
            {t("settings.checkNow")}
          </Button>
        </Field>
        {update.state === "downloading" ? <Progress done={update.downloaded} total={update.total} /> : null}
      </Section>

      <Section title={t("settings.data")} collapsible defaultOpen={false}>
        <Field label={t("settings.dataFolder")} hint={info.portable ? t("settings.portable") : undefined}>
          <span className="path-text">{info.dataDir}</span>
          <Button small onClick={() => api.revealPath(info.dataDir).catch(showError)}>
            {t("batch.openFolder")}
          </Button>
        </Field>
        <Field label={t("settings.logs")}>
          <Button small onClick={() => api.revealLogs().catch(showError)}>
            {t("batch.openFolder")}
          </Button>
        </Field>
      </Section>
    </PageFrame>
  );
}
