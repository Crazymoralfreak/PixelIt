//! What a generator says while it draws: the sampler step, and sometimes a
//! picture of the work so far. Parsing lives here so it can be tested without
//! a server.

use serde_json::Value;

/// A sampler step reported by ComfyUI over its socket.
/// `{"type":"progress","data":{"value":3,"max":20,"prompt_id":"..."}}`.
pub fn comfy_step(text: &str, prompt_id: &str) -> Option<(u32, u32)> {
    let value: Value = serde_json::from_str(text).ok()?;
    if value["type"].as_str() != Some("progress") {
        return None;
    }
    let data = &value["data"];
    if let Some(id) = data["prompt_id"].as_str() {
        if id != prompt_id {
            return None;
        }
    }
    let step = u32::try_from(data["value"].as_u64()?).ok()?;
    let total = u32::try_from(data["max"].as_u64()?).ok()?;
    (total > 0).then_some((step, total))
}

/// The picture bytes of a ComfyUI preview frame.
/// The first four bytes are the event (1), the next four the image kind.
pub fn comfy_preview(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.len() <= 8 {
        return None;
    }
    let event = u32::from_be_bytes(bytes[0..4].try_into().ok()?);
    (event == 1).then_some(&bytes[8..])
}

/// Step, total and the optional preview of a Forge `/progress` answer.
pub fn forge_progress(value: &Value) -> Option<(u32, u32, Option<&str>)> {
    let state = &value["state"];
    let step = u32::try_from(state["sampling_step"].as_u64()?).ok()?;
    let total = u32::try_from(state["sampling_steps"].as_u64().unwrap_or(0)).unwrap_or(0);
    let image = value["current_image"]
        .as_str()
        .filter(|text| !text.is_empty());
    Some((step, total.max(step).max(1), image))
}

/// The last `n/m` in a stable-diffusion.cpp log that can be a sampler step.
/// Sizes such as `1024/1024` are too large to be a step count and are ignored.
pub fn sampler_step(text: &str) -> Option<(u32, u32)> {
    let bytes = text.as_bytes();
    let mut found = None;
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'/' {
            continue;
        }
        index += 1;
        let second = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if second == index {
            continue;
        }
        let step: u32 = text[start..second - 1].parse().unwrap_or(0);
        let total: u32 = text[second..index].parse().unwrap_or(0);
        if (1..=150).contains(&total) && step <= total {
            found = Some((step, total));
        }
    }
    found
}

/// Whether an error from an image service means it will not draw several
/// pictures in one request.
pub fn refuses_count(error: &pixelit_core::Error) -> bool {
    if error.code != "httpStatus" || error.params.get("status").map(String::as_str) != Some("400") {
        return false;
    }
    let detail = error
        .params
        .get("detail")
        .map_or("", String::as_str)
        .to_ascii_lowercase();
    detail.contains("\"n\"")
        || detail.contains(" n ")
        || detail.contains("batch")
        || detail.contains("one image")
        || detail.contains("n must")
}

#[cfg(test)]
mod tests {
    use pixelit_core::Error;
    use serde_json::json;

    use super::*;

    #[test]
    fn comfy_progress_is_read_for_this_prompt() {
        let text = r#"{"type":"progress","data":{"value":3,"max":20,"prompt_id":"abc"}}"#;
        assert_eq!(comfy_step(text, "abc"), Some((3, 20)));
        assert_eq!(comfy_step(text, "other"), None);
        assert_eq!(comfy_step(r#"{"type":"executing"}"#, "abc"), None);
    }

    #[test]
    fn comfy_preview_frames_start_with_the_event() {
        let mut bytes = vec![0, 0, 0, 1, 0, 0, 0, 2, 9, 8];
        assert_eq!(comfy_preview(&bytes), Some(&[9, 8][..]));
        bytes[3] = 2;
        assert_eq!(comfy_preview(&bytes), None);
        assert_eq!(comfy_preview(&[0, 0, 0, 1]), None);
    }

    #[test]
    fn forge_progress_carries_the_step_and_the_picture() {
        let value = json!({
            "progress": 0.5,
            "state": { "sampling_step": 10, "sampling_steps": 20 },
            "current_image": "aaaa",
        });
        assert_eq!(forge_progress(&value), Some((10, 20, Some("aaaa"))));
        assert_eq!(forge_progress(&json!({})), None);
    }

    #[test]
    fn sampler_logs_keep_the_last_step() {
        let log = "size 1024/1024\nstep 1/20\nstep 4/20\n";
        assert_eq!(sampler_step(log), Some((4, 20)));
        assert_eq!(sampler_step("no steps here"), None);
    }

    #[test]
    fn a_refused_count_is_told_from_other_errors() {
        let refused = Error::new("httpStatus")
            .with("status", 400)
            .detail("n must be 1");
        assert!(refuses_count(&refused));
        let prompt = Error::new("httpStatus")
            .with("status", 400)
            .detail("the prompt was rejected");
        assert!(!refuses_count(&prompt));
        assert!(!refuses_count(&Error::new("network")));
    }
}
