//! File name templates.
//!
//! A template is plain text with tokens in braces, for example
//! `{name}_{width}x{height}` or `{date}_{index:3}`. Unknown tokens stay as they
//! are, and the result is always a valid Windows file name without extension.

/// Tokens a template may contain.
pub const TOKENS: &[&str] = &[
    "name",
    "ext",
    "width",
    "height",
    "index",
    "date",
    "time",
    "timestamp",
    "format",
    "parent",
    "ai_name",
];

/// Longest file name produced, in characters, extension not included.
pub const MAX_NAME_CHARS: usize = 180;

/// Values the tokens are replaced with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameParts<'a> {
    /// Source file name without extension.
    pub name: &'a str,
    /// Source file extension without the dot.
    pub ext: &'a str,
    /// Size of the result in pixels.
    pub width: u32,
    pub height: u32,
    /// Position of the file in the batch, starting at 1.
    pub index: u64,
    /// Local date as `YYYY-MM-DD`.
    pub date: &'a str,
    /// Local time as `HH-MM-SS`.
    pub time: &'a str,
    /// Unix time in seconds.
    pub timestamp: i64,
    /// Output format name, such as `webp`.
    pub format: &'a str,
    /// Name of the folder holding the source file.
    pub parent: &'a str,
    /// Name suggested by a vision model; empty falls back to `name`.
    pub ai_name: &'a str,
}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Makes a string safe to use as a Windows file name without extension.
pub fn sanitize(name: &str) -> String {
    let mut clean = String::with_capacity(name.len());
    for character in name.chars() {
        let forbidden = matches!(
            character,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
        ) || character.is_control();
        clean.push(if forbidden { '_' } else { character });
    }
    let mut clean: String = clean.trim().chars().take(MAX_NAME_CHARS).collect();
    while clean.ends_with('.') || clean.ends_with(' ') {
        clean.pop();
    }
    if clean.is_empty() {
        return String::from("image");
    }
    let stem = clean
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        clean.insert(0, '_');
    }
    clean
}

fn token_value(token: &str, parts: &NameParts<'_>) -> Option<String> {
    let (key, padding) = match token.split_once(':') {
        Some((key, digits)) => (key, digits.parse::<usize>().ok()?.min(12)),
        None => (token, 0),
    };
    let number = |value: u64| format!("{value:0padding$}");
    let value = match key {
        "name" => parts.name.to_owned(),
        "ext" => parts.ext.to_owned(),
        "width" => number(u64::from(parts.width)),
        "height" => number(u64::from(parts.height)),
        "index" => number(parts.index),
        "date" => parts.date.to_owned(),
        "time" => parts.time.to_owned(),
        "timestamp" => parts.timestamp.to_string(),
        "format" => parts.format.to_owned(),
        "parent" => parts.parent.to_owned(),
        "ai_name" => {
            if parts.ai_name.trim().is_empty() {
                parts.name.to_owned()
            } else {
                parts.ai_name.to_owned()
            }
        }
        _ => return None,
    };
    Some(value)
}

/// Fills a template and returns a valid file name without extension.
pub fn render(template: &str, parts: &NameParts<'_>) -> String {
    let template = if template.trim().is_empty() {
        "{name}"
    } else {
        template
    };
    let mut out = String::with_capacity(template.len() + parts.name.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            let token = &after[..close];
            if let Some(value) = token_value(token, parts) {
                out.push_str(&value);
            } else {
                out.push('{');
                out.push_str(token);
                out.push('}');
            }
            rest = &after[close + 1..];
        } else {
            out.push('{');
            rest = after;
        }
    }
    out.push_str(rest);
    sanitize(&out)
}

/// Whether the template gives different names to the files of one batch
/// without relying on the source names.
pub fn uses_index(template: &str) -> bool {
    template.contains("{index}") || template.contains("{index:")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts() -> NameParts<'static> {
        NameParts {
            name: "IMG_0042",
            ext: "jpg",
            width: 1920,
            height: 1080,
            index: 7,
            date: "2026-11-24",
            time: "18-30-05",
            timestamp: 1_795_000_000,
            format: "webp",
            parent: "Holiday",
            ai_name: "",
        }
    }

    #[test]
    fn fills_tokens() {
        let p = parts();
        assert_eq!(render("{name}", &p), "IMG_0042");
        assert_eq!(render("{name}_{width}x{height}", &p), "IMG_0042_1920x1080");
        assert_eq!(render("{parent}-{index:3}", &p), "Holiday-007");
        assert_eq!(
            render("{date}_{time}.{format}", &p),
            "2026-11-24_18-30-05.webp"
        );
        assert_eq!(render("{timestamp}", &p), "1795000000");
        assert_eq!(render("{ai_name}", &p), "IMG_0042");
        let named = NameParts {
            ai_name: "red car",
            ..p
        };
        assert_eq!(render("{ai_name}", &named), "red car");
    }

    #[test]
    fn unknown_and_broken_tokens_stay() {
        let p = parts();
        assert_eq!(render("{nope}_{name}", &p), "{nope}_IMG_0042");
        assert_eq!(render("{name", &p), "{name");
        assert_eq!(render("a}b{", &p), "a}b{");
        assert_eq!(render("{index:x}", &p), "{index_x}");
        assert_eq!(render("{index:99}", &p), "000000000007");
    }

    #[test]
    fn empty_template_means_name() {
        assert_eq!(render("  ", &parts()), "IMG_0042");
        assert_eq!(render("", &NameParts::default()), "image");
    }

    #[test]
    fn sanitizes_for_windows() {
        assert_eq!(sanitize("a<b>c:d\"e/f\\g|h?i*j"), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(sanitize("  name. . "), "name");
        assert_eq!(sanitize("line\nbreak"), "line_break");
        assert_eq!(sanitize("con"), "_con");
        assert_eq!(sanitize("COM1.backup"), "_COM1.backup");
        assert_eq!(sanitize("console"), "console");
        assert_eq!(sanitize("..."), "image");
        assert_eq!(sanitize(&"я".repeat(500)).chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn detects_index() {
        assert!(uses_index("{index}"));
        assert!(uses_index("x_{index:4}"));
        assert!(!uses_index("{name}"));
    }
}
