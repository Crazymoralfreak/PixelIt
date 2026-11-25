//! Live preview of the editor.
//!
//! The file is decoded once and reduced to the size of the screen. Results of
//! the steps are kept, so moving a slider of the last step re-runs only that
//! step and never the whole stack.

use std::path::{Path, PathBuf};

use pixelit_codecs::SourceInfo;
use pixelit_core::pipeline::{apply_op, Step};
use pixelit_core::{CancelToken, Image, Result};
use serde::{Deserialize, Serialize};

use crate::process::Runner;
use crate::thumbs::load_preview;

/// Step results kept per session.
const CACHED_STEPS: usize = 12;

struct CachedStep {
    /// The step as JSON: equal text means equal result.
    key: String,
    image: Image,
}

/// An open picture of the editor.
pub struct PreviewSession {
    source: PathBuf,
    base: Image,
    /// Size of the preview relative to the full file.
    scale: f32,
    info: SourceInfo,
    cache: Vec<CachedStep>,
}

impl PreviewSession {
    /// Opens a file reduced to at most `max_side` on its long side.
    pub fn open(
        runner: &Runner,
        source: &Path,
        max_side: u32,
        page: u32,
        cancel: &CancelToken,
    ) -> Result<Self> {
        let preview = load_preview(runner, source, max_side, page, cancel)?;
        let full = preview.info.width.max(preview.info.height).max(1);
        let shown = preview.image.width().max(preview.image.height());
        Ok(Self {
            source: source.to_path_buf(),
            scale: (shown as f32 / full as f32).min(1.0),
            base: preview.image,
            info: preview.info,
            cache: Vec::new(),
        })
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn info(&self) -> &SourceInfo {
        &self.info
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The picture before any step.
    pub fn base(&self) -> &Image {
        &self.base
    }

    /// The picture after the enabled steps.
    pub fn render(
        &mut self,
        runner: &Runner,
        steps: &[Step],
        cancel: &CancelToken,
    ) -> Result<Image> {
        let keys: Vec<String> = steps
            .iter()
            .filter(|step| step.enabled)
            .map(|step| serde_json::to_string(&step.op).unwrap_or_default())
            .collect();
        let ops = steps
            .iter()
            .filter(|step| step.enabled)
            .map(|step| &step.op);
        let reused = self
            .cache
            .iter()
            .zip(&keys)
            .take_while(|(cached, key)| &cached.key == *key)
            .count();
        self.cache.truncate(reused);
        let mut image = match self.cache.last() {
            Some(cached) => cached.image.clone(),
            None => self.base.clone(),
        };
        let mut context = runner.context(cancel);
        context.scale = self.scale;
        context.preview = true;
        for (position, (op, key)) in ops.zip(keys).enumerate().skip(reused) {
            image = apply_op(image, op, &context).map_err(|error| error.with("step", op.kind()))?;
            if position < CACHED_STEPS {
                self.cache.push(CachedStep {
                    key,
                    image: image.clone(),
                });
            }
        }
        Ok(image)
    }
}

/// Brightness distribution of a picture: 256 counts for red, green, blue
/// and luma.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Histogram {
    pub red: Vec<u32>,
    pub green: Vec<u32>,
    pub blue: Vec<u32>,
    pub luma: Vec<u32>,
}

pub fn histogram(image: &Image) -> Histogram {
    let mut counts = [[0u32; 256]; 4];
    for pixel in image.to_rgb8().chunks_exact(3) {
        let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
        counts[0][usize::from(r)] += 1;
        counts[1][usize::from(g)] += 1;
        counts[2][usize::from(b)] += 1;
        let luma = (u32::from(r) * 2126 + u32::from(g) * 7152 + u32::from(b) * 722) / 10_000;
        counts[3][luma.min(255) as usize] += 1;
    }
    Histogram {
        red: counts[0].to_vec(),
        green: counts[1].to_vec(),
        blue: counts[2].to_vec(),
        luma: counts[3].to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pixelit_codecs::{encode, EncodeOptions};
    use pixelit_core::pipeline::{Adjust, Flip, Op, Resize, ResizeMode};
    use pixelit_core::{Color, Depth, ImageFormat};

    use super::*;

    #[test]
    fn session_reuses_step_results() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("a.png");
        let image = Image::new(400, 200, Depth::Eight, Color::rgb(90, 90, 90)).unwrap();
        let bytes = encode(
            &image,
            &EncodeOptions::new(ImageFormat::Png),
            &CancelToken::new(),
        )
        .unwrap();
        fs::write(&source, bytes).unwrap();

        let runner = Runner::default();
        let cancel = CancelToken::new();
        let mut session = PreviewSession::open(&runner, &source, 100, 0, &cancel).unwrap();
        assert_eq!((session.base().width(), session.base().height()), (100, 50));
        assert!((session.scale() - 0.25).abs() < 1e-6);

        let flip = Step::new(
            "flip",
            Op::Flip(Flip {
                horizontal: true,
                vertical: false,
            }),
        );
        let brighten = |amount: f32| {
            Step::new(
                "adjust",
                Op::Adjust(Adjust {
                    brightness: amount,
                    ..Adjust::default()
                }),
            )
        };
        let first = session
            .render(&runner, &[flip.clone(), brighten(20.0)], &cancel)
            .unwrap();
        assert_eq!(session.cache.len(), 2);
        let second = session
            .render(&runner, &[flip.clone(), brighten(60.0)], &cancel)
            .unwrap();
        assert_eq!(session.cache.len(), 2);
        assert!(second.pixel8(10, 10)[0] > first.pixel8(10, 10)[0]);

        // A resize to 200 pixels of the full file is 50 pixels of the preview.
        let resize = Step::new(
            "resize",
            Op::Resize(Resize {
                mode: ResizeMode::Width,
                width: 200,
                ..Resize::default()
            }),
        );
        let resized = session.render(&runner, &[resize], &cancel).unwrap();
        assert_eq!(resized.width(), 50);
        assert_eq!(session.cache.len(), 1);

        let mut disabled = flip;
        disabled.enabled = false;
        let untouched = session.render(&runner, &[disabled], &cancel).unwrap();
        assert_eq!(&untouched, session.base());
    }

    #[test]
    fn histogram_counts_pixels() {
        let image = Image::new(4, 4, Depth::Eight, Color::rgb(255, 0, 0)).unwrap();
        let counts = histogram(&image);
        assert_eq!(counts.red[255], 16);
        assert_eq!(counts.green[0], 16);
        assert_eq!(counts.luma[54], 16);
    }
}
