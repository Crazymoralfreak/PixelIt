// Form of the output settings: format, quality, target, metadata and where
// the results go. Shared by the batch, the editor, recipes and the defaults.

import { useT } from "../i18n";
import { canBeLossless, FORMAT_NAMES, hasQuality } from "../lib/pipeline";
import {
  OUTPUT_FORMATS,
  type Conflict,
  type Destination,
  type ImageFormat,
  type OutputFormat,
  type OutputSpec,
  type Target,
} from "../types";
import { Checkbox, ColorInput, Field, NumberInput, Section, Segmented, Select, Slider, TextInput } from "../ui";
import { PathField } from "./PathField";

const AUTO_CANDIDATES: readonly ImageFormat[] = ["jpeg", "webp", "avif", "jxl", "png"];
const EFFORTS = ["fast", "balanced", "max"] as const;
const DEPTHS = ["auto", "eight", "sixteen"] as const;
const SUBSAMPLINGS = ["auto", "full", "half"] as const;
const METADATA_MODES = ["keep", "strip", "custom"] as const;
const CONFLICTS: readonly Conflict[] = ["rename", "overwrite", "skip"];
const TARGETS: ReadonlyArray<Target["kind"]> = ["none", "bytes", "quality"];
const DESTINATIONS: ReadonlyArray<Destination["kind"]> = ["sameFolder", "subfolder", "folder"];

interface OutputEditorProps {
  value: OutputSpec;
  onChange: (value: OutputSpec) => void;
  /** Leaves out where the results go, for a single "save as". */
  withoutDestination?: boolean;
}

export function OutputEditor({ value, onChange, withoutDestination }: OutputEditorProps) {
  const { t, td } = useT();
  const set = (patch: Partial<OutputSpec>) => onChange({ ...value, ...patch });
  const labelled = <T extends string>(prefix: string, values: readonly T[]) =>
    values.map((item) => ({ value: item, label: td(`${prefix}.${item}`) }));

  const formats: Array<{ value: OutputFormat; label: string }> = [
    { value: "same", label: t("output.formatSame") },
    { value: "auto", label: t("output.formatAuto") },
    ...OUTPUT_FORMATS.map((format) => ({ value: format, label: FORMAT_NAMES[format] })),
  ];

  const setTarget = (kind: Target["kind"]) => {
    switch (kind) {
      case "none":
        set({ target: { kind } });
        break;
      case "bytes":
        set({ target: { kind, bytes: 200 * 1024 } });
        break;
      case "quality":
        set({ target: { kind, score: 80 } });
        break;
      default: {
        const unreachable: never = kind;
        throw new Error(`Unknown target: ${String(unreachable)}`);
      }
    }
  };

  const setDestination = (kind: Destination["kind"]) => {
    switch (kind) {
      case "sameFolder":
        set({ destination: { kind } });
        break;
      case "subfolder":
        set({ destination: { kind, name: "PixelIt" } });
        break;
      case "folder":
        set({ destination: { kind, path: "" } });
        break;
      default: {
        const unreachable: never = kind;
        throw new Error(`Unknown destination: ${String(unreachable)}`);
      }
    }
  };

  const toggleAuto = (format: ImageFormat, on: boolean) => {
    const rest = value.autoFormats.filter((item) => item !== format);
    set({ autoFormats: on ? [...rest, format] : rest });
  };

  const lossless = value.lossless && canBeLossless(value.format);

  return (
    <div className="output-editor">
      <Field label={t("output.format")} hint={value.format === "auto" ? t("output.formatAutoHint") : undefined}>
        <Select value={value.format} options={formats} onChange={(format) => set({ format })} />
      </Field>
      {value.format === "auto" ? (
        <Field label={t("output.autoFormats")}>
          {AUTO_CANDIDATES.map((format) => (
            <Checkbox
              key={format}
              checked={value.autoFormats.includes(format)}
              onChange={(on) => toggleAuto(format, on)}
              label={FORMAT_NAMES[format]}
            />
          ))}
        </Field>
      ) : null}
      {canBeLossless(value.format) ? (
        <Field label={t("output.lossless")}>
          <Checkbox
            checked={value.lossless}
            onChange={(on) => set({ lossless: on })}
            label={t("output.losslessHint")}
          />
        </Field>
      ) : null}
      {hasQuality(value.format) && !lossless ? (
        <>
          <Field label={t("output.target")}>
            <Segmented value={value.target.kind} options={labelled("output.targetKind", TARGETS)} onChange={setTarget} />
          </Field>
          {value.target.kind === "none" ? (
            <Field label={t("output.quality")}>
              <Slider value={value.quality} onChange={(quality) => set({ quality })} min={1} max={100} reset={82} />
            </Field>
          ) : null}
          {value.target.kind === "bytes" ? (
            <Field label={t("output.targetBytes")} hint={t("output.targetBytesHint")}>
              <NumberInput
                value={Math.round(value.target.bytes / 1024)}
                onChange={(kilobytes) => set({ target: { kind: "bytes", bytes: kilobytes * 1024 } })}
                min={1}
                max={1024 * 1024}
                suffix="KB"
                wide
              />
            </Field>
          ) : null}
          {value.target.kind === "quality" ? (
            <Field label={t("output.targetScore")} hint={t("output.targetScoreHint")}>
              <Slider
                value={value.target.score}
                onChange={(score) => set({ target: { kind: "quality", score } })}
                min={30}
                max={95}
                reset={80}
              />
            </Field>
          ) : null}
        </>
      ) : null}
      <Field label={t("output.effort")}>
        <Segmented
          value={value.effort}
          options={labelled("output.effortKind", EFFORTS)}
          onChange={(effort) => set({ effort })}
        />
      </Field>

      <Field label={t("output.metadata")}>
        <Segmented
          value={value.metadata.mode}
          options={labelled("output.metadataMode", METADATA_MODES)}
          onChange={(mode) => set({ metadata: { ...value.metadata, mode } })}
        />
      </Field>
      {value.metadata.mode === "custom" ? (
        <Field label="">
          <Checkbox
            checked={value.metadata.keepExif}
            onChange={(keepExif) => set({ metadata: { ...value.metadata, keepExif } })}
            label="EXIF"
          />
          <Checkbox
            checked={value.metadata.keepGps}
            onChange={(keepGps) => set({ metadata: { ...value.metadata, keepGps } })}
            label={t("output.keepGps")}
          />
          <Checkbox
            checked={value.metadata.keepXmp}
            onChange={(keepXmp) => set({ metadata: { ...value.metadata, keepXmp } })}
            label="XMP"
          />
          <Checkbox
            checked={value.metadata.keepIcc}
            onChange={(keepIcc) => set({ metadata: { ...value.metadata, keepIcc } })}
            label={t("output.keepIcc")}
          />
        </Field>
      ) : null}

      {withoutDestination ? null : (
        <>
          <Field label={t("output.destination")}>
            <Select
              value={value.destination.kind}
              options={labelled("output.destinationKind", DESTINATIONS)}
              onChange={setDestination}
            />
          </Field>
          {value.destination.kind === "subfolder" ? (
            <Field label={t("output.subfolder")}>
              <TextInput
                value={value.destination.name}
                onChange={(name) => set({ destination: { kind: "subfolder", name } })}
              />
            </Field>
          ) : null}
          {value.destination.kind === "folder" ? (
            <>
              <Field label={t("output.folder")}>
                <PathField
                  kind="folder"
                  value={value.destination.path}
                  onChange={(path) => set({ destination: { kind: "folder", path } })}
                />
              </Field>
              <Field label="">
                <Checkbox
                  checked={value.keepStructure}
                  onChange={(keepStructure) => set({ keepStructure })}
                  label={t("output.keepStructure")}
                />
              </Field>
            </>
          ) : null}
          <Field label={t("output.naming")} hint={t("output.namingHint")}>
            <TextInput value={value.naming} onChange={(naming) => set({ naming })} wide />
          </Field>
          <Field label={t("output.conflict")}>
            <Select
              value={value.conflict}
              options={labelled("output.conflictKind", CONFLICTS)}
              onChange={(conflict) => set({ conflict })}
            />
          </Field>
          <Field label="" hint={t("output.replaceOriginalHint")}>
            <Checkbox
              checked={value.replaceOriginal}
              onChange={(replaceOriginal) => set({ replaceOriginal })}
              label={t("output.replaceOriginal")}
            />
          </Field>
        </>
      )}

      <Section title={t("output.advanced")} collapsible defaultOpen={false}>
        <Field label={t("output.depth")}>
          <Select value={value.depth} options={labelled("output.depthKind", DEPTHS)} onChange={(depth) => set({ depth })} />
        </Field>
        <Field label={t("output.subsampling")}>
          <Select
            value={value.subsampling}
            options={labelled("output.subsamplingKind", SUBSAMPLINGS)}
            onChange={(subsampling) => set({ subsampling })}
          />
        </Field>
        <Field label="">
          <Checkbox
            checked={value.progressive}
            onChange={(progressive) => set({ progressive })}
            label={t("output.progressive")}
          />
        </Field>
        <Field label={t("output.color")}>
          <Select
            value={value.color}
            options={labelled("output.colorKind", ["srgb", "original"] as const)}
            onChange={(color) => set({ color })}
          />
        </Field>
        <Field label={t("output.background")} hint={t("output.backgroundHint")}>
          <ColorInput value={value.background} onChange={(background) => set({ background })} />
        </Field>
        <Field label={t("output.copyright")}>
          <TextInput
            value={value.metadata.copyright}
            onChange={(copyright) => set({ metadata: { ...value.metadata, copyright } })}
            wide
          />
        </Field>
        <Field label={t("output.author")}>
          <TextInput
            value={value.metadata.author}
            onChange={(author) => set({ metadata: { ...value.metadata, author } })}
            wide
          />
        </Field>
      </Section>
    </div>
  );
}
