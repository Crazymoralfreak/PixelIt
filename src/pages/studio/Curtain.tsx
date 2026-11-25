// A wipe between the picture a change started from and the picture it made.

import { useState } from "react";
import { thumbUrl } from "../../api/urls";
import { useT } from "../../i18n";

export function Curtain({ before, after }: { before: string; after: string }) {
  const { t } = useT();
  const [split, setSplit] = useState(50);
  return (
    <div className="curtain">
      <img src={thumbUrl({ path: before, modified: 0 }, 1024)} alt="" draggable={false} />
      <div className="curtain-top" style={{ width: `${split}%` }}>
        <img src={thumbUrl({ path: after, modified: 0 }, 1024)} alt="" draggable={false} />
      </div>
      <input
        className="curtain-range"
        type="range"
        min={0}
        max={100}
        value={split}
        aria-label={t("studio.beforeAfter")}
        onChange={(event) => setSplit(Number(event.target.value))}
      />
    </div>
  );
}
