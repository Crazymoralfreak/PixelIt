//! Running a pipeline on one file: decode, apply the steps, encode, write.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pixelit_codecs::{decode_file, DecodeOptions, Decoded};
use pixelit_core::image::{Animation, Frame};
use pixelit_core::naming::{self, NameParts};
use pixelit_core::pipeline::{AiBackend, Conflict, Destination, OutputSpec, Step};
use pixelit_core::{
    ApplyContext, CancelToken, Error, ImageFormat, Limits, Media, Pipeline, Result,
};
use serde::{Deserialize, Serialize};

use crate::clock::LocalTime;
use crate::compress::{self, Encoded};
use crate::fonts::FileResources;
use crate::fsops::{self, UndoRecord, Written};
use crate::paths::Paths;

/// A neural backend shared between threads.
pub type SharedAi = Arc<dyn AiBackend + Send + Sync>;

/// Everything needed to run pipelines.
#[derive(Clone)]
pub struct Runner {
    pub resources: Arc<FileResources>,
    pub ai: Option<SharedAi>,
    pub limits: Limits,
    /// Folder of the PDFium library.
    pub pdfium: Option<PathBuf>,
    /// Data folder: on macOS a replaced original is kept here for undo.
    pub paths: Option<Paths>,
}

impl Default for Runner {
    fn default() -> Self {
        Self {
            resources: Arc::new(FileResources::new()),
            ai: None,
            limits: Limits::default(),
            pdfium: None,
            paths: None,
        }
    }
}

/// One file of a run.
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub source: PathBuf,
    /// Folder the user picked; the part of the source path below it is
    /// repeated under the destination when the structure is kept.
    pub root: Option<PathBuf>,
    /// Position in the batch, starting at 1.
    pub index: u64,
    /// File name suggested by a vision model.
    pub ai_name: String,
    /// Zero-based page of a multi-page document.
    pub page: u32,
}

impl Item {
    pub fn new(source: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            index: 1,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutcomeStatus {
    Done,
    /// The output file exists and the conflict rule says to keep it.
    Skipped,
}

/// What happened to one file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub source: PathBuf,
    pub output: PathBuf,
    pub status: OutcomeStatus,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub source_bytes: u64,
    pub output_bytes: u64,
    pub quality: Option<u8>,
    pub score: Option<f32>,
    pub target_missed: bool,
    /// An existing file was replaced and went to the Recycle Bin.
    pub replaced: bool,
    /// The source file went to the Recycle Bin.
    pub source_trashed: bool,
    /// Where a replaced output original was kept for undo, when not in Trash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<PathBuf>,
    /// Where the removed source was kept for undo, when not in Trash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kept: Option<PathBuf>,
}

impl Outcome {
    /// Adds the disk changes of this file to an undo record.
    pub fn note(&self, record: &mut UndoRecord) {
        if self.status == OutcomeStatus::Skipped {
            return;
        }
        let written = if self.replaced {
            Written::Replaced {
                path: self.output.clone(),
                kept: self.kept.clone(),
            }
        } else {
            Written::Created(self.output.clone())
        };
        record.note(&written, self.output_bytes);
        if self.source_trashed {
            record.trashed.push(fsops::TrashedFile {
                path: self.source.clone(),
                kept: self.source_kept.clone(),
            });
        }
    }
}

fn text_of(path: Option<&std::ffi::OsStr>) -> String {
    path.map_or_default(|part| part.to_string_lossy().into_owned())
}

/// The folder a result goes to.
pub fn output_folder(spec: &OutputSpec, source: &Path, root: Option<&Path>) -> PathBuf {
    let parent = source.parent().unwrap_or_else(|| Path::new("."));
    let below_root = root
        .filter(|_| spec.keep_structure)
        .and_then(|root| parent.strip_prefix(root).ok());
    match &spec.destination {
        Destination::SameFolder => parent.to_path_buf(),
        Destination::Subfolder { name } => {
            let name = naming::sanitize(name);
            match (root, below_root) {
                (Some(root), Some(below)) => root.join(name).join(below),
                _ => parent.join(name),
            }
        }
        Destination::Folder { path } => {
            let folder = PathBuf::from(path);
            match below_root {
                Some(below) => folder.join(below),
                None => folder,
            }
        }
    }
}

/// The path a result gets before the conflict rule is applied.
pub fn output_path(
    spec: &OutputSpec,
    item: &Item,
    format: ImageFormat,
    width: u32,
    height: u32,
) -> PathBuf {
    let now = LocalTime::now();
    let name = text_of(item.source.file_stem());
    let ext = text_of(item.source.extension());
    let parent = text_of(item.source.parent().and_then(Path::file_name));
    let parts = NameParts {
        name: &name,
        ext: &ext,
        width,
        height,
        index: item.index,
        date: &now.date,
        time: &now.time,
        timestamp: now.timestamp,
        format: format.name(),
        parent: &parent,
        ai_name: &item.ai_name,
    };
    let file = format!(
        "{}.{}",
        naming::render(&spec.naming, &parts),
        format.extension()
    );
    output_folder(spec, &item.source, item.root.as_deref()).join(file)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

impl Runner {
    /// The context steps run in.
    pub fn context<'a>(&'a self, cancel: &'a CancelToken) -> ApplyContext<'a> {
        let mut context = ApplyContext::new(self.resources.as_ref(), cancel);
        context.ai = self.ai.as_deref().map(|ai| ai as &dyn AiBackend);
        context
    }

    pub fn decode_options(&self) -> DecodeOptions {
        DecodeOptions {
            limits: self.limits,
            pdfium: self.pdfium.clone(),
            ..DecodeOptions::default()
        }
    }

    /// Decodes a file at full quality.
    pub fn load(&self, source: &Path, page: u32, cancel: &CancelToken) -> Result<Decoded> {
        let options = DecodeOptions {
            page,
            ..self.decode_options()
        };
        decode_file(source, &options, cancel)
    }

    /// Applies steps to a still image or to every frame of an animation.
    pub fn apply(&self, media: Media, steps: &[Step], cancel: &CancelToken) -> Result<Media> {
        if !steps.iter().any(|step| step.enabled) {
            return Ok(media);
        }
        let context = self.context(cancel);
        match media {
            Media::Still(image) => Ok(Media::Still(pixelit_core::pipeline::apply_steps(
                image, steps, &context,
            )?)),
            Media::Animated(animation) => {
                let mut frames = Vec::with_capacity(animation.frames.len());
                for frame in animation.frames {
                    frames.push(Frame {
                        image: pixelit_core::pipeline::apply_steps(frame.image, steps, &context)?,
                        delay_ms: frame.delay_ms,
                    });
                }
                Ok(Media::Animated(Animation {
                    frames,
                    loop_count: animation.loop_count,
                }))
            }
        }
    }

    /// Decodes a file and applies the steps of a pipeline.
    pub fn render(
        &self,
        item: &Item,
        pipeline: &Pipeline,
        cancel: &CancelToken,
    ) -> Result<Decoded> {
        let mut decoded = self.load(&item.source, item.page, cancel)?;
        decoded.media = self
            .apply(decoded.media, &pipeline.steps, cancel)
            .map_err(|error| error.with("path", item.source.display()))?;
        Ok(decoded)
    }

    /// Runs a pipeline up to the encoded bytes, writing nothing.
    pub fn export(
        &self,
        item: &Item,
        pipeline: &Pipeline,
        cancel: &CancelToken,
    ) -> Result<Encoded> {
        let decoded = self.render(item, pipeline, cancel)?;
        compress::encode_output(
            &decoded.media,
            &pipeline.output,
            decoded.info.format,
            &decoded.metadata,
            cancel,
        )
        .map_err(|error| error.with("path", item.source.display()))
    }

    /// Runs a pipeline on one file and writes the result.
    pub fn process(
        &self,
        item: &Item,
        pipeline: &Pipeline,
        cancel: &CancelToken,
    ) -> Result<Outcome> {
        let spec = &pipeline.output;
        let source_bytes = fs::metadata(&item.source)
            .map_err(|error| Error::from(error).with("path", item.source.display()))?
            .len();
        let encoded = self.export(item, pipeline, cancel)?;
        cancel.check()?;
        let path = output_path(spec, item, encoded.format, encoded.width, encoded.height);
        let written =
            fsops::write_file_in(&path, &encoded.bytes, spec.conflict, self.paths.as_ref())?;
        let skipped = matches!(written, Written::Skipped(_));
        let kept = match &written {
            Written::Replaced { kept, .. } => kept.clone(),
            _ => None,
        };
        let mut source_trashed = false;
        let mut source_kept = None;
        if spec.replace_original && !skipped && !same_file(written.path(), &item.source) {
            let removed = fsops::remove_for_undo(&item.source, self.paths.as_ref())?;
            source_kept = removed.kept;
            source_trashed = true;
        }
        Ok(Outcome {
            source: item.source.clone(),
            output: written.path().to_path_buf(),
            status: if skipped {
                OutcomeStatus::Skipped
            } else {
                OutcomeStatus::Done
            },
            format: encoded.format,
            width: encoded.width,
            height: encoded.height,
            source_bytes,
            output_bytes: if skipped {
                0
            } else {
                encoded.bytes.len() as u64
            },
            quality: encoded.quality,
            score: encoded.score,
            target_missed: encoded.target_missed,
            replaced: matches!(written, Written::Replaced { .. }),
            source_trashed,
            kept,
            source_kept,
        })
    }
}

/// Whether a pipeline can replace or remove existing files, which callers
/// confirm with the user first.
pub fn is_destructive(pipeline: &Pipeline) -> bool {
    pipeline.output.conflict == Conflict::Overwrite || pipeline.output.replace_original
}

#[cfg(test)]
mod tests {
    use pixelit_codecs::{encode, EncodeOptions};
    use pixelit_core::pipeline::{Op, OutputFormat, Resize, ResizeMode};
    use pixelit_core::{Color, Depth, Image};

    use super::*;

    fn write_png(path: &Path, width: u32, height: u32) {
        let image = Image::new(width, height, Depth::Eight, Color::rgb(40, 120, 200)).unwrap();
        let bytes = encode(
            &image,
            &EncodeOptions::new(ImageFormat::Png),
            &CancelToken::new(),
        )
        .unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn resize_pipeline(width: u32) -> Pipeline {
        Pipeline {
            steps: vec![Step::new(
                "resize",
                Op::Resize(Resize {
                    mode: ResizeMode::Width,
                    width,
                    ..Resize::default()
                }),
            )],
            ..Pipeline::default()
        }
    }

    #[test]
    fn folders_follow_the_destination() {
        let source = Path::new("/photos/trip/day1/a.jpg");
        let root = Path::new("/photos/trip");
        let mut spec = OutputSpec::default();
        assert_eq!(
            output_folder(&spec, source, Some(root)),
            Path::new("/photos/trip/PixelIt/day1")
        );
        assert_eq!(
            output_folder(&spec, source, None),
            Path::new("/photos/trip/day1/PixelIt")
        );
        spec.keep_structure = false;
        assert_eq!(
            output_folder(&spec, source, Some(root)),
            Path::new("/photos/trip/day1/PixelIt")
        );
        spec.destination = Destination::SameFolder;
        assert_eq!(
            output_folder(&spec, source, Some(root)),
            Path::new("/photos/trip/day1")
        );
        spec.destination = Destination::Folder {
            path: "/out".into(),
        };
        assert_eq!(output_folder(&spec, source, Some(root)), Path::new("/out"));
        spec.keep_structure = true;
        assert_eq!(
            output_folder(&spec, source, Some(root)),
            Path::new("/out/day1")
        );
    }

    #[test]
    fn names_follow_the_template() {
        let spec = OutputSpec {
            naming: "{name}_{width}x{height}_{index:3}".into(),
            destination: Destination::SameFolder,
            ..OutputSpec::default()
        };
        let item = Item {
            index: 7,
            ..Item::new("/photos/a.png")
        };
        assert_eq!(
            output_path(&spec, &item, ImageFormat::Webp, 640, 480),
            Path::new("/photos/a_640x480_007.webp")
        );
    }

    #[test]
    fn processes_a_file_into_a_subfolder() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("a.png");
        write_png(&source, 64, 32);
        let mut pipeline = resize_pipeline(32);
        pipeline.output.format = OutputFormat::Jpeg;
        let runner = Runner::default();
        let outcome = runner
            .process(&Item::new(&source), &pipeline, &CancelToken::new())
            .unwrap();
        assert_eq!(outcome.status, OutcomeStatus::Done);
        assert_eq!(outcome.output, folder.path().join("PixelIt").join("a.jpg"));
        assert_eq!((outcome.width, outcome.height), (32, 16));
        assert!(outcome.output.is_file());
        assert!(source.is_file());
        assert_eq!(
            outcome.output_bytes,
            fs::metadata(&outcome.output).unwrap().len()
        );

        // A second run keeps the first result and adds a numbered one.
        let again = runner
            .process(&Item::new(&source), &pipeline, &CancelToken::new())
            .unwrap();
        assert_eq!(
            again.output,
            folder.path().join("PixelIt").join("a (2).jpg")
        );

        let mut record = UndoRecord::new("test");
        outcome.note(&mut record);
        again.note(&mut record);
        assert_eq!(record.created.len(), 2);
        assert!(record.trashed.is_empty());
    }

    #[test]
    fn skip_rule_keeps_existing_results() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("a.png");
        write_png(&source, 16, 16);
        let mut pipeline = Pipeline::default();
        pipeline.output.conflict = Conflict::Skip;
        let runner = Runner::default();
        let first = runner
            .process(&Item::new(&source), &pipeline, &CancelToken::new())
            .unwrap();
        let second = runner
            .process(&Item::new(&source), &pipeline, &CancelToken::new())
            .unwrap();
        assert_eq!(first.status, OutcomeStatus::Done);
        assert_eq!(second.status, OutcomeStatus::Skipped);
        assert_eq!(second.output, first.output);
    }

    #[test]
    fn cancelled_run_writes_nothing() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("a.png");
        write_png(&source, 16, 16);
        let cancel = CancelToken::new();
        cancel.cancel();
        let error = Runner::default()
            .process(&Item::new(&source), &Pipeline::default(), &cancel)
            .unwrap_err();
        assert!(error.is_cancelled());
        assert!(!folder.path().join("PixelIt").exists());
    }

    #[test]
    fn ai_steps_need_a_backend() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("a.png");
        write_png(&source, 16, 16);
        let pipeline = Pipeline {
            steps: vec![Step::new(
                "bg",
                Op::RemoveBackground(pixelit_core::pipeline::RemoveBackground::default()),
            )],
            ..Pipeline::default()
        };
        let error = Runner::default()
            .process(&Item::new(&source), &pipeline, &CancelToken::new())
            .unwrap_err();
        assert_eq!(error.code, "aiUnavailable");
        assert!(is_destructive(&Pipeline {
            output: OutputSpec {
                replace_original: true,
                ..OutputSpec::default()
            },
            ..Pipeline::default()
        }));
        assert!(!is_destructive(&pipeline));
    }
}
