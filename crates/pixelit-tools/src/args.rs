//! Reading tool arguments and building their schemas.

use std::path::PathBuf;

use serde_json::{json, Value};

use pixelit_core::{Error, Result};

fn missing(name: &str) -> Error {
    Error::new("badArguments").with("name", name)
}

/// A required, non-empty string.
pub fn text(arguments: &Value, name: &str) -> Result<String> {
    optional_text(arguments, name).ok_or_else(|| missing(name))
}

pub fn optional_text(arguments: &Value, name: &str) -> Option<String> {
    arguments[name]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub fn path(arguments: &Value, name: &str) -> Result<PathBuf> {
    text(arguments, name).map(PathBuf::from)
}

pub fn optional_path(arguments: &Value, name: &str) -> Option<PathBuf> {
    optional_text(arguments, name).map(PathBuf::from)
}

/// A list of paths; a single string counts as a list of one.
pub fn paths(arguments: &Value, name: &str) -> Result<Vec<PathBuf>> {
    let list: Vec<PathBuf> = match &arguments[name] {
        Value::String(one) if !one.trim().is_empty() => vec![PathBuf::from(one.trim())],
        Value::Array(many) => many
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .collect(),
        _ => Vec::new(),
    };
    if list.is_empty() {
        Err(missing(name))
    } else {
        Ok(list)
    }
}

pub fn flag(arguments: &Value, name: &str) -> bool {
    arguments[name].as_bool().unwrap_or(false)
}

/// A number; models sometimes send numbers as strings.
pub fn number(arguments: &Value, name: &str) -> Option<f64> {
    match &arguments[name] {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.trim().parse().ok(),
        _ => None,
    }
    .filter(|value: &f64| value.is_finite())
}

pub fn whole(arguments: &Value, name: &str) -> Option<u64> {
    number(arguments, name)
        .filter(|value| *value >= 0.0)
        .map(|value| value.round() as u64)
}

/// A JSON Schema of an object with the given properties.
pub fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

pub fn string(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

pub fn integer(description: &str) -> Value {
    json!({ "type": "integer", "description": description })
}

pub fn boolean(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

pub fn choice(description: &str, values: &[&str]) -> Value {
    json!({ "type": "string", "description": description, "enum": values })
}

pub fn path_list(description: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": description })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_read_leniently() {
        let arguments = json!({
            "name": "  fox ",
            "empty": " ",
            "inputs": ["a.png", "", 7, " b.png "],
            "one": "c.png",
            "width": "640",
            "ratio": 1.5,
            "bad": "x",
            "on": true,
        });
        assert_eq!(text(&arguments, "name").unwrap(), "fox");
        assert_eq!(text(&arguments, "empty").unwrap_err().code, "badArguments");
        assert_eq!(
            text(&arguments, "absent").unwrap_err().params["name"],
            "absent"
        );
        assert_eq!(
            paths(&arguments, "inputs").unwrap(),
            [PathBuf::from("a.png"), PathBuf::from("b.png")]
        );
        assert_eq!(paths(&arguments, "one").unwrap(), [PathBuf::from("c.png")]);
        assert!(paths(&arguments, "absent").is_err());
        assert_eq!(whole(&arguments, "width"), Some(640));
        assert_eq!(number(&arguments, "ratio"), Some(1.5));
        assert_eq!(number(&arguments, "bad"), None);
        assert!(flag(&arguments, "on"));
        assert!(!flag(&arguments, "absent"));
    }

    #[test]
    fn schemas_list_required_names() {
        let schema = object(json!({ "path": string("File") }), &["path"]);
        assert_eq!(schema["required"][0], "path");
        assert_eq!(schema["properties"]["path"]["type"], "string");
    }
}
