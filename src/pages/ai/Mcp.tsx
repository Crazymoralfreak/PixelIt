// MCP in both directions: PixelIt as a server for other programs, and the
// servers whose tools the assistant of PixelIt may call.

import { useCallback, useEffect, useState } from "react";
import { api } from "../../api";
import { useAppEvent } from "../../api/events";
import { useApp } from "../../app/context";
import { PathField } from "../../components/PathField";
import { useT } from "../../i18n";
import { copyText } from "../../lib/clipboard";
import { isMac } from "../../lib/platform";
import type { Permission, ServerConfig, ServerStatus } from "../../types";
import { Button, Checkbox, EmptyState, Field, Modal, Section, Segmented, Select, Tag, TextArea, TextInput } from "../../ui";

const PERMISSIONS: readonly Permission[] = ["allow", "ask", "deny"];

const BLANK: ServerConfig = {
  id: "",
  name: "",
  enabled: true,
  transport: "stdio",
  command: "",
  args: [],
  env: {},
  url: "",
  defaultPermission: "ask",
  tools: {},
};

function lines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

function ServerDialog({
  server,
  onSave,
  onClose,
}: {
  server: ServerConfig;
  onSave: (server: ServerConfig) => void;
  onClose: () => void;
}) {
  const { t, td } = useT();
  const [draft, setDraft] = useState(server);
  const [args, setArgs] = useState(server.args.join("\n"));
  const [env, setEnv] = useState(
    Object.entries(server.env)
      .map(([key, value]) => `${key}=${value}`)
      .join("\n"),
  );
  const complete =
    draft.name.trim() !== "" && (draft.transport === "stdio" ? draft.command.trim() !== "" : draft.url.trim() !== "");

  const save = () => {
    const variables: Record<string, string> = {};
    for (const line of lines(env)) {
      const cut = line.indexOf("=");
      if (cut > 0) {
        variables[line.slice(0, cut).trim()] = line.slice(cut + 1);
      }
    }
    onSave({ ...draft, args: lines(args), env: variables });
  };

  return (
    <Modal
      title={server.id === "" ? t("mcp.newTitle") : t("mcp.editTitle")}
      onClose={onClose}
      wide
      footer={
        <>
          <Button onClick={onClose}>{t("common.cancel")}</Button>
          <Button variant="primary" disabled={!complete} onClick={save}>
            {t("common.save")}
          </Button>
        </>
      }
    >
      <Field label={t("recipes.name")}>
        <TextInput value={draft.name} onChange={(name) => setDraft({ ...draft, name })} wide autoFocus />
      </Field>
      <Field label={t("mcp.transport")}>
        <Segmented
          value={draft.transport}
          options={[
            { value: "stdio", label: t("mcp.stdio") },
            { value: "http", label: "HTTP" },
          ]}
          onChange={(transport) => setDraft({ ...draft, transport })}
        />
      </Field>
      {draft.transport === "stdio" ? (
        <>
          <Field label={t("mcp.command")} hint={t("mcp.commandHint")}>
            <TextInput value={draft.command} onChange={(command) => setDraft({ ...draft, command })} wide />
          </Field>
          <Field label={t("mcp.args")} hint={t("mcp.argsHint")}>
            <TextArea value={args} onChange={setArgs} rows={3} />
          </Field>
          <Field label={t("mcp.env")} hint={isMac() ? t("mcp.envHintMac") : t("mcp.envHint")}>
            <TextArea value={env} onChange={setEnv} rows={3} placeholder="API_KEY=..." />
          </Field>
        </>
      ) : (
        <Field label={t("ai.url")} hint={t("mcp.urlHint")}>
          <TextInput
            value={draft.url}
            onChange={(url) => setDraft({ ...draft, url })}
            wide
            placeholder="http://127.0.0.1:3000/mcp"
          />
        </Field>
      )}
      <Field label={t("mcp.defaultPermission")} hint={t("mcp.defaultPermissionHint")}>
        <Select
          value={draft.defaultPermission}
          options={PERMISSIONS.map((value) => ({ value, label: td(`mcp.permission.${value}`) }))}
          onChange={(defaultPermission) => setDraft({ ...draft, defaultPermission })}
        />
      </Field>
      <Field label="">
        <Checkbox
          checked={draft.enabled}
          onChange={(enabled) => setDraft({ ...draft, enabled })}
          label={t("mcp.enabled")}
        />
      </Field>
    </Modal>
  );
}

function AgentAccess() {
  const { t } = useT();
  const { settings, saveSettings } = useApp();
  const agents = settings.agents;
  const set = (patch: Partial<typeof agents>) => void saveSettings({ ...settings, agents: { ...agents, ...patch } });
  const [folder, setFolder] = useState("");

  return (
    <Section title={t("mcp.accessTitle")} collapsible defaultOpen={false}>
      <p className="tool-text">{t("mcp.accessText")}</p>
      <Field label={t("mcp.folders")} hint={agents.folders.length === 0 ? t("mcp.foldersNone") : undefined}>
        <div className="stack">
          {agents.folders.map((item) => (
            <span key={item} className="stack-row">
              <span title={item}>{item}</span>
              <Button small variant="ghost" onClick={() => set({ folders: agents.folders.filter((known) => known !== item) })}>
                ✕
              </Button>
            </span>
          ))}
          <span className="stack-row">
            <PathField kind="folder" value={folder} onChange={setFolder} />
            <Button
              small
              disabled={folder.trim() === "" || agents.folders.includes(folder.trim())}
              onClick={() => {
                set({ folders: [...agents.folders, folder.trim()] });
                setFolder("");
              }}
            >
              {t("common.add")}
            </Button>
          </span>
        </div>
      </Field>
      <Field label={t("mcp.allow")}>
        <div className="stack">
          <Checkbox
            checked={agents.allowOverwrite}
            onChange={(allowOverwrite) => set({ allowOverwrite })}
            label={t("mcp.allowOverwrite")}
          />
          <Checkbox checked={agents.allowDelete} onChange={(allowDelete) => set({ allowDelete })} label={t("mcp.allowDelete")} />
          <Checkbox
            checked={agents.allowDownloads}
            onChange={(allowDownloads) => set({ allowDownloads })}
            label={t("mcp.allowDownloads")}
          />
          <Checkbox checked={agents.allowCloud} onChange={(allowCloud) => set({ allowCloud })} label={t("mcp.allowCloud")} />
        </div>
      </Field>
    </Section>
  );
}

export function McpTab() {
  const { t, td, error } = useT();
  const { confirm, notify, showError } = useApp();
  const [servers, setServers] = useState<ServerConfig[]>([]);
  const [statuses, setStatuses] = useState<ServerStatus[]>([]);
  const [snippet, setSnippet] = useState("");
  const [editing, setEditing] = useState<ServerConfig | null>(null);

  const refresh = useCallback(() => {
    Promise.all([api.mcpServers(), api.mcpStatus()])
      .then(([configs, status]) => {
        setServers(configs);
        setStatuses(status);
      })
      .catch(showError);
  }, [showError]);

  useEffect(() => {
    refresh();
    api
      .mcpSnippet()
      .then((value) => setSnippet(JSON.stringify(value, null, 2)))
      .catch(showError);
  }, [refresh, showError]);
  useAppEvent("mcp:changed", refresh);

  const save = async (server: ServerConfig) => {
    try {
      await api.mcpServerSave(server);
      setEditing(null);
      refresh();
    } catch (failure) {
      showError(failure);
    }
  };

  const remove = async (server: ServerConfig) => {
    const agreed = await confirm({
      title: t("mcp.removeTitle"),
      text: t("mcp.removeText", { name: server.name }),
      confirmLabel: t("common.delete"),
      danger: true,
    });
    if (agreed) {
      api.mcpServerRemove(server.id).then(refresh).catch(showError);
    }
  };

  const connected = servers.filter(
    (server) => server.enabled && statuses.some((status) => status.id === server.id && status.connected),
  ).length;

  return (
    <>
      <Section title={t("mcp.serverTitle")} collapsible defaultOpen={false}>
        <p className="tool-text">{t("mcp.serverText")}</p>
        <pre className="snippet">{snippet}</pre>
        <div className="report-actions">
          <Button small onClick={() => void copyText(snippet).then((copied) => copied && notify(t("common.copied")))}>
            {t("common.copy")}
          </Button>
        </div>
      </Section>

      <AgentAccess />

      <Section
        title={t("mcp.clientsTitle")}
        summary={
          servers.length === 0 ? null : (
            <>
              {connected > 0 ? <Tag tone="ok">{t("mcp.connected")}</Tag> : null}
              <span>
                {connected} / {servers.length}
              </span>
            </>
          )
        }
        collapsible
        defaultOpen={false}
        actions={
          <Button small variant="primary" onClick={() => setEditing(BLANK)}>
            {t("mcp.add")}
          </Button>
        }
      >
        <p className="tool-text">{t("mcp.clientsText")}</p>
        {servers.length === 0 ? <EmptyState title={t("mcp.empty")} /> : null}
        {servers.map((server) => {
          const status = statuses.find((item) => item.id === server.id);
          return (
            <div key={server.id} className="card card-wide">
              <div className="card-title">
                {server.name}
                {!server.enabled ? (
                  <Tag>{t("watch.off")}</Tag>
                ) : status?.connected ? (
                  <Tag tone="ok">{t("mcp.connected")}</Tag>
                ) : (
                  <Tag tone="danger">{t("mcp.disconnected")}</Tag>
                )}
              </div>
              <div className="card-text">
                {server.transport === "stdio" ? [server.command, ...server.args].join(" ") : server.url}
              </div>
              {status?.error ? <div className="report-error">{error(status.error)}</div> : null}
              {(status?.tools ?? []).map((tool) => (
                <div key={tool.name} className="tool-permission" title={tool.description}>
                  <span className="tool-permission-name">{tool.name}</span>
                  <Select
                    value={tool.permission}
                    options={PERMISSIONS.map((value) => ({ value, label: td(`mcp.permission.${value}`) }))}
                    onChange={(permission) =>
                      api.mcpToolPermission(server.id, tool.name, permission).then(refresh).catch(showError)
                    }
                  />
                </div>
              ))}
              <div className="card-actions">
                <Button small onClick={() => void save({ ...server, enabled: !server.enabled })}>
                  {server.enabled ? t("watch.pause") : t("watch.resume")}
                </Button>
                <Button small disabled={!server.enabled} onClick={() => api.mcpReconnect(server.id).catch(showError)}>
                  {t("mcp.reconnect")}
                </Button>
                <Button small onClick={() => setEditing(server)}>
                  {t("common.edit")}
                </Button>
                <Button small onClick={() => void remove(server)}>
                  {t("common.delete")}
                </Button>
              </div>
            </div>
          );
        })}
      </Section>
      {editing ? <ServerDialog server={editing} onSave={(server) => void save(server)} onClose={() => setEditing(null)} /> : null}
    </>
  );
}
