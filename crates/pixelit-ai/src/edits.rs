//! Pictures the generator drew for the editor.
//!
//! A generator gives another picture every time it is asked, so the result
//! the editor has shown on its reduced copy is kept and used again when the
//! same change is exported from the full file.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use pixelit_core::pipeline::{AiEdit, ResizeFilter};
use pixelit_core::{ops, Image, Result};

/// Results kept; older ones are dropped.
const KEPT: usize = 4;

/// Side of the reduced picture two requests are compared by.
const SIDE: u32 = 32;

/// Largest mean difference of the reduced pictures, in levels of 255, that
/// still counts as the same picture: the reduced copy of the editor and the
/// full file differ by less, a visible change by more.
const SAME_PICTURE: f64 = 3.0;

/// Largest relative difference of the shapes of two pictures.
const SAME_SHAPE: f32 = 0.02;

/// What tells one request to change a picture from another.
#[derive(Clone, Debug, PartialEq)]
pub struct Print {
    /// The prompt, the strength, the attempt and the area: equal text means
    /// an equal request.
    request: String,
    /// Width over height.
    shape: f32,
    /// The picture reduced to `SIDE` x `SIDE`, three bytes a pixel.
    picture: Vec<u8>,
}

impl Print {
    pub fn of(image: &Image, spec: &AiEdit) -> Result<Self> {
        let picture =
            ops::resize::resize_to(image, SIDE, SIDE, ResizeFilter::Box, false, None)?.to_rgb8();
        Ok(Self {
            request: format!(
                "{}\n{}\n{}\n{}",
                spec.prompt.trim(),
                spec.strength,
                spec.variant,
                serde_json::to_string(&spec.strokes).unwrap_or_default()
            ),
            shape: image.width() as f32 / image.height().max(1) as f32,
            picture,
        })
    }

    fn matches(&self, other: &Self) -> bool {
        if self.request != other.request
            || self.picture.len() != other.picture.len()
            || (self.shape - other.shape).abs() > SAME_SHAPE * self.shape
        {
            return false;
        }
        let difference: u64 = self
            .picture
            .iter()
            .zip(&other.picture)
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        difference as f64 <= SAME_PICTURE * self.picture.len() as f64
    }
}

#[derive(Default)]
pub struct Edits {
    kept: Mutex<VecDeque<(Print, Arc<Image>)>>,
}

impl Edits {
    /// The picture drawn for the same request, when there is one.
    pub fn find(&self, print: &Print) -> Option<Arc<Image>> {
        self.kept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(known, _)| known.matches(print))
            .map(|(_, generated)| Arc::clone(generated))
    }

    pub fn remember(&self, print: Print, generated: Image) {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        if kept.len() >= KEPT {
            kept.pop_front();
        }
        kept.push_back((print, Arc::new(generated)));
    }
}

#[cfg(test)]
mod tests {
    use pixelit_core::pipeline::Stroke;
    use pixelit_core::{Color, Depth};

    use super::*;

    fn edit(prompt: &str) -> AiEdit {
        AiEdit {
            prompt: prompt.to_owned(),
            ..AiEdit::default()
        }
    }

    #[test]
    fn the_full_file_matches_its_reduced_copy() {
        let full = Image::new(800, 400, Depth::Eight, Color::rgb(90, 120, 30)).unwrap();
        let reduced = Image::new(200, 100, Depth::Eight, Color::rgb(91, 119, 30)).unwrap();
        let spec = edit("make it night");
        let shown = Print::of(&reduced, &spec).unwrap();
        assert!(shown.matches(&Print::of(&full, &spec).unwrap()));

        let changed = Image::new(800, 400, Depth::Eight, Color::rgb(120, 120, 30)).unwrap();
        assert!(!shown.matches(&Print::of(&changed, &spec).unwrap()));
        let square = Image::new(400, 400, Depth::Eight, Color::rgb(90, 120, 30)).unwrap();
        assert!(!shown.matches(&Print::of(&square, &spec).unwrap()));
    }

    #[test]
    fn another_request_is_another_print() {
        let image = Image::new(64, 64, Depth::Eight, Color::WHITE).unwrap();
        let first = Print::of(&image, &edit("a hat")).unwrap();
        assert!(!first.matches(&Print::of(&image, &edit("a scarf")).unwrap()));
        let mut again = edit("a hat");
        again.variant = 1;
        assert!(!first.matches(&Print::of(&image, &again).unwrap()));
        let mut area = edit("a hat");
        area.strokes.push(Stroke {
            points: vec![[0.1, 0.1], [0.2, 0.2]],
            width: 4.0,
        });
        assert!(!first.matches(&Print::of(&image, &area).unwrap()));
        assert!(first.matches(&Print::of(&image, &edit(" a hat ")).unwrap()));
    }

    #[test]
    fn only_the_latest_results_are_kept() {
        let edits = Edits::default();
        let image = Image::new(8, 8, Depth::Eight, Color::WHITE).unwrap();
        for index in 0..=KEPT {
            let print = Print::of(&image, &edit(&format!("prompt {index}"))).unwrap();
            edits.remember(print, image.clone());
        }
        let oldest = Print::of(&image, &edit("prompt 0")).unwrap();
        assert!(edits.find(&oldest).is_none());
        let latest = Print::of(&image, &edit(&format!("prompt {KEPT}"))).unwrap();
        assert!(edits.find(&latest).is_some());
    }
}
