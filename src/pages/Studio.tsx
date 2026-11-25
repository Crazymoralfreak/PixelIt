// The studio: pictures from a prompt, the pictures already made, and a model
// that looks at pictures to describe, tag, name and read them.

import { useEffect, useRef, useState } from "react";
import { useApp } from "../app/context";
import { useT } from "../i18n";
import type { GeneratedRecord } from "../types";
import { PageFrame, Tabs } from "../ui";
import { Generate } from "./studio/Generate";
import type { StudioHandoff } from "./studio/handoff";
import { History } from "./studio/History";
import { useStyles } from "./studio/Styles";
import { Vision } from "./studio/Vision";

export default function StudioPage() {
  const { t } = useT();
  const { ai, studioDrop } = useApp();
  const { styles } = useStyles();
  const [tab, setTab] = useState<"generate" | "history" | "vision">("generate");
  const [handoff, setHandoff] = useState<StudioHandoff | null>(null);
  const [seenDrop, setSeenDrop] = useState(0);
  const token = useRef(1);

  useEffect(() => {
    if (!studioDrop || studioDrop.token === seenDrop) {
      return;
    }
    setSeenDrop(studioDrop.token);
    setTab("generate");
    setHandoff({ token: studioDrop.token, kind: "drop", path: studioDrop.path });
  }, [studioDrop, seenDrop]);

  const send = (next: StudioHandoff) => {
    setTab("generate");
    setHandoff(next);
  };
  const fresh = (): number => token.current++;

  return (
    <PageFrame title={t("nav.studio")}>
      <Tabs
        value={tab}
        options={[
          { value: "generate", label: t("studio.tabGenerate") },
          { value: "history", label: t("studio.tabHistory") },
          { value: "vision", label: t("studio.tabVision") },
        ]}
        onChange={setTab}
      />
      <div hidden={tab !== "generate"}>
        <Generate handoff={handoff} />
      </div>
      <div hidden={tab !== "history"}>
        <History
          styles={styles}
          upscaleReady={ai?.capabilities.upscale ?? false}
          onRepeat={(record: GeneratedRecord) => send({ token: fresh(), kind: "repeat", record })}
          onSeed={(seed) => send({ token: fresh(), kind: "seed", seed })}
          onVariants={(record) => send({ token: fresh(), kind: "variants", record })}
          onSimilar={(record, path) => send({ token: fresh(), kind: "similar", record, path })}
          onEdit={(path) => send({ token: fresh(), kind: "edit", path })}
        />
      </div>
      <div className="column" hidden={tab !== "vision"}>
        <Vision />
      </div>
    </PageFrame>
  );
}
