//! The `parameters` text other generators write into a PNG.
//!
//! The shape is the one AUTOMATIC1111, Forge and Civitai share: the prompt,
//! then `Negative prompt:`, then one line of `Name: value` pairs.

use crate::config::GenerationProvider;

/// What a `parameters` block says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parameters {
    pub prompt: String,
    pub negative: String,
    pub steps: u32,
    pub cfg: f32,
    pub seed: u64,
    pub width: u32,
    pub height: u32,
    pub model: String,
}

/// The text to store in the file.
pub fn write(parameters: &Parameters) -> String {
    let mut text = parameters.prompt.trim().to_owned();
    if !parameters.negative.trim().is_empty() {
        text.push_str("\nNegative prompt: ");
        text.push_str(parameters.negative.trim());
    }
    let mut tail = format!(
        "Steps: {}, Sampler: Euler, CFG scale: {}, Seed: {}, Size: {}x{}",
        parameters.steps.max(1),
        trim_number(parameters.cfg),
        parameters.seed,
        parameters.width,
        parameters.height,
    );
    if !parameters.model.trim().is_empty() {
        tail.push_str(", Model: ");
        tail.push_str(parameters.model.trim());
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&tail);
    text
}

fn trim_number(value: f32) -> String {
    let text = format!("{value:.4}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Reads a `parameters` block. A block without the size line is refused,
/// so ordinary captions are not taken for generation settings.
pub fn read(text: &str) -> Option<Parameters> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (head, tail) = split_tail(text)?;
    let size = value_of(tail, "Size")?;
    let (width, height) = size.split_once('x').or_else(|| size.split_once('×'))?;
    let width = width.trim().parse().ok()?;
    let height = height.trim().parse().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let (prompt, negative) = split_negative(head);
    Some(Parameters {
        prompt,
        negative,
        steps: value_of(tail, "Steps")
            .and_then(|text| text.parse().ok())
            .unwrap_or(0),
        cfg: value_of(tail, "CFG scale")
            .and_then(|text| text.parse().ok())
            .unwrap_or(0.0),
        seed: value_of(tail, "Seed")
            .and_then(|text| text.parse().ok())
            .unwrap_or(0),
        width,
        height,
        model: value_of(tail, "Model").unwrap_or_default().to_owned(),
    })
}

/// The provider a model line came from, when the line says so. PixelIt writes
/// the bare model name, so this stays `None` for its own files.
pub fn provider_of(model: &str) -> GenerationProvider {
    let _ = model;
    GenerationProvider::None
}

fn split_negative(head: &str) -> (String, String) {
    match head.split_once("\nNegative prompt:") {
        Some((prompt, negative)) => (prompt.trim().to_owned(), negative.trim().to_owned()),
        None => (head.trim().to_owned(), String::new()),
    }
}

/// The settings line is the last line that contains `Steps:`.
fn split_tail(text: &str) -> Option<(&str, &str)> {
    let start = text.rfind("\nSteps:")?;
    Some((text[..start].trim(), text[start + 1..].trim()))
}

fn value_of<'a>(tail: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("{name}: ");
    let rest = tail.split(&marker).nth(1)?;
    let end = rest.find(", ").unwrap_or(rest.len());
    let value = rest[..end].trim();
    (!value.is_empty()).then_some(value)
}

/// Keeps about the same area and moves the sides to a ratio, on a grid of 64.
pub fn fit_ratio(width: u32, height: u32, ratio_w: u32, ratio_h: u32) -> (u32, u32) {
    let ratio_w = ratio_w.max(1);
    let ratio_h = ratio_h.max(1);
    let area = f64::from(width.clamp(64, 4096)) * f64::from(height.clamp(64, 4096));
    let ratio = f64::from(ratio_w) / f64::from(ratio_h);
    let height = (area / ratio).sqrt();
    let width = height * ratio;
    (snap(width), snap(height))
}

fn snap(value: f64) -> u32 {
    let snapped = (value / 64.0).round().max(1.0) as u32 * 64;
    snapped.clamp(64, 4096)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Parameters {
        Parameters {
            prompt: String::from("a \"red\" fox"),
            negative: String::from("blur, text"),
            steps: 24,
            cfg: 6.5,
            seed: 42,
            width: 1216,
            height: 832,
            model: String::from("dream.safetensors"),
        }
    }

    #[test]
    fn a_block_round_trips() {
        let text = write(&sample());
        let back = read(&text).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn a_block_without_a_negative_prompt_is_read() {
        let text =
            "a fox\nSteps: 20, Sampler: Euler, CFG scale: 7, Seed: 1, Size: 512x768, Model: sdxl";
        let back = read(text).unwrap();
        assert_eq!(back.prompt, "a fox");
        assert!(back.negative.is_empty());
        assert_eq!((back.width, back.height), (512, 768));
        assert_eq!(back.model, "sdxl");
    }

    #[test]
    fn a_caption_is_not_parameters() {
        assert!(read("a fox in a field").is_none());
        assert!(read("Steps: 20").is_none());
    }

    #[test]
    fn a_ratio_keeps_the_area_on_the_grid() {
        let (width, height) = fit_ratio(1024, 1024, 16, 9);
        assert_eq!((width % 64, height % 64), (0, 0));
        let area = f64::from(width) * f64::from(height);
        assert!((area - 1024.0 * 1024.0).abs() / (1024.0 * 1024.0) < 0.15);
        assert!(width > height);
        assert_eq!(fit_ratio(1024, 1024, 1, 1), (1024, 1024));
    }
}
