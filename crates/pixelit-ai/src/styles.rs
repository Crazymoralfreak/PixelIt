//! Styles: named additions to a prompt, in the way Fooocus stores them.
//!
//! A style keeps text only. Several may be on at once; each is applied in
//! order. `{prompt}` in an addition is replaced by the prompt so far.

use std::fs;
use std::path::Path;

use pixelit_core::{Error, Result};
use pixelit_engine::store::{load_or_default, no_migration, save_json};
use pixelit_engine::Paths;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version of `styles.json`.
const VERSION: u32 = 1;

/// A style shipped with PixelIt. Its name is translated in the interface.
#[derive(Clone, Copy)]
struct Builtin {
    id: &'static str,
    name: &'static str,
    prompt: &'static str,
    negative: &'static str,
}

const BUILTIN: &[Builtin] = &[
    Builtin {
        id: "photo",
        name: "Photograph",
        prompt: "{prompt}, photograph, natural light, sharp focus",
        negative: "illustration, painting, cartoon, render",
    },
    Builtin {
        id: "cinematic",
        name: "Cinematic",
        prompt: "cinematic still of {prompt}, film grain, shallow depth of field",
        negative: "cartoon, flat lighting, oversaturated",
    },
    Builtin {
        id: "anime",
        name: "Anime",
        prompt: "{prompt}, anime style, clean lineart, vibrant color",
        negative: "photorealistic, 3d render, photograph",
    },
    Builtin {
        id: "digital",
        name: "Digital painting",
        prompt: "{prompt}, digital painting, detailed, painterly",
        negative: "photograph, blurry, sketch",
    },
    Builtin {
        id: "watercolor",
        name: "Watercolor",
        prompt: "{prompt}, watercolor painting, soft washes, paper texture",
        negative: "photograph, 3d render, hard edges",
    },
    Builtin {
        id: "render",
        name: "3D render",
        prompt: "{prompt}, 3d render, studio lighting, detailed materials",
        negative: "photograph, sketch, painting",
    },
    Builtin {
        id: "pixel",
        name: "Pixel art",
        prompt: "{prompt}, pixel art, limited palette, crisp pixels",
        negative: "photograph, blurry, 3d render",
    },
    Builtin {
        id: "sketch",
        name: "Pencil sketch",
        prompt: "{prompt}, pencil sketch, graphite, paper texture",
        negative: "color photograph, 3d render, painting",
    },
    Builtin {
        id: "oil",
        name: "Oil painting",
        prompt: "{prompt}, oil painting, visible brushstrokes, canvas texture",
        negative: "photograph, digital art, 3d render",
    },
    Builtin {
        id: "portrait",
        name: "Portrait",
        prompt: "portrait of {prompt}, detailed face, soft light",
        negative: "cropped face, blurry, extra limbs",
    },
];

/// One style. Built-in styles cannot be changed or removed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Style {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub negative: String,
    pub builtin: bool,
}

fn builtin_styles() -> Vec<Style> {
    BUILTIN
        .iter()
        .map(|style| Style {
            id: style.id.to_owned(),
            name: style.name.to_owned(),
            prompt: style.prompt.to_owned(),
            negative: style.negative.to_owned(),
            builtin: true,
        })
        .collect()
}

fn is_builtin(id: &str) -> bool {
    BUILTIN.iter().any(|style| style.id == id)
}

fn load_custom(paths: &Paths) -> Vec<Style> {
    let mut styles: Vec<Style> = load_or_default(&paths.styles(), VERSION, no_migration);
    styles.retain(|style| !style.id.trim().is_empty() && !is_builtin(&style.id));
    for style in &mut styles {
        style.builtin = false;
        style.id = slug(&style.id);
    }
    styles
}

fn save_custom(paths: &Paths, styles: &[Style]) -> Result<()> {
    let custom: Vec<Style> = styles
        .iter()
        .filter(|style| !style.builtin)
        .cloned()
        .collect();
    save_json(&paths.styles(), VERSION, &custom)
}

/// Built-in styles first within their group: the user's own, then the built-in.
pub fn list(paths: &Paths) -> Vec<Style> {
    let mut styles = load_custom(paths);
    styles.extend(builtin_styles());
    styles
}

/// Stores a style of the user. A built-in id is refused. An empty id is made
/// from the name, and an id already in use is replaced.
pub fn save(paths: &Paths, mut style: Style) -> Result<Style> {
    style.name = trim_owned(&style.name);
    style.prompt = trim_owned(&style.prompt);
    style.negative = trim_owned(&style.negative);
    if style.name.is_empty() {
        return Err(Error::new("emptyName"));
    }
    let id = if style.id.trim().is_empty() {
        slug(&style.name)
    } else {
        slug(&style.id)
    };
    if id.is_empty() || is_builtin(&id) {
        return Err(Error::new("builtinStyle"));
    }
    style.id = id;
    style.builtin = false;
    let mut styles = load_custom(paths);
    if let Some(known) = styles.iter_mut().find(|known| known.id == style.id) {
        *known = style.clone();
    } else {
        styles.push(style.clone());
    }
    save_custom(paths, &styles)?;
    Ok(style)
}

/// Removes a style of the user. A built-in style is left in place.
pub fn remove(paths: &Paths, id: &str) -> Result<bool> {
    if is_builtin(id) {
        return Err(Error::new("builtinStyle"));
    }
    let mut styles = load_custom(paths);
    let before = styles.len();
    styles.retain(|style| style.id != id);
    save_custom(paths, &styles)?;
    Ok(styles.len() != before)
}

/// The styles of `ids` that exist, in the order of `ids`.
pub fn selected(paths: &Paths, ids: &[String]) -> Vec<Style> {
    let styles = list(paths);
    ids.iter()
        .filter_map(|id| styles.iter().find(|style| style.id == *id).cloned())
        .collect()
}

/// Applies styles to a prompt and a negative prompt.
pub fn apply(prompt: &str, negative: &str, styles: &[Style]) -> (String, String) {
    let mut text = prompt.trim().to_owned();
    let mut parts = split_negative(negative);
    for style in styles {
        let addition = style.prompt.trim();
        if addition.contains("{prompt}") {
            text = addition.replace("{prompt}", text.trim());
        } else if addition.is_empty() {
            // A style may add only a negative prompt.
        } else if text.trim().is_empty() {
            addition.clone_into(&mut text);
        } else {
            text = format!("{}, {addition}", text.trim().trim_end_matches(','));
        }
        for part in split_negative(&style.negative) {
            if !parts.iter().any(|known| known.eq_ignore_ascii_case(&part)) {
                parts.push(part);
            }
        }
    }
    (text, parts.join(", "))
}

fn split_negative(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Reads a Fooocus style file: a list of `{name, prompt, negative_prompt}`.
/// A style whose name is already saved is replaced.
pub fn import_file(paths: &Paths, file: &Path) -> Result<Vec<Style>> {
    let text = fs::read_to_string(file)?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| Error::new("badFile").detail(error))?;
    let items = value
        .as_array()
        .ok_or_else(|| Error::new("badFile").detail("expected a list of styles"))?;
    let mut saved = Vec::new();
    for item in items {
        let name = item["name"].as_str().unwrap_or_default().trim().to_owned();
        if name.is_empty() {
            continue;
        }
        let style = save(
            paths,
            Style {
                name,
                prompt: item["prompt"].as_str().unwrap_or_default().to_owned(),
                negative: item["negative_prompt"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                ..Style::default()
            },
        )?;
        saved.push(style);
    }
    if saved.is_empty() {
        return Err(Error::new("badFile").detail("no styles"));
    }
    Ok(saved)
}

/// Writes styles in the Fooocus shape. Unknown ids are skipped.
pub fn export_file(paths: &Paths, ids: &[String], file: &Path) -> Result<()> {
    let styles = selected(paths, ids);
    if styles.is_empty() {
        return Err(Error::new("notFound"));
    }
    let body: Vec<Value> = styles
        .iter()
        .map(|style| {
            serde_json::json!({
                "name": style.name,
                "prompt": style.prompt,
                "negative_prompt": style.negative,
            })
        })
        .collect();
    let text =
        serde_json::to_vec_pretty(&body).map_err(|error| Error::new("internal").detail(error))?;
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder)?;
    }
    fs::write(file, text)?;
    Ok(())
}

fn trim_owned(text: &str) -> String {
    text.trim().to_owned()
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn additions_append_and_wrap() {
        let styles = [
            Style {
                prompt: String::from("soft light"),
                negative: String::from("blur, blur"),
                ..Style::default()
            },
            Style {
                prompt: String::from("cinematic still of {prompt}"),
                negative: String::from("Blur, cartoon"),
                ..Style::default()
            },
        ];
        let (prompt, negative) = apply("a fox", "noise, blur", &styles);
        assert_eq!(prompt, "cinematic still of a fox, soft light");
        assert_eq!(negative, "noise, blur, cartoon");
    }

    #[test]
    fn an_empty_prompt_takes_the_addition() {
        let style = Style {
            prompt: String::from("a red fox"),
            ..Style::default()
        };
        assert_eq!(apply("  ", "", &[style]).0, "a red fox");
    }

    #[test]
    fn fooocus_files_round_trip() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let file = folder.path().join("styles.json");
        fs::write(
            &file,
            r#"[{"name":"Ink","prompt":"{prompt}, ink","negative_prompt":"color"}]"#,
        )
        .unwrap();
        let imported = import_file(&paths, &file).unwrap();
        assert_eq!(imported[0].id, "ink");
        assert!(!imported[0].builtin);
        let again = folder.path().join("out.json");
        export_file(&paths, &[String::from("ink")], &again).unwrap();
        let back: Value = serde_json::from_str(&fs::read_to_string(&again).unwrap()).unwrap();
        assert_eq!(back[0]["prompt"], "{prompt}, ink");
        assert!(remove(&paths, "ink").unwrap());
        assert!(remove(&paths, "photo").is_err());
        assert!(list(&paths)
            .iter()
            .any(|style| style.id == "photo" && style.builtin));
    }

    #[test]
    fn saving_replaces_the_same_name() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        save(
            &paths,
            Style {
                name: String::from("Ink"),
                prompt: String::from("ink"),
                ..Style::default()
            },
        )
        .unwrap();
        save(
            &paths,
            Style {
                name: String::from("Ink"),
                prompt: String::from("pen"),
                ..Style::default()
            },
        )
        .unwrap();
        let custom: Vec<Style> = load_custom(&paths);
        assert_eq!(custom.len(), 1);
        assert_eq!(custom[0].prompt, "pen");
    }
}
