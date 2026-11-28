// Whether the generator of the form answers, with the buttons that start and
// stop a ComfyUI or Forge installed on this computer. Nothing is shown for a
// generator that is not a program of this computer.

import { useEffect, useRef, useState } from "react";
import { api } from "../../api";
import { useAppEvent } from "../../api/events";
import { useApp } from "../../app/context";
import { useT } from "../../i18n";
import { takesTuning } from "../../lib/generation";
import type { GenerationProvider, GeneratorService, ServiceState } from "../../types";
import { Button, Tag } from "../../ui";

/** How often the address is asked, in milliseconds. */
const WAITING = 1500;
const IDLE = 8000;

function toneOf(state: ServiceState): "plain" | "ok" | "warning" | "danger" {
  switch (state) {
    case "ready":
      return "ok";
    case "starting":
      return "warning";
    case "stopped":
    case "remote":
      return "plain";
    case "missing":
      return "danger";
    default: {
      const unreachable: never = state;
      return unreachable;
    }
  }
}

export function ServiceStatus({
  provider,
  onReady,
}: {
  provider: GenerationProvider;
  /** The generator began to answer: its lists can be asked for again. */
  onReady: () => void;
}) {
  const { t, td } = useT();
  const { navigate, showError } = useApp();
  const [service, setService] = useState<GeneratorService | null>(null);
  const [busy, setBusy] = useState(false);
  const previous = useRef<ServiceState | null>(null);
  const ready = useRef(onReady);
  ready.current = onReady;
  const shown = useRef(provider);
  shown.current = provider;
  // ComfyUI and Forge are the programs this computer can run.
  const local = takesTuning(provider);

  const ask = () => {
    if (!local) {
      return;
    }
    api
      .generatorStatus(provider)
      .then((next) => {
        if (shown.current !== provider) {
          // An answer about the generator that was chosen before.
          return;
        }
        if (previous.current !== null && previous.current !== "ready" && next.state === "ready") {
          ready.current();
        }
        previous.current = next.state;
        setService(next);
      })
      .catch(() => setService(null));
  };

  const state = service?.state ?? null;
  useEffect(() => {
    if (!local) {
      previous.current = null;
      setService(null);
      return;
    }
    if (state === null) {
      ask();
    }
    const timer = window.setInterval(ask, state === "starting" ? WAITING : IDLE);
    return () => window.clearInterval(timer);
    // The pace follows the state; `ask` reads nothing but the provider.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [provider, local, state]);
  useEffect(() => {
    previous.current = null;
    setService(null);
  }, [provider]);
  useAppEvent("ai:changed", ask);

  if (!local || !service || service.state === "remote") {
    return null;
  }
  const app = service.app;

  const act = (work: Promise<void>) => {
    setBusy(true);
    work
      .catch(showError)
      .finally(() => {
        setBusy(false);
        ask();
      });
  };

  return (
    <div className="studio-service">
      <Tag tone={toneOf(service.state)}>
        {td(`ai.provider.${provider}`)} · {td(`studio.service.${service.state}`)}
      </Tag>
      {service.state === "stopped" && app !== "" ? (
        <Button small variant="primary" disabled={busy} onClick={() => act(api.appsStart(app))}>
          {t("ai.apps.start")}
        </Button>
      ) : null}
      {service.ours && app !== "" && (service.state === "ready" || service.state === "starting") ? (
        <Button small disabled={busy} onClick={() => act(api.appsStop(app))}>
          {t("ai.apps.stop")}
        </Button>
      ) : null}
      {service.state === "missing" ? (
        <Button small onClick={() => navigate("ai")}>
          {t("studio.setUp")}
        </Button>
      ) : null}
    </div>
  );
}
