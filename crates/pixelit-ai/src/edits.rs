//! Pictures the generator drew for the editor.
//!
//! A generator gives another picture every time it is asked, so the result
//! the editor has shown on its reduced copy is kept and used again when the
//! same change is exported from the full file.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use pixelit_core::pipeline::{AiDetail, AiEdit, ResizeFilter};
use pixelit_core::{ops, Image, Result};

/// A step that asks the generator to draw a picture again.
#[derive(Clone, Copy, Debug)]
pub enum Redraw<'a> {
    /// A change described in words.
    Edit(&'a AiEdit),
    /// Fine detail painted into a picture that keeps what it shows.
    Detail(&'a AiDetail),
}

impl Redraw<'_> {
    /// Everything of the step that decides what the generator draws: equal
    /// text means an equal request.
    fn text(self) -> String {
        match self {
            Self::Edit(spec) => format!(
                "edit\n{}\n{}\n{}\n{}\n{}\n{}",
                spec.prompt.trim(),
                spec.strength,
                spec.variant,
                serde_json::to_string(&spec.strokes).unwrap_or_default(),
                spec.provider,
                spec.model,
            ),
            Self::Detail(spec) => format!(
                "detail\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
                spec.prompt.trim(),
                spec.strength,
                spec.variant,
                serde_json::to_string(&spec.strokes).unwrap_or_default(),
                spec.provider,
                spec.model,
                spec.negative.trim(),
                spec.steps,
                spec.cfg,
                serde_json::to_string(&spec.tuning.tidy()).unwrap_or_default(),
            ),
        }
    }
}

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

/// What tells one request to draw a picture again from another.
#[derive(Clone, Debug, PartialEq)]
pub struct Print {
    /// What the step asks for: equal text means an equal request.
    request: String,
    /// Width over height.
    shape: f32,
    /// The picture reduced to `SIDE` x `SIDE`, three bytes a pixel.
    picture: Vec<u8>,
}

impl Print {
    pub fn of(image: &Image, step: Redraw<'_>) -> Result<Self> {
        let picture =
            ops::resize::resize_to(image, SIDE, SIDE, ResizeFilter::Box, false, None)?.to_rgb8();
        Ok(Self {
            request: step.text(),
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

    fn print(image: &Image, spec: &AiEdit) -> Print {
        Print::of(image, Redraw::Edit(spec)).unwrap()
    }

    fn detail_print(image: &Image, spec: &AiDetail) -> Print {
        Print::of(image, Redraw::Detail(spec)).unwrap()
    }

    #[test]
    fn the_full_file_matches_its_reduced_copy() {
        let full = Image::new(800, 400, Depth::Eight, Color::rgb(90, 120, 30)).unwrap();
        let reduced = Image::new(200, 100, Depth::Eight, Color::rgb(91, 119, 30)).unwrap();
        let spec = edit("make it night");
        let shown = print(&reduced, &spec);
        assert!(shown.matches(&print(&full, &spec)));

        let changed = Image::new(800, 400, Depth::Eight, Color::rgb(120, 120, 30)).unwrap();
        assert!(!shown.matches(&print(&changed, &spec)));
        let square = Image::new(400, 400, Depth::Eight, Color::rgb(90, 120, 30)).unwrap();
        assert!(!shown.matches(&print(&square, &spec)));
    }

    #[test]
    fn another_request_is_another_print() {
        let image = Image::new(64, 64, Depth::Eight, Color::WHITE).unwrap();
        let first = print(&image, &edit("a hat"));
        assert!(!first.matches(&print(&image, &edit("a scarf"))));
        let mut again = edit("a hat");
        again.variant = 1;
        assert!(!first.matches(&print(&image, &again)));
        let mut area = edit("a hat");
        area.strokes.push(Stroke {
            points: vec![[0.1, 0.1], [0.2, 0.2]],
            width: 4.0,
        });
        assert!(!first.matches(&print(&image, &area)));
        assert!(first.matches(&print(&image, &edit(" a hat "))));
    }

    #[test]
    fn a_detail_pass_is_told_from_an_edit_and_by_its_settings() {
        let image = Image::new(64, 64, Depth::Eight, Color::WHITE).unwrap();
        // The same words and the same strength as the edit.
        let detail = AiDetail {
            prompt: String::from("a hat"),
            strength: AiEdit::default().strength,
            ..AiDetail::default()
        };
        let first = detail_print(&image, &detail);
        assert!(first.matches(&detail_print(&image, &detail)));
        assert!(!first.matches(&print(&image, &edit("a hat"))));

        // What the generator is set to is part of the request as well.
        let mut tuned = detail.clone();
        tuned.tuning.sampler = String::from("dpmpp_2m");
        assert!(!first.matches(&detail_print(&image, &tuned)));
        let mut avoided = detail.clone();
        avoided.negative = String::from("blur");
        assert!(!first.matches(&detail_print(&image, &avoided)));
        let mut longer = detail;
        longer.steps = 40;
        assert!(!first.matches(&detail_print(&image, &longer)));
    }

    #[test]
    fn only_the_latest_results_are_kept() {
        let edits = Edits::default();
        let image = Image::new(8, 8, Depth::Eight, Color::WHITE).unwrap();
        for index in 0..=KEPT {
            edits.remember(
                print(&image, &edit(&format!("prompt {index}"))),
                image.clone(),
            );
        }
        let oldest = print(&image, &edit("prompt 0"));
        assert!(edits.find(&oldest).is_none());
        let latest = print(&image, &edit(&format!("prompt {KEPT}")));
        assert!(edits.find(&latest).is_some());
    }
}
