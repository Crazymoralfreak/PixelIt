// The picture being changed: a brush for the area, and a choice of how far
// to grow the canvas so the generator can paint past its edges.

import { useEffect, useState } from "react";
import { thumbUrl } from "../../api/urls";
import { useT } from "../../i18n";
import { BRUSH_EDIT, BrushOverlay } from "../editor/Overlay";
import type { Expand, Stroke } from "../../types";
import { Button, Field, Segmented, Slider } from "../../ui";
import { ASPECTS, NO_EXPAND, marginsForRatio } from "./format";

const BRUSH_MIN = 0.5;
const BRUSH_MAX = 30;

function nextBrush(brush: number, grow: boolean): number {
  const next = grow ? Math.floor(brush + 1) : Math.ceil(brush - 1);
  return Math.min(BRUSH_MAX, Math.max(BRUSH_MIN, next));
}

export function EditSource({
  path,
  strokes,
  expand,
  onStrokes,
  onExpand,
}: {
  path: string;
  strokes: Stroke[];
  expand: Expand;
  onStrokes: (strokes: Stroke[]) => void;
  onExpand: (expand: Expand) => void;
}) {
  const { t, td } = useT();
  const [brush, setBrush] = useState(8);
  const [natural, setNatural] = useState({ width: 0, height: 0 });
  const [amount, setAmount] = useState<25 | 50 | 100>(25);
  const [sides, setSides] = useState({ left: true, right: true, top: false, bottom: false });
  const growing = expand.left + expand.right + expand.top + expand.bottom > 0;

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
        return;
      }
      if (event.code === "BracketLeft" || event.code === "BracketRight") {
        event.preventDefault();
        setBrush((current) => nextBrush(current, event.code === "BracketRight"));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const growSides = (nextAmount: number, nextSides: typeof sides, size = natural) => {
    if (size.width === 0 || size.height === 0) {
      return;
    }
    onExpand({
      left: nextSides.left ? Math.round((size.width * nextAmount) / 100) : 0,
      right: nextSides.right ? Math.round((size.width * nextAmount) / 100) : 0,
      top: nextSides.top ? Math.round((size.height * nextAmount) / 100) : 0,
      bottom: nextSides.bottom ? Math.round((size.height * nextAmount) / 100) : 0,
    });
  };

  const toggleSide = (side: keyof typeof sides) => {
    const next = { ...sides, [side]: !sides[side] };
    setSides(next);
    growSides(amount, next);
  };

  return (
    <div className="studio-source">
      <div className="studio-canvas">
        <img
          src={thumbUrl({ path, modified: 0 }, 1024)}
          alt=""
          draggable={false}
          onLoad={(event) => {
            const width = event.currentTarget.naturalWidth;
            const height = event.currentTarget.naturalHeight;
            setNatural({ width, height });
          }}
        />
        {natural.width > 0 ? (
          <BrushOverlay
            width={natural.width}
            height={natural.height}
            brush={brush}
            color={BRUSH_EDIT}
            strokes={strokes}
            onStrokes={onStrokes}
          />
        ) : null}
      </div>
      <Field label={t("editor.brush")} hint={t("studio.brushHint")}>
        <Slider value={brush} onChange={setBrush} min={BRUSH_MIN} max={BRUSH_MAX} step={0.5} reset={8} />
        <Button small disabled={strokes.length === 0} onClick={() => onStrokes(strokes.slice(0, -1))}>
          {t("op.removeLast")}
        </Button>
        <Button small disabled={strokes.length === 0} onClick={() => onStrokes([])}>
          {t("common.clear")}
        </Button>
      </Field>
      <Field label={t("studio.expand")} hint={t("studio.expandHint")}>
        <Segmented
          value={growing ? "on" : "off"}
          options={[
            { value: "off", label: t("studio.expandOff") },
            { value: "on", label: t("studio.expandOn") },
          ]}
          onChange={(value) => {
            if (value === "off") {
              onExpand(NO_EXPAND);
            } else {
              growSides(amount, sides);
            }
          }}
        />
      </Field>
      {growing ? (
        <>
          <Field label={t("studio.expandSides")}>
            {(["left", "right", "top", "bottom"] as const).map((side) => (
              <Button key={side} small variant={sides[side] ? "primary" : "default"} onClick={() => toggleSide(side)}>
                {td(`studio.side.${side}`)}
              </Button>
            ))}
          </Field>
          <Field label={t("studio.expandAmount")}>
            <Segmented
              value={String(amount)}
              options={[25, 50, 100].map((value) => ({ value: String(value), label: `${value}%` }))}
              onChange={(value) => {
                const next = Number(value) as 25 | 50 | 100;
                setAmount(next);
                growSides(next, sides);
              }}
            />
          </Field>
          <Field label={t("studio.expandRatio")}>
            {ASPECTS.map(([ratioW, ratioH]) => (
              <Button
                key={`${ratioW}:${ratioH}`}
                small
                onClick={() => onExpand(marginsForRatio(natural.width, natural.height, ratioW, ratioH))}
              >
                {ratioW}:{ratioH}
              </Button>
            ))}
          </Field>
        </>
      ) : null}
    </div>
  );
}
