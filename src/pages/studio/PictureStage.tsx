// A generated picture in a box it always fits. With the picture it was made
// from, the two lie under a curtain: before on the left, after on the right.
// In the dialog the wheel zooms and a drag moves the picture.

import { useEffect, useRef, useState, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import { previewUrl } from "../../api/urls";
import { useT } from "../../i18n";
import { Button, cx } from "../../ui";

interface View {
  scale: number;
  x: number;
  y: number;
}

const FIT: View = { scale: 1, x: 0, y: 0 };
const MAX_SCALE = 8;
/** One notch of the wheel, and one press of a button. */
const STEP = 1.25;
/** What a double click zooms to. */
const CLOSE = 2.5;

/** Keeps the picture over the whole box, so no empty edge comes into view. */
function held(view: View, width: number, height: number): View {
  if (view.scale <= 1) {
    return FIT;
  }
  return {
    scale: view.scale,
    x: Math.min(0, Math.max(width * (1 - view.scale), view.x)),
    y: Math.min(0, Math.max(height * (1 - view.scale), view.y)),
  };
}

/** The view after zooming by `factor`, with the point under the cursor kept in place. */
function zoomedAt(view: View, pointX: number, pointY: number, factor: number, width: number, height: number): View {
  const scale = Math.min(MAX_SCALE, Math.max(1, view.scale * factor));
  const grown = scale / view.scale;
  return held(
    {
      scale,
      x: pointX - (pointX - view.x) * grown,
      y: pointY - (pointY - view.y) * grown,
    },
    width,
    height,
  );
}

export function PictureStage({
  file,
  before,
  side,
  ratio,
  zoom = false,
  onOpen,
}: {
  file: string;
  /** The picture this one was made from. */
  before?: string;
  /** The longest side asked from the core, in pixels. */
  side: number;
  /** Width over height of the box. Without it the box fills its parent. */
  ratio?: number;
  /** The wheel zooms and a drag moves the picture. */
  zoom?: boolean;
  /** A double click, when the picture is not zoomed by it. */
  onOpen?: () => void;
}) {
  const { t } = useT();
  const box = useRef<HTMLDivElement>(null);
  const drag = useRef<{ pointX: number; pointY: number; x: number; y: number } | null>(null);
  const [view, setView] = useState<View>(FIT);
  const [split, setSplit] = useState(50);

  useEffect(() => setView(FIT), [file]);

  useEffect(() => {
    const node = box.current;
    if (!node || !zoom) {
      return;
    }
    // React listens to the wheel passively, and the dialog must not scroll.
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const rect = node.getBoundingClientRect();
      const factor = event.deltaY < 0 ? STEP : 1 / STEP;
      setView((current) => zoomedAt(current, event.clientX - rect.left, event.clientY - rect.top, factor, rect.width, rect.height));
    };
    node.addEventListener("wheel", onWheel, { passive: false });
    return () => node.removeEventListener("wheel", onWheel);
  }, [zoom]);

  const zoomBy = (factor: number) => {
    const rect = box.current?.getBoundingClientRect();
    if (rect) {
      setView((current) => zoomedAt(current, rect.width / 2, rect.height / 2, factor, rect.width, rect.height));
    }
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!zoom || view.scale <= 1 || event.button !== 0) {
      return;
    }
    drag.current = { pointX: event.clientX, pointY: event.clientY, x: view.x, y: view.y };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const from = drag.current;
    if (!from) {
      return;
    }
    const rect = event.currentTarget.getBoundingClientRect();
    setView((current) =>
      held(
        { scale: current.scale, x: from.x + event.clientX - from.pointX, y: from.y + event.clientY - from.pointY },
        rect.width,
        rect.height,
      ),
    );
  };

  const onPointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (drag.current) {
      drag.current = null;
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };

  const onDoubleClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    if (!zoom) {
      onOpen?.();
      return;
    }
    const rect = event.currentTarget.getBoundingClientRect();
    setView((current) =>
      current.scale > 1 ? FIT : zoomedAt(current, event.clientX - rect.left, event.clientY - rect.top, CLOSE, rect.width, rect.height),
    );
  };

  const moved = { transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})` };
  // The controls sit on the picture; a press on them is not a drag of it.
  const apart = (event: { stopPropagation: () => void }) => event.stopPropagation();

  return (
    <div
      ref={box}
      className={cx("stage", ratio === undefined ? "stage-fill" : "stage-tile", zoom && view.scale > 1 && "stage-held")}
      style={ratio === undefined ? undefined : { aspectRatio: String(ratio) }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={onDoubleClick}
    >
      {before ? (
        <div className="stage-layer">
          <img className="stage-picture" src={previewUrl({ path: before, modified: 0 }, side)} alt="" draggable={false} style={moved} />
        </div>
      ) : null}
      <div className="stage-layer" style={before ? { clipPath: `inset(0 0 0 ${split}%)` } : undefined}>
        <img className="stage-picture" src={previewUrl({ path: file, modified: 0 }, side)} alt="" draggable={false} style={moved} />
      </div>
      {before ? (
        <>
          <div className="stage-line" style={{ left: `${split}%` }} />
          <span className="stage-label stage-label-before">{t("studio.before")}</span>
          <span className="stage-label stage-label-after">{t("studio.after")}</span>
          <input
            className="stage-range"
            type="range"
            min={0}
            max={100}
            step={0.5}
            value={split}
            aria-label={t("studio.beforeAfter")}
            onChange={(event) => setSplit(Number(event.target.value))}
            onPointerDown={apart}
            onDoubleClick={apart}
          />
        </>
      ) : null}
      {zoom ? (
        <div className="stage-tools" onPointerDown={apart} onDoubleClick={apart}>
          <Button small disabled={view.scale <= 1} onClick={() => zoomBy(1 / STEP)}>
            −
          </Button>
          <Button small disabled={view.scale <= 1} title={t("editor.zoomFit")} onClick={() => setView(FIT)}>
            ×{view.scale.toFixed(1)}
          </Button>
          <Button small disabled={view.scale >= MAX_SCALE} onClick={() => zoomBy(STEP)}>
            +
          </Button>
        </div>
      ) : null}
    </div>
  );
}
