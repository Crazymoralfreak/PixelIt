//! Texts shown outside the web view: the tray, notifications, the context
//! menu of Explorer, the menu bar of macOS. They come from the same
//! dictionaries as the interface.

use std::sync::LazyLock;

use serde_json::Value;

use pixelit_core::Error;

static RU: LazyLock<Value> = LazyLock::new(|| parse(include_str!("../../../src/i18n/ru.json")));
static EN: LazyLock<Value> = LazyLock::new(|| parse(include_str!("../../../src/i18n/en.json")));
static DE: LazyLock<Value> = LazyLock::new(|| parse(include_str!("../../../src/i18n/de.json")));
static ES: LazyLock<Value> = LazyLock::new(|| parse(include_str!("../../../src/i18n/es.json")));
static FR: LazyLock<Value> = LazyLock::new(|| parse(include_str!("../../../src/i18n/fr.json")));
static PT: LazyLock<Value> = LazyLock::new(|| parse(include_str!("../../../src/i18n/pt.json")));
static PL: LazyLock<Value> = LazyLock::new(|| parse(include_str!("../../../src/i18n/pl.json")));
static TR: LazyLock<Value> = LazyLock::new(|| parse(include_str!("../../../src/i18n/tr.json")));
static ZH: LazyLock<Value> = LazyLock::new(|| parse(include_str!("../../../src/i18n/zh.json")));
static JA: LazyLock<Value> = LazyLock::new(|| parse(include_str!("../../../src/i18n/ja.json")));
static KO: LazyLock<Value> = LazyLock::new(|| parse(include_str!("../../../src/i18n/ko.json")));

const LANGUAGES: &[&str] = &[
    "ru", "en", "de", "es", "fr", "pt", "pl", "tr", "zh", "ja", "ko",
];

fn parse(text: &str) -> Value {
    serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or(Value::Null)
}

fn dictionary(lang: &str) -> &'static Value {
    match lang {
        "ru" => &RU,
        "de" => &DE,
        "es" => &ES,
        "fr" => &FR,
        "pt" => &PT,
        "pl" => &PL,
        "tr" => &TR,
        "zh" => &ZH,
        "ja" => &JA,
        "ko" => &KO,
        _ => &EN,
    }
}

/// Whether PixelIt has a dictionary for this language code.
pub fn known_language(lang: &str) -> bool {
    LANGUAGES.contains(&lang)
}

fn lookup<'a>(dictionary: &'a Value, key: &str) -> Option<&'a str> {
    key.split('.')
        .try_fold(dictionary, |node, part| node.get(part))?
        .as_str()
}

fn fill(template: &str, params: &[(&str, &str)]) -> String {
    params
        .iter()
        .fold(template.to_owned(), |text, (key, value)| {
            text.replace(&format!("{{{key}}}"), value)
        })
}

/// Text for a key such as `native.tray.open`. A key missing in the language
/// falls back to English, a key missing everywhere is shown as it is.
pub fn text(lang: &str, key: &str, params: &[(&str, &str)]) -> String {
    let template = lookup(dictionary(lang), key)
        .or_else(|| lookup(&EN, key))
        .unwrap_or(key);
    fill(template, params)
}

pub fn error_text(lang: &str, error: &Error) -> String {
    let key = format!("errors.{}", error.code);
    if lookup(&EN, &key).is_none() {
        return text(lang, "errors.unknown", &[("message", &error.code)]);
    }
    let params: Vec<(&str, &str)> = error
        .params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    text(lang, &key, &params)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const NATIVE_KEYS: &[&str] = &[
        "native.tray.open",
        "native.tray.capture",
        "native.tray.quit",
        "native.menu.about",
        "native.menu.services",
        "native.menu.hide",
        "native.menu.hideOthers",
        "native.menu.showAll",
        "native.menu.quit",
        "native.menu.edit",
        "native.menu.undo",
        "native.menu.redo",
        "native.menu.cut",
        "native.menu.copy",
        "native.menu.paste",
        "native.menu.selectAll",
        "native.menu.window",
        "native.menu.minimize",
        "native.menu.zoom",
        "native.menu.close",
        "native.contextMenu",
        "native.notify.updateAvailable",
        "native.notify.updateAvailableText",
        "native.notify.updateInstalling",
        "native.notify.updateInstallingText",
        "native.notify.batchDone",
        "native.notify.batchDoneText",
        "native.dialog.openPictures",
        "native.dialog.pictures",
        "native.dialog.allFiles",
        "native.dialog.openFolder",
        "errors.unknown",
    ];

    /// Error codes the crates and the shell can return or put into the journal.
    const ERROR_CODES: &[&str] = &[
        "accessDenied",
        "agentNotAllowed",
        "agentPathDenied",
        "aiModelFailed",
        "aiUnavailable",
        "aiWorkerFailed",
        "aiWorkerMissing",
        "appAlreadyRunning",
        "appLaunchFailed",
        "appMissing",
        "appNotOurs",
        "assistantLoop",
        "autostartFailed",
        "badArchive",
        "badArguments",
        "badColor",
        "badCrop",
        "badFile",
        "badFont",
        "badImage",
        "badKey",
        "badPage",
        "badResponse",
        "badUrl",
        "badWorkflow",
        "builtinStyle",
        "cancelled",
        "cannotEncode",
        "captionUnsupported",
        "captureFailed",
        "capturePermission",
        "chatBusy",
        "chatMissing",
        "checksumMismatch",
        "cloudConfirm",
        "cloudDisabled",
        "componentMissing",
        "emptyImage",
        "emptyName",
        "emptyPrompt",
        "emptyQuery",
        "encodeFailed",
        "fileTooLarge",
        "fontNotFound",
        "generationFailed",
        "heicCodecMissing",
        "heicCodecMissingMac",
        "hotkeyInvalid",
        "hotkeyTaken",
        "httpStatus",
        "internal",
        "io",
        "keyStoreFailed",
        "localAddress",
        "mcpBadServer",
        "mcpDisconnected",
        "mcpFailed",
        "mcpStartFailed",
        "mcpTimeout",
        "mcpToolFailed",
        "mcpVersion",
        "modelMissing",
        "network",
        "newerFile",
        "noArchive",
        "noCheckpoint",
        "noImages",
        "noModelFolder",
        "noWebImages",
        "notFound",
        "notPicture",
        "pdfPassword",
        "pdfiumMissing",
        "providerNotSet",
        "recipeNotFound",
        "refineNeedsGenerator",
        "refineNoPrompt",
        "refineTooLarge",
        "registryFailed",
        "resizeFailed",
        "responseTooLarge",
        "sessionMissing",
        "tooLarge",
        "tooLargeForFormat",
        "toolDeclined",
        "trashFailed",
        "undoNotFound",
        "unknownComponent",
        "unknownTool",
        "unsupportedFormat",
        "updating",
        "upscaleModelFailed",
        "upscalerMissing",
        "watchFailed",
    ];

    /// `{name}` placeholders that appear in a string, sorted, so word order may differ.
    fn placeholders(text: &str) -> Vec<&str> {
        let mut names = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find('{') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('}') else {
                break;
            };
            let name = &after[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                && !names.contains(&name)
            {
                names.push(name);
            }
            rest = &after[end + 1..];
        }
        names.sort_unstable();
        names
    }

    fn leaf_strings(value: &Value, prefix: &str, out: &mut Vec<(String, String)>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    leaf_strings(child, &path, out);
                }
            }
            Value::String(text) => out.push((prefix.to_owned(), text.clone())),
            _ => {}
        }
    }

    #[test]
    fn every_dictionary_has_the_same_keys_and_placeholders() {
        let mut english = Vec::new();
        leaf_strings(&EN, "", &mut english);
        assert_ne!(
            english,
            [] as [(std::string::String, std::string::String); 0]
        );
        let english_map: HashMap<&str, &str> = english
            .iter()
            .map(|(key, text)| (key.as_str(), text.as_str()))
            .collect();
        for lang in LANGUAGES {
            let dictionary = dictionary(lang);
            assert!(dictionary.is_object(), "{lang} is not valid JSON");
            let mut leaves = Vec::new();
            leaf_strings(dictionary, "", &mut leaves);
            assert_eq!(
                leaves.len(),
                english.len(),
                "{lang} has a different number of strings"
            );
            for (key, text) in &leaves {
                let Some(en_text) = english_map.get(key.as_str()) else {
                    panic!("{lang} has unexpected key {key}");
                };
                assert_eq!(
                    placeholders(text),
                    placeholders(en_text),
                    "{lang} placeholders differ for {key}"
                );
            }
            for key in NATIVE_KEYS {
                assert!(lookup(dictionary, key).is_some(), "{lang} missing {key}");
            }
            for code in ERROR_CODES {
                assert!(
                    lookup(dictionary, &format!("errors.{code}")).is_some(),
                    "{lang} missing errors.{code}"
                );
            }
        }
    }

    #[test]
    fn placeholders_are_filled_and_unknown_codes_do_not_leak_keys() {
        assert_eq!(text("en", "native.tray.quit", &[]), "Quit");
        assert_eq!(text("ru", "native.tray.quit", &[]), "Выход");
        assert_eq!(
            text("xx", "native.tray.quit", &[]),
            text("en", "native.tray.quit", &[])
        );
        assert!(known_language("de") && !known_language("xx"));
        let message = error_text("en", &Error::new("somethingNew"));
        assert!(message.contains("somethingNew") && !message.contains("errors."));
        let missing = error_text("en", &Error::new("modelMissing").with("model", "rmbg"));
        assert!(missing.contains("rmbg"));
    }
}
