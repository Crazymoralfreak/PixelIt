// Everything about models: what is installed on this computer, which
// servers PixelIt talks to, and MCP.

import { useEffect, useState } from "react";
import { api } from "../api";
import { useApp } from "../app/context";
import { JobRow } from "../components/JobsPanel";
import { PathField } from "../components/PathField";
import { useT } from "../i18n";
import { formatBytes } from "../lib/format";
import type {
  AiSettings,
  AiStatus,
  ComponentKind,
  ComponentStatus,
  Endpoint,
  FoundServer,
  Generation,
  GenerationProvider,
} from "../types";
import {
  Button,
  Checkbox,
  Field,
  NumberInput,
  PageFrame,
  Section,
  Segmented,
  Select,
  Slider,
  Tabs,
  Tag,
  TextInput,
} from "../ui";
import { McpTab } from "./ai/Mcp";

const KINDS: readonly ComponentKind[] = [
  "runtime",
  "background",
  "upscale",
  "inpaint",
  "faces",
  "detector",
  "generator",
  "pdf",
];
/** Tasks the one-click setup covers with one model each. */
const STARTER_KINDS: readonly ComponentKind[] = ["background", "upscale", "inpaint", "faces"];
const PROVIDERS: readonly GenerationProvider[] = ["none", "comfy", "forge", "sdCpp", "cloud"];
const SERVER_NAMES: Record<FoundServer["kind"], string> = {
  ollama: "Ollama",
  lmStudio: "LM Studio",
  llamaCpp: "llama.cpp",
  comfy: "ComfyUI",
  forge: "Forge / A1111",
};

function hostOf(url: string): string {
  try {
    return new URL(url).hostname.toLowerCase();
  } catch {
    return "";
  }
}

/** The key of a server, kept in the Windows Credential Manager and never shown again. */
function KeyField({ url }: { url: string }) {
  const { t } = useT();
  const { notify, showError } = useApp();
  const host = hostOf(url);
  const [stored, setStored] = useState(false);
  const [key, setKey] = useState("");

  useEffect(() => {
    setKey("");
    if (host === "") {
      setStored(false);
      return;
    }
    let alive = true;
    api
      .aiKeyHas(host)
      .then((has) => alive && setStored(has))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [host]);

  if (host === "") {
    return null;
  }
  return (
    <Field label={t("ai.key")} hint={t("ai.keyHint", { host })}>
      {stored ? (
        <>
          <Tag tone="ok">{t("ai.keyStored")}</Tag>
          <Button
            small
            onClick={() =>
              api
                .aiKeyDelete(host)
                .then(() => setStored(false))
                .catch(showError)
            }
          >
            {t("common.delete")}
          </Button>
        </>
      ) : (
        <>
          <TextInput value={key} onChange={setKey} type="password" autoComplete="off" />
          <Button
            small
            disabled={key.trim() === ""}
            onClick={() =>
              api
                .aiKeySet(host, key.trim())
                .then(() => {
                  setStored(true);
                  setKey("");
                  notify(t("ai.keySaved"));
                })
                .catch(showError)
            }
          >
            {t("common.save")}
          </Button>
        </>
      )}
    </Field>
  );
}

function EndpointFields({ value, onChange }: { value: Endpoint; onChange: (value: Endpoint) => void }) {
  const { t } = useT();
  const { showError } = useApp();
  const [models, setModels] = useState<string[]>([]);

  return (
    <>
      <Field label={t("ai.url")} hint={t("ai.urlHint")}>
        <TextInput
          value={value.url}
          onChange={(url) => onChange({ ...value, url })}
          wide
          placeholder="http://127.0.0.1:11434/v1"
        />
      </Field>
      <Field label={t("op.model")}>
        <TextInput value={value.model} onChange={(model) => onChange({ ...value, model })} />
        <Button small disabled={value.url.trim() === ""} onClick={() => api.aiModels(value.url).then(setModels).catch(showError)}>
          {t("ai.listModels")}
        </Button>
        {models.length > 0 ? (
          <Select
            value={models.includes(value.model) ? value.model : ""}
            options={[{ value: "", label: t("ai.chooseModel") }, ...models.map((model) => ({ value: model, label: model }))]}
            onChange={(model) => model !== "" && onChange({ ...value, model })}
          />
        ) : null}
      </Field>
      <KeyField url={value.url} />
    </>
  );
}

function ComponentRow({ component }: { component: ComponentStatus }) {
  const { t } = useT();
  const { jobs, refreshAi, confirm, showError } = useApp();
  const job = jobs.find(
    (item) =>
      item.kind === "install" &&
      item.title === component.id &&
      (item.state === "queued" || item.state === "running" || item.state === "paused"),
  );

  const remove = async () => {
    const agreed = await confirm({
      title: t("ai.removeTitle"),
      text: t("ai.removeText", { name: component.name }),
      confirmLabel: t("common.delete"),
      danger: true,
    });
    if (agreed) {
      api.componentRemove(component.id).then(refreshAi).catch(showError);
    }
  };

  return (
    <div className="component">
      <div className="component-main">
        <div className="component-name">
          {component.name}
          {component.installed ? <Tag tone="ok">{t("ai.installed")}</Tag> : null}
        </div>
        <div className="component-facts">
          {formatBytes(component.bytes)} · {component.license} · {hostOf(component.source) || component.source}
        </div>
        {job ? <JobRow job={job} /> : null}
      </div>
      {job ? null : component.installed ? (
        <Button small onClick={() => void remove()}>
          {t("common.delete")}
        </Button>
      ) : (
        <Button small variant="primary" onClick={() => api.componentInstall(component.id).catch(showError)}>
          {t("ai.install")}
        </Button>
      )}
    </div>
  );
}

function ComponentsTab({ ai, onSettings }: { ai: AiStatus; onSettings: (settings: AiSettings) => void }) {
  const { t, td } = useT();
  const { jobs, confirm, showError } = useApp();
  const { hardware, advice, settings, components } = ai;

  // One model for every task that has none yet.
  const starter = STARTER_KINDS.flatMap((kind) => {
    const ofKind = components.filter((component) => component.kind === kind);
    return ofKind.some((component) => component.installed) ? [] : ofKind.slice(0, 1);
  });
  const needed = new Set(starter.flatMap((component) => [component.id, ...component.requires]));
  const starterBytes = components
    .filter((component) => needed.has(component.id) && !component.installed)
    .reduce((sum, component) => sum + component.bytes, 0);

  const installStarter = async () => {
    const agreed = await confirm({
      title: t("ai.starterTitle"),
      text: t("ai.starterText", {
        names: starter.map((component) => component.name).join(", "),
        size: formatBytes(starterBytes),
      }),
      confirmLabel: t("ai.install"),
    });
    if (agreed) {
      for (const component of starter) {
        await api.componentInstall(component.id).catch(showError);
      }
    }
  };

  const installedOf = (kind: ComponentKind) =>
    components.filter((component) => component.kind === kind && component.installed);
  const modelSelect = (kind: "background" | "upscale" | "inpaint" | "faces") => (
    <Field key={kind} label={td(`ai.kind.${kind}`)}>
      <Select
        value={settings.models[kind]}
        options={[
          { value: "", label: t("op.modelDefault") },
          ...installedOf(kind).map((component) => ({ value: component.id, label: component.name })),
        ]}
        onChange={(model) => onSettings({ ...settings, models: { ...settings.models, [kind]: model } })}
      />
    </Field>
  );

  const deviceName =
    settings.device === "gpu" ? t("ai.deviceGpu") : settings.device === "cpu" ? t("ai.deviceCpu") : t("ai.deviceAuto");
  /** Components that are being downloaded right now. */
  const installing = new Set(
    jobs
      .filter((job) => job.kind === "install" && (job.state === "queued" || job.state === "running" || job.state === "paused"))
      .map((job) => job.title),
  );

  return (
    <>
      <Section title={t("ai.hardware")} collapsible>
        <div className="report-line">
          {t("ai.hardwareText", { ram: formatBytes(hardware.ramMb * 1024 * 1024), threads: hardware.threads })}
        </div>
        {hardware.gpus.map((gpu) => (
          <div key={gpu.index} className="report-line">
            {gpu.name} · {formatBytes(gpu.memoryMb * 1024 * 1024)}
          </div>
        ))}
        <div className="report-line">{advice.useGpu ? t("ai.adviceGpu") : t("ai.adviceCpu")}</div>
        {starter.length > 0 ? (
          <div className="tool-run">
            <span>{t("ai.starterHint")}</span>
            <Button variant="primary" onClick={() => void installStarter()}>
              {t("ai.starter", { size: formatBytes(starterBytes) })}
            </Button>
          </div>
        ) : null}
      </Section>

      <Section title={t("ai.device")} summary={<span>{deviceName}</span>} collapsible defaultOpen={false}>
        <Field label={t("ai.runOn")}>
          <Segmented
            value={settings.device}
            options={[
              { value: "auto", label: t("ai.deviceAuto") },
              { value: "gpu", label: t("ai.deviceGpu") },
              { value: "cpu", label: t("ai.deviceCpu") },
            ]}
            onChange={(device) => onSettings({ ...settings, device })}
          />
        </Field>
        {hardware.gpus.length > 1 && settings.device !== "cpu" ? (
          <Field label={t("ai.gpu")}>
            <Select
              value={String(settings.gpu)}
              options={[
                { value: "-1", label: t("ai.deviceAuto") },
                ...hardware.gpus.map((gpu) => ({ value: String(gpu.index), label: gpu.name })),
              ]}
              onChange={(gpu) => onSettings({ ...settings, gpu: Number(gpu) })}
            />
          </Field>
        ) : null}
        <Field label={t("settings.threads")} hint={t("settings.threadsHint")}>
          <NumberInput value={settings.threads} onChange={(threads) => onSettings({ ...settings, threads })} min={0} max={256} />
        </Field>
        {(["background", "upscale", "inpaint", "faces"] as const)
          .filter((kind) => installedOf(kind).length > 1)
          .map(modelSelect)}
      </Section>

      {KINDS.map((kind) => {
        const ofKind = components.filter((component) => component.kind === kind);
        if (ofKind.length === 0) {
          return null;
        }
        const ready = ofKind.filter((component) => component.installed).length;
        return (
          <Section
            key={kind}
            title={td(`ai.kind.${kind}`)}
            summary={
              <>
                {ofKind.some((component) => installing.has(component.id)) ? (
                  <Tag tone="warning">{t("common.working")}</Tag>
                ) : ready > 0 ? (
                  <Tag tone="ok">{t("ai.installed")}</Tag>
                ) : null}
                <span>
                  {ready} / {ofKind.length}
                </span>
              </>
            }
            collapsible
            defaultOpen={false}
          >
            <p className="tool-text">{td(`ai.kindText.${kind}`)}</p>
            {ofKind.map((component) => (
              <ComponentRow key={component.id} component={component} />
            ))}
          </Section>
        );
      })}
    </>
  );
}

function ProvidersTab({ ai, onSettings }: { ai: AiStatus; onSettings: (settings: AiSettings) => void }) {
  const { t, td } = useT();
  const { notify, showError } = useApp();
  const [draft, setDraft] = useState(ai.settings);
  const [found, setFound] = useState<FoundServer[] | null>(null);
  const [searching, setSearching] = useState(false);
  const saved = JSON.stringify(ai.settings);
  const changed = JSON.stringify(draft) !== saved;

  // Settings changed elsewhere (another tab, an assistant tool) replace the draft.
  useEffect(() => setDraft(JSON.parse(saved) as AiSettings), [saved]);

  const generation = draft.generation;
  const setGeneration = (patch: Partial<Generation>) => setDraft({ ...draft, generation: { ...generation, ...patch } });

  const discover = () => {
    setSearching(true);
    api
      .aiDiscover()
      .then((list) => {
        setFound(list);
        if (list.length === 0) {
          notify(t("ai.noneFound"));
        }
      })
      .catch(showError)
      .finally(() => setSearching(false));
  };

  const assignServer = (server: FoundServer, role: "vision" | "assistant") =>
    setDraft({ ...draft, [role]: { url: server.url, model: server.models[0] ?? "" } });

  return (
    <>
      <Section title={t("ai.privacy")} collapsible defaultOpen={false}>
        <p className="tool-text">{t("ai.privacyText")}</p>
        <Field label="">
          <div className="stack">
            <Checkbox
              checked={draft.allowCloud}
              onChange={(allowCloud) => setDraft({ ...draft, allowCloud })}
              label={t("ai.allowCloud")}
            />
            <Checkbox
              checked={draft.confirmCloud}
              onChange={(confirmCloud) => setDraft({ ...draft, confirmCloud })}
              label={t("ai.confirmCloud")}
              disabled={!draft.allowCloud}
            />
          </div>
        </Field>
      </Section>

      <Section
        title={t("ai.localServers")}
        collapsible
        defaultOpen={false}
        actions={
          <Button small disabled={searching} onClick={discover}>
            {searching ? t("common.working") : t("ai.discover")}
          </Button>
        }
      >
        <p className="tool-text">{t("ai.localServersText")}</p>
        {(found ?? []).map((server) => (
          <div key={server.url} className="component">
            <div className="component-main">
              <div className="component-name">{SERVER_NAMES[server.kind]}</div>
              <div className="component-facts">
                {server.url} · {t("ai.modelsCount", { count: server.models.length })}
              </div>
            </div>
            {server.kind === "comfy" ? (
              <Button small onClick={() => setGeneration({ provider: "comfy", comfyUrl: server.url })}>
                {t("ai.useForGeneration")}
              </Button>
            ) : server.kind === "forge" ? (
              <Button small onClick={() => setGeneration({ provider: "forge", forgeUrl: server.url })}>
                {t("ai.useForGeneration")}
              </Button>
            ) : (
              <>
                <Button small onClick={() => assignServer(server, "vision")}>
                  {t("ai.useForVision")}
                </Button>
                <Button small onClick={() => assignServer(server, "assistant")}>
                  {t("ai.useForAssistant")}
                </Button>
              </>
            )}
          </div>
        ))}
      </Section>

      <Section
        title={t("ai.vision")}
        summary={draft.vision.model ? <span>{draft.vision.model}</span> : null}
        collapsible
        defaultOpen={false}
      >
        <p className="tool-text">{t("ai.visionText")}</p>
        <EndpointFields value={draft.vision} onChange={(vision) => setDraft({ ...draft, vision })} />
      </Section>

      <Section
        title={t("ai.assistant")}
        summary={draft.assistant.model ? <span>{draft.assistant.model}</span> : null}
        collapsible
        defaultOpen={false}
      >
        <p className="tool-text">{t("ai.assistantText")}</p>
        <EndpointFields value={draft.assistant} onChange={(assistant) => setDraft({ ...draft, assistant })} />
      </Section>

      <Section
        title={t("ai.generation")}
        summary={<span>{td(`ai.provider.${generation.provider}`)}</span>}
        collapsible
        defaultOpen={false}
      >
        <Field label={t("ai.providerLabel")} hint={td(`ai.providerText.${generation.provider}`)}>
          <Select
            value={generation.provider}
            options={PROVIDERS.map((provider) => ({ value: provider, label: td(`ai.provider.${provider}`) }))}
            onChange={(provider) => setGeneration({ provider })}
          />
        </Field>
        {generation.provider === "comfy" ? (
          <>
            <Field label={t("ai.url")}>
              <TextInput value={generation.comfyUrl} onChange={(comfyUrl) => setGeneration({ comfyUrl })} wide />
            </Field>
            <Field label={t("ai.checkpoint")} hint={t("ai.checkpointHint")}>
              <TextInput
                value={generation.comfyCheckpoint}
                onChange={(comfyCheckpoint) => setGeneration({ comfyCheckpoint })}
                wide
              />
            </Field>
            <Field label={t("ai.workflow")} hint={t("ai.workflowHint")}>
              <PathField
                kind="file"
                filter={{ name: "JSON", extensions: ["json"] }}
                value={generation.comfyWorkflow}
                onChange={(comfyWorkflow) => setGeneration({ comfyWorkflow })}
              />
            </Field>
          </>
        ) : null}
        {generation.provider === "forge" ? (
          <Field label={t("ai.url")}>
            <TextInput value={generation.forgeUrl} onChange={(forgeUrl) => setGeneration({ forgeUrl })} wide />
          </Field>
        ) : null}
        {generation.provider === "sdCpp" ? (
          <>
            <Field label={t("ai.checkpoint")} hint={t("ai.sdcppModelHint")}>
              <PathField
                kind="file"
                filter={{ name: t("ai.checkpoint"), extensions: ["safetensors", "gguf", "ckpt"] }}
                value={generation.sdcppModel}
                onChange={(sdcppModel) => setGeneration({ sdcppModel })}
              />
            </Field>
            <Field label={t("ai.sdcppArguments")}>
              <TextInput
                value={generation.sdcppArguments}
                onChange={(sdcppArguments) => setGeneration({ sdcppArguments })}
                wide
              />
            </Field>
          </>
        ) : null}
        {generation.provider === "cloud" ? (
          <EndpointFields value={generation.cloud} onChange={(cloud) => setGeneration({ cloud })} />
        ) : null}
        {generation.provider === "none" ? null : (
          <>
            <Field label={t("op.size")}>
              <NumberInput value={generation.width} onChange={(width) => setGeneration({ width })} min={64} max={4096} step={8} suffix="×" wide />
              <NumberInput value={generation.height} onChange={(height) => setGeneration({ height })} min={64} max={4096} step={8} suffix="px" wide />
            </Field>
            <Field label={t("studio.steps")}>
              <Slider value={generation.steps} onChange={(steps) => setGeneration({ steps })} min={1} max={150} reset={24} />
            </Field>
            <Field label={t("studio.cfg")}>
              <Slider value={generation.cfg} onChange={(cfg) => setGeneration({ cfg })} min={0} max={30} step={0.5} reset={6} />
            </Field>
            <Field label={t("studio.negative")}>
              <TextInput value={generation.negative} onChange={(negative) => setGeneration({ negative })} wide />
            </Field>
          </>
        )}
      </Section>

      <div className="save-bar">
        <Button disabled={!changed} onClick={() => setDraft(ai.settings)}>
          {t("common.reset")}
        </Button>
        <Button variant="primary" disabled={!changed} onClick={() => onSettings(draft)}>
          {t("common.save")}
        </Button>
      </div>
    </>
  );
}

export default function AiPage({ active }: { active: boolean }) {
  const { t } = useT();
  const { ai, refreshAi, showError } = useApp();
  const [tab, setTab] = useState<"components" | "providers" | "mcp">("components");

  useEffect(() => {
    if (active) {
      refreshAi();
    }
  }, [active, refreshAi]);

  const saveSettings = (settings: AiSettings) => {
    api.aiSettingsSet(settings).then(refreshAi).catch(showError);
  };

  return (
    <PageFrame title={t("nav.ai")}>
      <Tabs
        value={tab}
        options={[
          { value: "components", label: t("ai.tabComponents") },
          { value: "providers", label: t("ai.tabProviders") },
          { value: "mcp", label: "MCP" },
        ]}
        onChange={setTab}
      />
      {ai === null ? null : (
        <>
          <div className="column" hidden={tab !== "components"}>
            <ComponentsTab ai={ai} onSettings={saveSettings} />
          </div>
          <div className="column" hidden={tab !== "providers"}>
            <ProvidersTab ai={ai} onSettings={saveSettings} />
          </div>
        </>
      )}
      <div className="column" hidden={tab !== "mcp"}>
        <McpTab />
      </div>
    </PageFrame>
  );
}
