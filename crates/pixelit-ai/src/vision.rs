//! Questions about a picture for a vision model: description, alt text,
//! keywords, a file name, text recognition.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use pixelit_codecs::{encode, metadata, EncodeOptions};
use pixelit_core::pipeline::{Conflict, ResizeFilter};
use pixelit_core::{naming, ops, CancelToken, Color, Error, Image, ImageFormat, Result};
use pixelit_engine::fsops::{self, Written};

use crate::config::AiSettings;
use crate::llm::{self, Message, Role};
use crate::net::{self, Net};

/// Longest side of the picture a model receives.
const VISION_SIDE: u32 = 1024;

/// Text recognition needs more detail.
const OCR_SIDE: u32 = 1792;

const MAX_TAGS: usize = 16;
const MAX_NAME: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VisionTask {
    Describe,
    AltText,
    Tags,
    Name,
    Ocr,
}

/// English name of an interface language, as models understand it best.
pub fn language_name(code: &str) -> &str {
    match code.split(['-', '_']).next().unwrap_or(code) {
        "en" => "English",
        "ru" => "Russian",
        "de" => "German",
        "es" => "Spanish",
        "fr" => "French",
        "ja" => "Japanese",
        "ko" => "Korean",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "tr" => "Turkish",
        "zh" => "Chinese",
        _ => code,
    }
}

pub fn prompt(task: VisionTask, language: &str) -> String {
    let language = language_name(language);
    match task {
        VisionTask::Describe => format!(
            "Describe this image in two to four sentences. Answer in {language}. Output only the description."
        ),
        VisionTask::AltText => format!(
            "Write alt text for this image for a web page: one sentence of at most 125 characters, without the words \"image of\" or \"picture of\". Answer in {language}. Output only the alt text."
        ),
        VisionTask::Tags => format!(
            "List 5 to 12 keywords for this image, most important first, lower case, separated by commas. Answer in {language}. Output only the keywords."
        ),
        VisionTask::Name => format!(
            "Suggest a file name for this image: two to five words in {language}, lower case, joined with hyphens, without an extension. Output only the name."
        ),
        VisionTask::Ocr => String::from(
            "Transcribe all text visible in this image exactly as written, keeping the line breaks. Output only the text. If there is no text, output nothing.",
        ),
    }
}

/// Removes quotes and code fences a model may wrap its answer in.
fn unwrap_answer(text: &str) -> String {
    let mut text = text.trim();
    if let Some(inner) = text.strip_prefix("```") {
        let inner = inner.split_once('\n').map_or(inner, |(_, rest)| rest);
        text = inner.trim_end().trim_end_matches("```").trim();
    }
    for (open, close) in [('"', '"'), ('«', '»'), ('“', '”'), ('\'', '\''), ('`', '`')] {
        if text.len() >= 2 && text.starts_with(open) && text.ends_with(close) {
            text = text[open.len_utf8()..text.len() - close.len_utf8()].trim();
        }
    }
    text.to_owned()
}

/// Splits an answer into keywords.
pub fn tags(text: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for tag in text.split([',', ';', '\n']) {
        let tag = tag
            .trim()
            .trim_start_matches(|c: char| {
                c.is_ascii_digit() || matches!(c, '.' | ')' | '-' | '*' | '#' | ' ')
            })
            .trim_matches(|c: char| matches!(c, '"' | '\'' | '.' | ' '))
            .to_lowercase();
        if !tag.is_empty() && tag.chars().count() <= 40 && !seen.contains(&tag) {
            seen.push(tag);
        }
        if seen.len() == MAX_TAGS {
            break;
        }
    }
    seen
}

/// Turns an answer into a safe file name without extension.
pub fn file_name(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    let stem = line.rsplit_once('.').map_or(line, |(stem, extension)| {
        if extension.len() <= 4 && extension.chars().all(|c| c.is_ascii_alphanumeric()) {
            stem
        } else {
            line
        }
    });
    let joined: String = stem
        .trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    let clean = naming::sanitize(&joined);
    clean
        .chars()
        .take(MAX_NAME)
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

/// The picture as a JPEG small enough for a model.
pub fn prepare(image: &Image, max_side: u32) -> Result<Vec<u8>> {
    let mut small = ops::resize::fit_within(image, max_side, ResizeFilter::Lanczos3)?;
    small.flatten(Color::WHITE);
    encode(
        &small,
        &EncodeOptions::new(ImageFormat::Jpeg).with_quality(85),
        &CancelToken::new(),
    )
}

/// Asks the vision model of the settings about a picture.
pub fn ask(
    net: &Net,
    settings: &AiSettings,
    image: &Image,
    task: VisionTask,
    language: &str,
) -> Result<String> {
    if !settings.vision.is_set() {
        return Err(Error::new("providerNotSet"));
    }
    let side = if task == VisionTask::Ocr {
        OCR_SIDE
    } else {
        VISION_SIDE
    };
    let mut message = Message::new(Role::User, prompt(task, language));
    message.images.push(prepare(image, side)?);
    let reply = llm::chat(net, settings, &settings.vision, &[message], &[])?;
    let answer = unwrap_answer(&reply.content);
    Ok(match task {
        VisionTask::Describe | VisionTask::AltText | VisionTask::Ocr => answer,
        VisionTask::Tags => tags(&answer).join(", "),
        VisionTask::Name => file_name(&answer),
    })
}

/// Whether the vision model of the settings runs outside this computer.
pub fn leaves_computer(settings: &AiSettings) -> bool {
    settings.vision.is_set() && !net::is_local(&settings.vision.url)
}

/// Writes a description and keywords into the XMP metadata of a JPEG or PNG
/// file. The pixels are not re-encoded; the previous version of the file goes
/// to the Recycle Bin.
pub fn write_caption(path: &Path, description: &str, keywords: &[String]) -> Result<Written> {
    let unsupported = || Error::new("captionUnsupported").with("path", path.display());
    let bytes = fs::read(path).map_err(|error| Error::from(error).with("path", path.display()))?;
    let format = ImageFormat::sniff(&bytes).ok_or_else(unsupported)?;
    let existing = metadata::extract(&bytes, format).xmp;
    let packet = metadata::caption_xmp(existing.as_deref(), description, keywords)
        .ok_or_else(unsupported)?;
    let updated = metadata::replace_xmp(&bytes, format, &packet).ok_or_else(unsupported)?;
    fsops::write_file(path, &updated, Conflict::Overwrite)
}

/// Gives a file the suggested name, keeping its folder and extension. A taken
/// name gets a number.
pub fn rename(path: &Path, name: &str) -> Result<PathBuf> {
    if name.trim().is_empty() {
        return Err(Error::new("badResponse"));
    }
    let name = file_name(name);
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().into_owned());
    let folder = path.parent().unwrap_or(Path::new(""));
    let mut number = 1;
    let target = loop {
        let stem = if number == 1 {
            name.clone()
        } else {
            format!("{name}-{number}")
        };
        let candidate = folder.join(match &extension {
            Some(extension) => format!("{stem}.{extension}"),
            None => stem,
        });
        if candidate == path {
            return Ok(candidate);
        }
        if !candidate.exists() {
            break candidate;
        }
        number += 1;
    };
    fs::rename(path, &target).map_err(|error| Error::from(error).with("path", path.display()))?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use pixelit_core::Depth;

    use super::*;

    #[test]
    fn prompts_name_the_language() {
        assert!(prompt(VisionTask::Describe, "ru").contains("Russian"));
        assert!(prompt(VisionTask::Tags, "pt-BR").contains("Portuguese"));
        assert!(prompt(VisionTask::AltText, "xx").contains("in xx"));
        assert!(!prompt(VisionTask::Ocr, "ru").contains("Russian"));
    }

    #[test]
    fn answers_are_unwrapped() {
        assert_eq!(unwrap_answer(" \"a red fox\" "), "a red fox");
        assert_eq!(unwrap_answer("«лиса»"), "лиса");
        assert_eq!(unwrap_answer("```text\nline\n```"), "line");
        assert_eq!(unwrap_answer("plain"), "plain");
    }

    #[test]
    fn keywords_are_split_and_deduplicated() {
        assert_eq!(
            tags("Fox, red fox; forest\n1. Snow, fox, \"Winter\"."),
            ["fox", "red fox", "forest", "snow", "winter"]
        );
        assert_eq!(tags(""), [] as [std::string::String; 0]);
    }

    #[test]
    fn names_are_safe() {
        assert_eq!(file_name("Red Fox in Snow.jpg\nextra"), "red-fox-in-snow");
        assert_eq!(file_name("рыжая лиса"), "рыжая-лиса");
        assert!(!file_name("a/b:c*d").contains(['/', ':', '*']));
    }

    #[test]
    fn pictures_are_reduced_for_models() {
        let image = Image::new(3000, 1000, Depth::Eight, Color::rgba(10, 200, 30, 128)).unwrap();
        let bytes = prepare(&image, 1024).unwrap();
        assert!(bytes.starts_with(&[0xff, 0xd8]));
        let back = crate::generate::decode_image(&bytes).unwrap();
        assert_eq!(back.width(), 1024);
    }

    #[test]
    fn files_are_renamed_without_collisions() {
        let folder = tempfile::tempdir().unwrap();
        let first = folder.path().join("IMG_0001.jpg");
        let second = folder.path().join("IMG_0002.jpg");
        fs::write(&first, b"1").unwrap();
        fs::write(&second, b"2").unwrap();
        let renamed = rename(&first, "Red Fox").unwrap();
        assert_eq!(renamed, folder.path().join("red-fox.jpg"));
        assert_eq!(
            rename(&second, "red fox").unwrap(),
            folder.path().join("red-fox-2.jpg")
        );
        assert_eq!(rename(&renamed, "red fox").unwrap(), renamed);
        assert_eq!(rename(&renamed, "  ").unwrap_err().code, "badResponse");
    }

    #[test]
    fn captions_need_a_supported_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("note.txt");
        fs::write(&path, b"plain text").unwrap();
        let error = write_caption(&path, "text", &[]).unwrap_err();
        assert_eq!(error.code, "captionUnsupported");
        assert!(!leaves_computer(&AiSettings::default()));
    }

    #[test]
    fn an_unset_model_is_refused() {
        let image = Image::new(8, 8, Depth::Eight, Color::WHITE).unwrap();
        let error = ask(
            &Net::new(),
            &AiSettings::default(),
            &image,
            VisionTask::Describe,
            "en",
        )
        .unwrap_err();
        assert_eq!(error.code, "providerNotSet");
    }
}
