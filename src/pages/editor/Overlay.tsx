// What is drawn over the picture of the editor while a step is being shaped
// by hand: the crop box, annotations, the mask of an object to remove.
// Coordinates of the steps are fractions of the picture, so the overlay works
// in the pixels of the shown picture and converts on the way.

import { useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { defaultShape } from "../../lib/pipeline";
import type { AnnotateOp, CropOp, InpaintOp, Op, Shape, ShapeKind, Stroke } from "../../types";

export interface ToolState {
  shape: ShapeKind;
  color: string;
  /** Stroke thickness, in percent of the short side. */
  width: number;
  fill: boolean;
  text: string;
  /** Text height, in percent of the short side. */
  size: number;
  font: string;
  /** Brush of the mask, in percent of the short side. */
  brush: number;
}

export const DEFAULT_TOOL: ToolState = {
  shape: "arrow",
  color: "#e53935",
  width: 0.6,
  fill: false,
  text: "",
  size: 4,
  font: "",
  brush: 4,
};

interface OverlayProps {
  /** Size of the picture under the overlay, in its own pixels. */
  width: number;
  height: number;
  /** Screen pixels per picture pixel, to keep handles the same size on screen. */
  zoom: number;
  op: CropOp | AnnotateOp | InpaintOp;
  tool: ToolState;
  onChange: (op: Op) => void;
}

type Point = [number, number];

const MIN_CROP = 0.01;
const HANDLES = ["nw", "n", "ne", "e", "se", "s", "sw", "w"] as const;
type Handle = (typeof HANDLES)[number];

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** The pointer as fractions of the overlay. */
function fraction(event: ReactPointerEvent<SVGSVGElement>): Point {
  const box = event.currentTarget.getBoundingClientRect();
  return [
    clamp((event.clientX - box.left) / box.width, 0, 1),
    clamp((event.clientY - box.top) / box.height, 0, 1),
  ];
}

function dragCrop(start: CropOp, mode: Handle | "move", dx: number, dy: number): CropOp {
  let left = start.x;
  let top = start.y;
  let right = start.x + start.width;
  let bottom = start.y + start.height;
  if (mode === "move") {
    const width = right - left;
    const height = bottom - top;
    left = clamp(left + dx, 0, 1 - width);
    top = clamp(top + dy, 0, 1 - height);
    right = left + width;
    bottom = top + height;
  } else {
    if (mode.includes("w")) {
      left = clamp(left + dx, 0, right - MIN_CROP);
    }
    if (mode.includes("e")) {
      right = clamp(right + dx, left + MIN_CROP, 1);
    }
    if (mode.includes("n")) {
      top = clamp(top + dy, 0, bottom - MIN_CROP);
    }
    if (mode.includes("s")) {
      bottom = clamp(bottom + dy, top + MIN_CROP, 1);
    }
  }
  return { ...start, x: left, y: top, width: right - left, height: bottom - top };
}

function handlePosition(handle: Handle, crop: CropOp): Point {
  const x = handle.includes("w") ? crop.x : handle.includes("e") ? crop.x + crop.width : crop.x + crop.width / 2;
  const y = handle.includes("n") ? crop.y : handle.includes("s") ? crop.y + crop.height : crop.y + crop.height / 2;
  return [x, y];
}

function CropOverlay({ width, height, zoom, op, onChange }: OverlayProps & { op: CropOp }) {
  const drag = useRef<{ start: CropOp; mode: Handle | "move"; from: Point } | null>(null);
  const grip = 10 / zoom;
  const line = 1 / zoom;
  const box = { x: op.x * width, y: op.y * height, width: op.width * width, height: op.height * height };

  const begin = (event: ReactPointerEvent<SVGSVGElement>) => {
    const point = fraction(event);
    const target = event.target as SVGElement;
    const handle = target.dataset.handle as Handle | undefined;
    const inside =
      point[0] >= op.x && point[0] <= op.x + op.width && point[1] >= op.y && point[1] <= op.y + op.height;
    if (handle) {
      drag.current = { start: op, mode: handle, from: point };
    } else if (inside) {
      drag.current = { start: op, mode: "move", from: point };
    } else {
      // A press outside starts a new box from that corner.
      const start: CropOp = {
        ...op,
        x: Math.min(point[0], 1 - MIN_CROP),
        y: Math.min(point[1], 1 - MIN_CROP),
        width: MIN_CROP,
        height: MIN_CROP,
      };
      drag.current = { start, mode: "se", from: point };
      onChange(start);
    }
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const move = (event: ReactPointerEvent<SVGSVGElement>) => {
    const current = drag.current;
    if (!current) {
      return;
    }
    const point = fraction(event);
    onChange(dragCrop(current.start, current.mode, point[0] - current.from[0], point[1] - current.from[1]));
  };

  const end = () => {
    drag.current = null;
  };

  return (
    <svg
      className="overlay overlay-crop"
      viewBox={`0 0 ${width} ${height}`}
      onPointerDown={begin}
      onPointerMove={move}
      onPointerUp={end}
      onPointerCancel={end}
    >
      <path
        fillRule="evenodd"
        fill="rgba(0, 0, 0, 0.55)"
        d={`M0 0H${width}V${height}H0Z M${box.x} ${box.y}h${box.width}v${box.height}h${-box.width}Z`}
      />
      <rect {...box} fill="none" stroke="#fff" strokeWidth={line} />
      {[1, 2].map((part) => (
        <g key={part} stroke="rgba(255, 255, 255, 0.45)" strokeWidth={line}>
          <line
            x1={box.x + (box.width * part) / 3}
            y1={box.y}
            x2={box.x + (box.width * part) / 3}
            y2={box.y + box.height}
          />
          <line
            x1={box.x}
            y1={box.y + (box.height * part) / 3}
            x2={box.x + box.width}
            y2={box.y + (box.height * part) / 3}
          />
        </g>
      ))}
      {HANDLES.map((handle) => {
        const [x, y] = handlePosition(handle, op);
        return (
          <rect
            key={handle}
            data-handle={handle}
            className={`crop-handle crop-handle-${handle}`}
            x={x * width - grip / 2}
            y={y * height - grip / 2}
            width={grip}
            height={grip}
            fill="#fff"
            stroke="#000"
            strokeWidth={line}
          />
        );
      })}
    </svg>
  );
}

function pathOf(points: Point[], width: number, height: number): string {
  return points.map(([x, y]) => `${x * width},${y * height}`).join(" ");
}

function ShapeView({ shape, width, height }: { shape: Shape; width: number; height: number }) {
  const short = Math.min(width, height);
  const stroke = Math.max(1, (shape.width / 100) * short);
  const [ax, ay, bx, by] = [shape.x1 * width, shape.y1 * height, shape.x2 * width, shape.y2 * height];
  const box = { x: Math.min(ax, bx), y: Math.min(ay, by), width: Math.abs(bx - ax), height: Math.abs(by - ay) };
  const outline = { fill: shape.fill ? shape.color : "none", stroke: shape.color, strokeWidth: stroke };

  switch (shape.kind) {
    case "line":
      return <line x1={ax} y1={ay} x2={bx} y2={by} stroke={shape.color} strokeWidth={stroke} />;
    case "arrow": {
      const length = Math.hypot(bx - ax, by - ay);
      if (length < 0.5) {
        return null;
      }
      const [ux, uy] = [(bx - ax) / length, (by - ay) / length];
      const head = Math.max(2, Math.min(stroke * 5, length * 0.6));
      const [baseX, baseY] = [bx - ux * head, by - uy * head];
      const tip = [
        `${bx},${by}`,
        `${baseX - (uy * head) / 2},${baseY + (ux * head) / 2}`,
        `${baseX + (uy * head) / 2},${baseY - (ux * head) / 2}`,
      ].join(" ");
      return (
        <g>
          <line
            x1={ax}
            y1={ay}
            x2={bx - ux * head * 0.8}
            y2={by - uy * head * 0.8}
            stroke={shape.color}
            strokeWidth={stroke}
          />
          <polygon points={tip} fill={shape.color} />
        </g>
      );
    }
    case "rect":
      return <rect {...box} {...outline} />;
    case "highlight":
      return <rect {...box} fill={shape.color} opacity={0.4} />;
    case "ellipse":
      return (
        <ellipse cx={(ax + bx) / 2} cy={(ay + by) / 2} rx={box.width / 2} ry={box.height / 2} {...outline} />
      );
    case "pen":
      return (
        <polyline
          points={pathOf(shape.points, width, height)}
          fill="none"
          stroke={shape.color}
          strokeWidth={stroke}
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      );
    case "text": {
      const size = Math.max(4, (shape.size / 100) * short);
      return (
        <text
          x={ax}
          y={ay}
          fill={shape.color}
          fontSize={size}
          fontFamily={shape.font || "sans-serif"}
          dominantBaseline="text-before-edge"
        >
          {shape.text}
        </text>
      );
    }
    case "blur":
    case "pixelate":
      return (
        <rect
          {...box}
          fill="rgba(128, 128, 128, 0.55)"
          stroke="#fff"
          strokeWidth={Math.max(1, short / 500)}
          strokeDasharray={`${short / 80} ${short / 80}`}
        />
      );
    default: {
      const unreachable: never = shape.kind;
      return unreachable;
    }
  }
}

function AnnotateOverlay({ width, height, op, tool, onChange }: OverlayProps & { op: AnnotateOp }) {
  const [draft, setDraft] = useState<Shape | null>(null);

  const begin = (event: ReactPointerEvent<SVGSVGElement>) => {
    const [x, y] = fraction(event);
    const shape: Shape = {
      ...defaultShape(tool.shape, tool.color),
      x1: x,
      y1: y,
      x2: x,
      y2: y,
      width: tool.width,
      fill: tool.fill,
      text: tool.text,
      font: tool.font,
      size: tool.size,
      points: tool.shape === "pen" ? [[x, y]] : [],
    };
    if (tool.shape === "text") {
      if (tool.text.trim() !== "") {
        onChange({ ...op, shapes: [...op.shapes, shape] });
      }
      return;
    }
    setDraft(shape);
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const move = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (!draft) {
      return;
    }
    const [x, y] = fraction(event);
    setDraft({
      ...draft,
      x2: x,
      y2: y,
      points: draft.kind === "pen" ? [...draft.points, [x, y]] : draft.points,
    });
  };

  const end = () => {
    if (!draft) {
      return;
    }
    const moved = Math.hypot((draft.x2 - draft.x1) * width, (draft.y2 - draft.y1) * height) >= 3;
    if (moved || draft.points.length > 2) {
      onChange({ ...op, shapes: [...op.shapes, draft] });
    }
    setDraft(null);
  };

  return (
    <svg
      className="overlay overlay-draw"
      viewBox={`0 0 ${width} ${height}`}
      onPointerDown={begin}
      onPointerMove={move}
      onPointerUp={end}
      onPointerCancel={end}
    >
      {op.shapes.map((shape, index) => (
        <ShapeView key={index} shape={shape} width={width} height={height} />
      ))}
      {draft ? <ShapeView shape={draft} width={width} height={height} /> : null}
    </svg>
  );
}

function InpaintOverlay({ width, height, op, tool, onChange }: OverlayProps & { op: InpaintOp }) {
  const [draft, setDraft] = useState<Stroke | null>(null);
  const short = Math.min(width, height);

  const begin = (event: ReactPointerEvent<SVGSVGElement>) => {
    const point = fraction(event);
    // Two equal points make a dot, which a single click should leave.
    setDraft({ points: [point, point], width: tool.brush });
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const move = (event: ReactPointerEvent<SVGSVGElement>) => {
    if (draft) {
      setDraft({ ...draft, points: [...draft.points, fraction(event)] });
    }
  };

  const end = () => {
    if (draft) {
      onChange({ ...op, strokes: [...op.strokes, draft] });
      setDraft(null);
    }
  };

  const strokes = draft ? [...op.strokes, draft] : op.strokes;
  return (
    <svg
      className="overlay overlay-draw"
      viewBox={`0 0 ${width} ${height}`}
      onPointerDown={begin}
      onPointerMove={move}
      onPointerUp={end}
      onPointerCancel={end}
    >
      {strokes.map((stroke, index) => (
        <polyline
          key={index}
          points={pathOf(stroke.points, width, height)}
          fill="none"
          stroke="rgba(255, 45, 85, 0.55)"
          strokeWidth={Math.max(1, (stroke.width / 100) * short)}
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      ))}
    </svg>
  );
}

export function Overlay(props: OverlayProps) {
  const { op } = props;
  switch (op.type) {
    case "crop":
      return <CropOverlay {...props} op={op} />;
    case "annotate":
      return <AnnotateOverlay {...props} op={op} />;
    case "inpaint":
      return <InpaintOverlay {...props} op={op} />;
    default: {
      const unreachable: never = op;
      return unreachable;
    }
  }
}
