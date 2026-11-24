// Tools that make something new out of pictures: icon and social sets, a
// kit for web pages, collages, PDF files, animations, the search for
// duplicates and the screen capture.

import { useEffect, useState, type ReactNode } from "react";
import { api } from "../api";
import { thumbUrl } from "../api/urls";
import { useApp } from "../app/context";
import { FilesField } from "../components/FilesField";
import { PathField } from "../components/PathField";
import { useT } from "../i18n";
import { copyText } from "../lib/clipboard";
import { fileName, folderOf, formatBytes, joinPath } from "../lib/format";
import { FORMAT_NAMES } from "../lib/pipeline";
import type {
  AnimationSpec,
  CollageSpec,
  DuplicateGroup,
  Generated,
  IconPreset,
  ImageFormat,
  PdfSpec,
  SocialSize,
} from "../types";
import {
  Button,
  Checkbox,
  ColorInput,
  EmptyState,
  Field,
  NumberInput,
  PageFrame,
  Section,
  Slider,
  Tabs,
  Tag,
  TextInput,
} from "../ui";

const TOOLS = ["icons", "social", "webKit", "collage", "pdf", "animation", "duplicates", "capture"] as const;
type Tool = (typeof TOOLS)[number];

/** Platform names are the same in every language. */
const ICON_PRESETS: ReadonlyArray<{ value: IconPreset; label: string }> = [
  { value: "favicon", label: "Favicon" },
  { value: "pwa", label: "PWA" },
  { value: "windows", label: "Windows" },
  { value: "android", label: "Android" },
  { value: "ios", label: "iOS" },
  { value: "macos", label: "macOS" },
];
const SOCIAL_NAMES: Record<string, string> = {
  og: "Open Graph",
  x: "X",
  "instagram-square": "Instagram 1:1",
  "instagram-portrait": "Instagram 4:5",
  story: "Stories 9:16",
  youtube: "YouTube",
  linkedin: "LinkedIn",
  pinterest: "Pinterest",
  "facebook-cover": "Facebook",
};
const WEB_FORMATS: readonly ImageFormat[] = ["avif", "webp", "jpeg"];
const MAX_TOLERANCE = 16;

/** Runs an action of a tool, keeping "busy" while it works. */
function useRun(): [boolean, (action: () => Promise<void>) => void] {
  const { showError } = useApp();
  const [busy, setBusy] = useState(false);
  const run = (action: () => Promise<void>) => {
    setBusy(true);
    action()
      .catch(showError)
      .finally(() => setBusy(false));
  };
  return [busy, run];
}

function RunButton({ busy, disabled, onClick, children }: { busy: boolean; disabled?: boolean; onClick: () => void; children: ReactNode }) {
  const { t } = useT();
  return (
    <div className="tool-run">
      <Button variant="primary" disabled={busy || disabled} onClick={onClick}>
        {busy ? t("common.working") : children}
      </Button>
    </div>
  );
}

function GeneratedView({ result }: { result: Generated }) {
  const { t } = useT();
  const { notify, showError } = useApp();
  return (
    <Section title={t("tools.result")}>
      <div className="report-line">{t("tools.filesWritten", { count: result.files.length })}</div>
      {result.blurhash ? <div className="report-line">BlurHash: {result.blurhash}</div> : null}
      {result.snippet ? <pre className="snippet">{result.snippet}</pre> : null}
      <div className="report-actions">
        <Button small onClick={() => api.revealPath(result.folder).catch(showError)}>
          {t("batch.openFolder")}
        </Button>
        {result.snippet ? (
          <Button
            small
            onClick={() => void copyText(result.snippet).then((copied) => copied && notify(t("common.copied")))}
          >
            {t("tools.copySnippet")}
          </Button>
        ) : null}
      </div>
    </Section>
  );
}

function SourceFields({
  source,
  folder,
  onSource,
  onFolder,
}: {
  source: string;
  folder: string;
  onSource: (path: string) => void;
  onFolder: (path: string) => void;
}) {
  const { t } = useT();
  return (
    <>
      <Field label={t("tools.source")}>
        <PathField
          kind="picture"
          value={source}
          onChange={(path) => {
            onSource(path);
            if (folder === "") {
              onFolder(folderOf(path));
            }
          }}
        />
      </Field>
      <Field label={t("tools.folder")} hint={t("tools.folderHint")}>
        <PathField kind="folder" value={folder} onChange={onFolder} />
      </Field>
    </>
  );
}

function IconsTool() {
  const { t } = useT();
  const [source, setSource] = useState("");
  const [folder, setFolder] = useState("");
  const [presets, setPresets] = useState<IconPreset[]>(["favicon", "pwa"]);
  const [padding, setPadding] = useState(0);
  const [background, setBackground] = useState<string | null>(null);
  const [appName, setAppName] = useState("");
  const [result, setResult] = useState<Generated | null>(null);
  const [busy, run] = useRun();

  return (
    <>
      <SourceFields source={source} folder={folder} onSource={setSource} onFolder={setFolder} />
      <Field label={t("tools.icons.presets")}>
        {ICON_PRESETS.map(({ value, label }) => (
          <Checkbox
            key={value}
            checked={presets.includes(value)}
            onChange={(on) => setPresets(on ? [...presets, value] : presets.filter((item) => item !== value))}
            label={label}
          />
        ))}
      </Field>
      <Field label={t("tools.icons.padding")}>
        <Slider value={padding} onChange={setPadding} min={0} max={40} reset={0} />
      </Field>
      <Field label={t("op.newBackground")}>
        <Checkbox
          checked={background === null}
          onChange={(transparent) => setBackground(transparent ? null : "#ffffff")}
          label={t("op.transparent")}
        />
        {background === null ? null : <ColorInput value={background} onChange={setBackground} />}
      </Field>
      <Field label={t("tools.icons.appName")}>
        <TextInput value={appName} onChange={setAppName} />
      </Field>
      <RunButton
        busy={busy}
        disabled={source === "" || folder === "" || presets.length === 0}
        onClick={() =>
          run(async () => setResult(await api.iconsGenerate({ source, folder, presets, padding, background, appName })))
        }
      >
        {t("tools.create")}
      </RunButton>
      {result ? <GeneratedView result={result} /> : null}
    </>
  );
}

function SocialTool() {
  const { t } = useT();
  const { showError } = useApp();
  const [source, setSource] = useState("");
  const [folder, setFolder] = useState("");
  const [sizes, setSizes] = useState<SocialSize[]>([]);
  const [ids, setIds] = useState<string[]>([]);
  const [quality, setQuality] = useState(85);
  const [result, setResult] = useState<Generated | null>(null);
  const [busy, run] = useRun();

  useEffect(() => {
    api
      .socialSizes()
      .then((list) => {
        setSizes(list);
        setIds(list.map((size) => size.id));
      })
      .catch(showError);
  }, [showError]);

  return (
    <>
      <SourceFields source={source} folder={folder} onSource={setSource} onFolder={setFolder} />
      <Field label={t("tools.social.sizes")}>
        <div className="check-grid">
          {sizes.map((size) => (
            <Checkbox
              key={size.id}
              checked={ids.includes(size.id)}
              onChange={(on) => setIds(on ? [...ids, size.id] : ids.filter((id) => id !== size.id))}
              label={`${SOCIAL_NAMES[size.id] ?? size.id} · ${size.width}×${size.height}`}
            />
          ))}
        </div>
      </Field>
      <Field label={t("output.quality")}>
        <Slider value={quality} onChange={setQuality} min={1} max={100} reset={85} />
      </Field>
      <RunButton
        busy={busy}
        disabled={source === "" || folder === "" || ids.length === 0}
        onClick={() => run(async () => setResult(await api.socialGenerate(source, folder, ids, quality)))}
      >
        {t("tools.create")}
      </RunButton>
      {result ? <GeneratedView result={result} /> : null}
    </>
  );
}

function WebKitTool() {
  const { t } = useT();
  const [source, setSource] = useState("");
  const [folder, setFolder] = useState("");
  const [widths, setWidths] = useState("480, 768, 1080, 1440, 1920");
  const [formats, setFormats] = useState<ImageFormat[]>([...WEB_FORMATS]);
  const [quality, setQuality] = useState(78);
  const [alt, setAlt] = useState("");
  const [sizes, setSizes] = useState("100vw");
  const [urlPrefix, setUrlPrefix] = useState("");
  const [result, setResult] = useState<Generated | null>(null);
  const [busy, run] = useRun();

  const parsedWidths = widths
    .split(/[\s,;]+/)
    .map(Number)
    .filter((width) => Number.isInteger(width) && width > 0 && width <= 16384);

  return (
    <>
      <SourceFields source={source} folder={folder} onSource={setSource} onFolder={setFolder} />
      <Field label={t("tools.webKit.widths")} hint={t("tools.webKit.widthsHint")}>
        <TextInput value={widths} onChange={setWidths} wide />
      </Field>
      <Field label={t("tools.webKit.formats")} hint={t("tools.webKit.formatsHint")}>
        {WEB_FORMATS.map((format) => (
          <Checkbox
            key={format}
            checked={formats.includes(format)}
            onChange={(on) =>
              setFormats(WEB_FORMATS.filter((item) => (item === format ? on : formats.includes(item))))
            }
            label={FORMAT_NAMES[format]}
          />
        ))}
      </Field>
      <Field label={t("output.quality")}>
        <Slider value={quality} onChange={setQuality} min={1} max={100} reset={78} />
      </Field>
      <Field label={t("tools.webKit.alt")}>
        <TextInput value={alt} onChange={setAlt} wide />
      </Field>
      <Field label="sizes">
        <TextInput value={sizes} onChange={setSizes} wide />
      </Field>
      <Field label={t("tools.webKit.urlPrefix")}>
        <TextInput value={urlPrefix} onChange={setUrlPrefix} placeholder="/images/" />
      </Field>
      <RunButton
        busy={busy}
        disabled={source === "" || folder === "" || formats.length === 0 || parsedWidths.length === 0}
        onClick={() =>
          run(async () =>
            setResult(
              await api.webKitGenerate({ source, folder, widths: parsedWidths, formats, quality, alt, sizes, urlPrefix }),
            ),
          )
        }
      >
        {t("tools.create")}
      </RunButton>
      {result ? <GeneratedView result={result} /> : null}
    </>
  );
}

/** Asks where to save, runs the tool and tells where the file is. */
function useSaveTo(): (suggested: string, kind: string, extensions: string[], write: (output: string) => Promise<string>) => Promise<void> {
  const { t } = useT();
  const { notify } = useApp();
  return async (suggested, kind, extensions, write) => {
    const output = await api.pickSave(suggested, kind, extensions);
    if (output) {
      const written = await write(output);
      notify(t("tools.saved", { name: fileName(written) }));
      await api.revealPath(written);
    }
  };
}

function CollageTool() {
  const { t } = useT();
  const [files, setFiles] = useState<string[]>([]);
  const [spec, setSpec] = useState<CollageSpec>({
    columns: 0,
    cellWidth: 512,
    cellHeight: 512,
    gap: 16,
    background: "#ffffff",
    fill: true,
  });
  const [quality, setQuality] = useState(88);
  const [busy, run] = useRun();
  const saveTo = useSaveTo();
  const set = (patch: Partial<CollageSpec>) => setSpec({ ...spec, ...patch });

  return (
    <>
      <FilesField files={files} onChange={setFiles} />
      <Field label={t("tools.collage.columns")} hint={t("tools.collage.columnsHint")}>
        <NumberInput value={spec.columns} onChange={(columns) => set({ columns })} min={0} max={64} />
      </Field>
      <Field label={t("tools.collage.cell")}>
        <NumberInput value={spec.cellWidth} onChange={(cellWidth) => set({ cellWidth })} min={16} max={4096} suffix="×" wide />
        <NumberInput value={spec.cellHeight} onChange={(cellHeight) => set({ cellHeight })} min={16} max={4096} suffix="px" wide />
      </Field>
      <Field label={t("tools.collage.gap")}>
        <NumberInput value={spec.gap} onChange={(gap) => set({ gap })} min={0} max={512} suffix="px" />
        <ColorInput value={spec.background} onChange={(background) => set({ background })} />
      </Field>
      <Field label="">
        <Checkbox checked={spec.fill} onChange={(fill) => set({ fill })} label={t("tools.collage.fill")} />
      </Field>
      <Field label={t("output.quality")}>
        <Slider value={quality} onChange={setQuality} min={1} max={100} reset={88} />
      </Field>
      <RunButton
        busy={busy}
        disabled={files.length === 0}
        onClick={() =>
          run(() =>
            saveTo("collage.jpg", t("native.dialog.pictures"), ["jpg", "png", "webp", "avif"], (output) =>
              api.collageCreate(files, spec, output, quality),
            ),
          )
        }
      >
        {t("tools.createAndSave")}
      </RunButton>
    </>
  );
}

function PdfTool() {
  const { t } = useT();
  const [files, setFiles] = useState<string[]>([]);
  const [spec, setSpec] = useState<PdfSpec>({ dpi: 150, quality: 88, title: "" });
  const [busy, run] = useRun();
  const saveTo = useSaveTo();

  return (
    <>
      <FilesField files={files} onChange={setFiles} />
      <Field label={t("tools.pdf.dpi")} hint={t("tools.pdf.dpiHint")}>
        <NumberInput value={spec.dpi} onChange={(dpi) => setSpec({ ...spec, dpi })} min={36} max={1200} suffix="dpi" wide />
      </Field>
      <Field label={t("output.quality")}>
        <Slider value={spec.quality} onChange={(quality) => setSpec({ ...spec, quality })} min={1} max={100} reset={88} />
      </Field>
      <Field label={t("tools.pdf.title")}>
        <TextInput value={spec.title} onChange={(title) => setSpec({ ...spec, title })} wide />
      </Field>
      <RunButton
        busy={busy}
        disabled={files.length === 0}
        onClick={() => run(() => saveTo("pictures.pdf", "PDF", ["pdf"], (output) => api.pdfCreate(files, spec, output)))}
      >
        {t("tools.createAndSave")}
      </RunButton>
    </>
  );
}

function AnimationTool() {
  const { t } = useT();
  const [files, setFiles] = useState<string[]>([]);
  const [spec, setSpec] = useState<AnimationSpec>({ delayMs: 100, loopCount: 0, width: 0, height: 0 });
  const [quality, setQuality] = useState(80);
  const [busy, run] = useRun();
  const saveTo = useSaveTo();
  const set = (patch: Partial<AnimationSpec>) => setSpec({ ...spec, ...patch });

  return (
    <>
      <FilesField files={files} onChange={setFiles} />
      <Field label={t("tools.animation.delay")}>
        <NumberInput value={spec.delayMs} onChange={(delayMs) => set({ delayMs })} min={10} max={60000} step={10} suffix="ms" wide />
      </Field>
      <Field label={t("tools.animation.loops")} hint={t("tools.animation.loopsHint")}>
        <NumberInput value={spec.loopCount} onChange={(loopCount) => set({ loopCount })} min={0} max={65535} />
      </Field>
      <Field label={t("op.size")} hint={t("tools.animation.sizeHint")}>
        <NumberInput value={spec.width} onChange={(width) => set({ width })} min={0} max={8192} suffix="×" wide />
        <NumberInput value={spec.height} onChange={(height) => set({ height })} min={0} max={8192} suffix="px" wide />
      </Field>
      <Field label={t("output.quality")}>
        <Slider value={quality} onChange={setQuality} min={1} max={100} reset={80} />
      </Field>
      <RunButton
        busy={busy}
        disabled={files.length < 2}
        onClick={() =>
          run(() =>
            saveTo("animation.webp", t("tools.animation.fileKind"), ["webp", "gif"], (output) =>
              api.animationCreate(files, spec, output, quality),
            ),
          )
        }
      >
        {t("tools.createAndSave")}
      </RunButton>
    </>
  );
}

function DuplicatesTool() {
  const { t } = useT();
  const { settings, jobResult, confirm, notify, showError } = useApp();
  const [folder, setFolder] = useState(settings.recentFolders[0] ?? "");
  const [recursive, setRecursive] = useState(true);
  const [tolerance, setTolerance] = useState(6);
  const [groups, setGroups] = useState<DuplicateGroup[] | null>(null);
  const [marked, setMarked] = useState<string[]>([]);
  const [busy, run] = useRun();

  const search = () =>
    run(async () => {
      setGroups(null);
      const found = await jobResult<DuplicateGroup[]>(await api.duplicatesFind(folder, recursive, tolerance));
      setGroups(found);
      // Everything but the first file of a group (the largest picture) is offered for removal.
      setMarked(found.flatMap((group) => group.files.slice(1).map((file) => file.path)));
    });

  const trash = async () => {
    const agreed = await confirm({
      title: t("viewer.trashTitle"),
      text: t("viewer.trashText", { count: marked.length }),
      confirmLabel: t("common.delete"),
      danger: true,
    });
    if (!agreed) {
      return;
    }
    try {
      const moved = await api.filesTrash(marked);
      notify(t("tools.duplicates.trashed", { count: moved }));
    } catch (error) {
      showError(error);
    }
    setGroups(
      (groups ?? [])
        .map((group) => ({ ...group, files: group.files.filter((file) => !marked.includes(file.path)) }))
        .filter((group) => group.files.length > 1),
    );
    setMarked([]);
  };

  return (
    <>
      <Field label={t("watch.folder")}>
        <PathField kind="folder" value={folder} onChange={setFolder} />
      </Field>
      <Field label="">
        <Checkbox checked={recursive} onChange={setRecursive} label={t("batch.recursive")} />
      </Field>
      <Field label={t("tools.duplicates.tolerance")} hint={t("tools.duplicates.toleranceHint")}>
        <Slider value={tolerance} onChange={setTolerance} min={0} max={MAX_TOLERANCE} reset={6} />
      </Field>
      <RunButton busy={busy} disabled={folder === ""} onClick={search}>
        {t("tools.duplicates.search")}
      </RunButton>
      {groups === null ? null : groups.length === 0 ? (
        <EmptyState title={t("tools.duplicates.none")} />
      ) : (
        <Section
          title={t("tools.duplicates.groups", { count: groups.length })}
          actions={
            <Button small variant="danger" disabled={marked.length === 0} onClick={() => void trash()}>
              {t("tools.duplicates.trash", { count: marked.length })}
            </Button>
          }
        >
          {groups.map((group, index) => (
            <div key={index} className="duplicate-group">
              <Tag tone={group.exact ? "warning" : "plain"}>
                {group.exact ? t("tools.duplicates.exact") : t("tools.duplicates.similar")}
              </Tag>
              <div className="duplicate-files">
                {group.files.map((file) => (
                  <div key={file.path} className="duplicate-file" title={file.path}>
                    {/* The size of the file stands in for its change time in the address. */}
                    <img src={thumbUrl({ path: file.path, modified: file.bytes }, 256)} alt="" loading="lazy" draggable={false} />
                    <Checkbox
                      checked={marked.includes(file.path)}
                      onChange={(on) =>
                        setMarked(on ? [...marked, file.path] : marked.filter((path) => path !== file.path))
                      }
                      label={fileName(file.path)}
                    />
                    <span className="duplicate-facts">
                      {file.width}×{file.height} · {formatBytes(file.bytes)}
                    </span>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </Section>
      )}
    </>
  );
}

function CaptureTool() {
  const { t } = useT();
  const { settings, info, openInEditor, navigate } = useApp();
  const [busy, run] = useRun();
  return (
    <>
      <Field label={t("settings.captureHotkey")}>
        <span>{settings.captureHotkey || t("settings.hotkeyNone")}</span>
        <Button small onClick={() => navigate("settings")}>
          {t("common.change")}
        </Button>
      </Field>
      <Field label={t("tools.capture.folder")}>
        <span>{joinPath(info.dataDir, "captures")}</span>
        <Button small onClick={() => void api.revealPath(joinPath(info.dataDir, "captures")).catch(() => undefined)}>
          {t("batch.openFolder")}
        </Button>
      </Field>
      <RunButton busy={busy} onClick={() => run(async () => openInEditor(await api.captureScreen()))}>
        {t("viewer.capture")}
      </RunButton>
    </>
  );
}

function ToolView({ tool }: { tool: Tool }) {
  switch (tool) {
    case "icons":
      return <IconsTool />;
    case "social":
      return <SocialTool />;
    case "webKit":
      return <WebKitTool />;
    case "collage":
      return <CollageTool />;
    case "pdf":
      return <PdfTool />;
    case "animation":
      return <AnimationTool />;
    case "duplicates":
      return <DuplicatesTool />;
    case "capture":
      return <CaptureTool />;
    default: {
      const unreachable: never = tool;
      return unreachable;
    }
  }
}

export default function ToolsPage() {
  const { t, td } = useT();
  const [tool, setTool] = useState<Tool>("icons");
  return (
    <PageFrame title={t("nav.tools")}>
      <Tabs value={tool} options={TOOLS.map((value) => ({ value, label: td(`tools.tab.${value}`) }))} onChange={setTool} />
      {/* Every tool stays mounted, so its form survives a look at another tab. */}
      {TOOLS.map((value) => (
        <div key={value} className="tool" hidden={value !== tool}>
          <p className="tool-text">{td(`tools.about.${value}`)}</p>
          <ToolView tool={value} />
        </div>
      ))}
    </PageFrame>
  );
}
