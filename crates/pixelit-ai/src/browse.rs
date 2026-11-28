//! Search of the model catalogues Stability Matrix shows: CivitAI, CivArchive,
//! Hugging Face and OpenModelDB. Nothing is downloaded here.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use pixelit_core::{Error, Result};

use crate::net::Ask;

use crate::config::ModelKind;
use crate::net::Net;
use crate::secrets;

/// A catalogue the model search can ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchSource {
    Civitai,
    CivArchive,
    HuggingFace,
    OpenModelDb,
}

impl SearchSource {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Civitai => "civitai",
            Self::CivArchive => "civarchive",
            Self::HuggingFace => "huggingface",
            Self::OpenModelDb => "openmodeldb",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "civitai" => Some(Self::Civitai),
            "civarchive" => Some(Self::CivArchive),
            "huggingface" => Some(Self::HuggingFace),
            "openmodeldb" => Some(Self::OpenModelDb),
            _ => None,
        }
    }

    /// Host whose key, when one is stored, is sent with the search.
    pub const fn host(self) -> Option<&'static str> {
        match self {
            Self::Civitai => Some("civitai.com"),
            Self::HuggingFace => Some("huggingface.co"),
            Self::CivArchive | Self::OpenModelDb => None,
        }
    }
}

/// One downloadable file of a catalogue model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFile {
    pub file_name: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
    /// The extension, without the dot.
    pub format: String,
}

/// One file the user can download. `files` lists every choice; the fields
/// beside it are the one offered first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelHit {
    pub source: SearchSource,
    pub name: String,
    pub kind: ModelKind,
    pub file_name: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
    pub base_model: String,
    /// Small pictures of the model. The window does not load them itself.
    pub previews: Vec<String>,
    #[serde(default)]
    pub files: Vec<ModelFile>,
    /// Full path when a file of this model is already in a shared folder.
    #[serde(default)]
    pub installed: String,
}

fn format_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map_or_default(|ext| ext.to_string_lossy().to_ascii_lowercase())
}

fn lone_file(name: &str, url: &str, bytes: u64, sha: &str) -> ModelFile {
    ModelFile {
        format: format_of(name),
        file_name: name.to_owned(),
        url: url.to_owned(),
        bytes,
        sha256: sha.to_owned(),
    }
}

/// Fills `files` from the primary file when a catalogue only has one.
fn finish(mut hit: ModelHit) -> ModelHit {
    if hit.files.is_empty() && !hit.file_name.is_empty() {
        hit.files
            .push(lone_file(&hit.file_name, &hit.url, hit.bytes, &hit.sha256));
    }
    hit
}

/// One page of a catalogue search. `cursor` is empty when there is no next page.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub hits: Vec<ModelHit>,
    pub cursor: String,
}

/// What the search was asked for.
#[derive(Clone, Debug)]
pub struct SearchQuery {
    pub source: SearchSource,
    pub text: String,
    pub kind: ModelKind,
    pub sort: String,
    pub period: String,
    pub base_model: String,
    pub show_nsfw: bool,
    /// Opaque token of the next page. Empty asks for the first page.
    pub cursor: String,
}

fn enc(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

fn civitai_type(kind: ModelKind) -> &'static str {
    match kind {
        ModelKind::Checkpoints => "Checkpoint",
        ModelKind::Loras => "LORA",
        ModelKind::Vaes => "VAE",
        ModelKind::Embeddings => "TextualInversion",
        ModelKind::ControlNet => "Controlnet",
        ModelKind::Upscale => "Upscaler",
    }
}

fn text_of(value: &Value, names: &[&str]) -> String {
    for name in names {
        if let Some(text) = value.get(*name).and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return text.to_owned();
            }
        }
    }
    String::new()
}

fn number_of(value: &Value, names: &[&str]) -> u64 {
    for name in names {
        if let Some(number) = value.get(*name).and_then(Value::as_u64) {
            return number;
        }
        if let Some(number) = value.get(*name).and_then(Value::as_f64) {
            return number as u64;
        }
    }
    0
}

fn adult(value: &Value) -> bool {
    value.get("nsfw").and_then(Value::as_bool).unwrap_or(false)
        || number_of(value, &["nsfwLevel"]) > 1
}

/// A https picture from a catalogue we know, asked at a modest size.
pub fn modest_preview(url: &str) -> String {
    let url = url.trim();
    if !url.starts_with("https://") {
        return String::new();
    }
    let Some(host) = crate::net::host_of(url) else {
        return String::new();
    };
    let allowed = matches!(
        host.as_str(),
        "image.civitai.com"
            | "civitai.com"
            | "civarchive.com"
            | "huggingface.co"
            | "openmodeldb.info"
    ) || host.ends_with(".huggingface.co")
        || host.ends_with(".civitai.com")
        || host.ends_with(".openmodeldb.info");
    if !allowed {
        return String::new();
    }
    if let Some(start) = url.find("/width=") {
        let digits = start + "/width=".len();
        let end = url[digits..]
            .find(|symbol: char| !symbol.is_ascii_digit())
            .map_or(url.len(), |index| digits + index);
        let mut modest = String::new();
        modest.push_str(&url[..digits]);
        modest.push_str("768");
        modest.push_str(&url[end..]);
        return modest;
    }
    if let Some(start) = url.find("/original=true") {
        let mut modest = String::new();
        modest.push_str(&url[..start]);
        modest.push_str("/width=768");
        modest.push_str(&url[start + "/original=true".len()..]);
        return modest;
    }
    url.to_owned()
}

fn is_still(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    !lower.contains(".mp4") && !lower.contains(".webm") && !lower.contains(".mov")
}

/// Every still picture a catalogue entry offers, at a size that fits a card.
fn pictures(value: &Value, show_nsfw: bool) -> Vec<String> {
    let Some(images) = value.get("images").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for image in images {
        if !show_nsfw && adult(image) {
            continue;
        }
        if image
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("video"))
        {
            continue;
        }
        let url = image
            .as_str()
            .map_or_else(|| text_of(image, &["url", "src"]), str::to_owned);
        if !is_still(&url) {
            continue;
        }
        let modest = modest_preview(&url);
        if modest.is_empty() || found.iter().any(|known| known == &modest) {
            continue;
        }
        found.push(modest);
        if found.len() == 12 {
            break;
        }
    }
    found
}

fn file_of(version: &Value) -> Option<(String, String, u64, String)> {
    let files = version.get("files")?.as_array()?;
    let best = files.iter().max_by_key(|file| {
        let name = text_of(file, &["name"]);
        let preferred = Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("safetensors"));
        (preferred, number_of(file, &["sizeKB", "size"]))
    })?;
    let name = text_of(best, &["name"]);
    let url = text_of(best, &["downloadUrl", "url"]);
    if name.is_empty() || url.is_empty() {
        return None;
    }
    let bytes = number_of(best, &["sizeKB"])
        .saturating_mul(1024)
        .max(number_of(best, &["size"]));
    let sha = best.get("hashes").map_or(String::new(), |hashes| {
        text_of(hashes, &["SHA256", "sha256"])
    });
    Some((name, url, bytes, sha))
}

/// Reads a CivitAI `/api/v1/models` answer.
pub fn civitai_hits(value: &Value, kind: ModelKind, show_nsfw: bool) -> Vec<ModelHit> {
    let mut hits = Vec::new();
    for item in value
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if !show_nsfw && adult(item) {
            continue;
        }
        let versions = item.get("modelVersions").and_then(Value::as_array);
        let Some(version) = versions.and_then(|list| list.first()) else {
            continue;
        };
        if !show_nsfw && adult(version) {
            continue;
        }
        let Some((file_name, url, bytes, sha256)) = file_of(version) else {
            continue;
        };
        hits.push(finish(ModelHit {
            source: SearchSource::Civitai,
            name: text_of(item, &["name"]),
            kind,
            file_name,
            url,
            bytes,
            sha256,
            base_model: text_of(version, &["baseModel"]),
            previews: pictures(version, show_nsfw),
            files: Vec::new(),
            installed: String::new(),
        }));
    }
    hits
}

fn results_of(value: &Value) -> Option<&Vec<Value>> {
    value
        .pointer("/pageProps/data/results")
        .or_else(|| value.pointer("/data/results"))
        .or_else(|| value.get("results"))
        .and_then(Value::as_array)
}

/// Reads a CivArchive search page.
pub fn civarchive_hits(value: &Value, kind: ModelKind, show_nsfw: bool) -> Vec<ModelHit> {
    let mut hits = Vec::new();
    for item in results_of(value).into_iter().flatten() {
        if !show_nsfw && adult(item) {
            continue;
        }
        let version = item.get("version").unwrap_or(item);
        let Some((file_name, url, bytes, sha256)) = file_of(version).or_else(|| file_of(item))
        else {
            continue;
        };
        hits.push(finish(ModelHit {
            source: SearchSource::CivArchive,
            name: text_of(item, &["name"]),
            kind,
            file_name,
            url,
            bytes,
            sha256,
            base_model: text_of(version, &["baseModel", "base_model"]),
            previews: pictures(version, show_nsfw),
            files: Vec::new(),
            installed: String::new(),
        }));
    }
    hits
}

/// The build id Next.js puts in the CivArchive front page.
pub fn civarchive_build(html: &str) -> Option<&str> {
    let marker = "\"buildId\":\"";
    let start = html.find(marker)? + marker.len();
    let end = html[start..].find('"')?;
    let id = &html[start..start + end];
    (!id.is_empty()).then_some(id)
}

/// Reads a Hugging Face model list. The file itself is resolved when downloading.
pub fn huggingface_hits(value: &Value, kind: ModelKind) -> Vec<ModelHit> {
    let mut hits = Vec::new();
    for item in value.as_array().into_iter().flatten() {
        let id = text_of(item, &["id", "modelId"]);
        if id.is_empty() {
            continue;
        }
        hits.push(finish(ModelHit {
            source: SearchSource::HuggingFace,
            name: id.clone(),
            kind,
            file_name: String::new(),
            url: format!("https://huggingface.co/api/models/{id}"),
            bytes: 0,
            sha256: String::new(),
            base_model: String::new(),
            previews: Vec::new(),
            files: Vec::new(),
            installed: String::new(),
        }));
    }
    hits
}

/// Picks the model file out of a Hugging Face model description.
pub fn huggingface_file(value: &Value) -> Option<ModelHit> {
    let id = text_of(value, &["id"]);
    let siblings = value.get("siblings")?.as_array()?;
    let best = siblings.iter().max_by_key(|file| {
        let name = text_of(file, &["rfilename"]);
        let lower = name.to_ascii_lowercase();
        let preferred = ["safetensors", "gguf", "ckpt", "pth"]
            .iter()
            .any(|extension| {
                Path::new(&lower)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
            });
        (
            preferred,
            number_of(file.pointer("/lfs").unwrap_or(file), &["size"]),
        )
    })?;
    let file_name = text_of(best, &["rfilename"]);
    if id.is_empty() || file_name.is_empty() {
        return None;
    }
    let lfs = best.get("lfs").unwrap_or(best);
    Some(finish(ModelHit {
        source: SearchSource::HuggingFace,
        name: id.clone(),
        kind: ModelKind::Checkpoints,
        file_name: file_name.clone(),
        url: format!("https://huggingface.co/{id}/resolve/main/{file_name}"),
        bytes: number_of(lfs, &["size"]),
        sha256: text_of(lfs, &["sha256"]),
        base_model: String::new(),
        previews: Vec::new(),
        files: Vec::new(),
        installed: String::new(),
    }))
}

/// Reads the OpenModelDB catalogue and keeps the models whose name matches.
pub fn openmodeldb_hits(value: &Value, text: &str, show_nsfw: bool) -> Vec<ModelHit> {
    let needle = text.trim().to_ascii_lowercase();
    let mut hits = Vec::new();
    let Some(object) = value.as_object() else {
        return hits;
    };
    for (id, model) in object {
        if !show_nsfw && adult(model) {
            continue;
        }
        let name = text_of(model, &["name"]);
        let shown = if name.is_empty() { id.clone() } else { name };
        if !needle.is_empty()
            && !shown.to_ascii_lowercase().contains(&needle)
            && !id.to_ascii_lowercase().contains(&needle)
        {
            continue;
        }
        let Some(resources) = model.get("resources").and_then(Value::as_array) else {
            continue;
        };
        let mut files: Vec<ModelFile> = resources
            .iter()
            .filter_map(|resource| openmodeldb_file(id, resource))
            .collect();
        if files.is_empty() {
            continue;
        }
        let onnx = files
            .iter()
            .position(|file| file.format == "onnx")
            .unwrap_or(0);
        files.swap(0, onnx);
        let chosen = files[0].clone();
        hits.push(ModelHit {
            source: SearchSource::OpenModelDb,
            name: shown,
            kind: ModelKind::Upscale,
            file_name: chosen.file_name,
            url: chosen.url,
            bytes: chosen.bytes,
            sha256: chosen.sha256,
            base_model: text_of(model, &["architecture"]),
            previews: pictures(model, show_nsfw),
            files,
            installed: String::new(),
        });
    }
    hits
}

/// Hosts that show a page about a file instead of the file itself.
const PAGE_HOSTS: &[&str] = &[
    "mega.nz",
    "mega.io",
    "drive.google.com",
    "disk.yandex.",
    "dropbox.com",
];

/// A link that is the file itself, not a page that only offers the file.
fn direct_file(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://") && !PAGE_HOSTS.iter().any(|host| lower.contains(host))
}

/// Hugging Face `/blob/` pages are not the file. `/resolve/` is.
fn huggingface_resolve(url: &str) -> String {
    if url.contains("huggingface.co/") && url.contains("/blob/") {
        url.replacen("/blob/", "/resolve/", 1)
    } else {
        url.to_owned()
    }
}

/// One OpenModelDB file that has a direct address.
fn openmodeldb_file(id: &str, resource: &Value) -> Option<ModelFile> {
    let url = resource
        .get("urls")
        .and_then(Value::as_array)
        .and_then(|urls| {
            urls.iter()
                .filter_map(Value::as_str)
                .find(|url| direct_file(url))
        })?;
    let url = huggingface_resolve(url);
    let extension = text_of(resource, &["type"]);
    let file_name = if extension.is_empty() {
        format!("{id}.bin")
    } else {
        format!("{id}.{extension}")
    };
    Some(ModelFile {
        format: extension.to_ascii_lowercase(),
        file_name,
        url,
        bytes: number_of(resource, &["size"]),
        sha256: text_of(resource, &["sha256"]),
    })
}

/// Largest preview kept in memory: a catalogue thumbnail, not the model.
const MAX_PREVIEW: usize = 1_500_000;

/// A catalogue picture, fetched here so the window never opens it itself.
pub fn load_preview(net: &Net, url: &str) -> Result<(String, String)> {
    let modest = modest_preview(url);
    if modest.is_empty() {
        return Err(Error::new("badUrl").with("url", url));
    }
    let stream = net.open_get(
        &modest,
        &Ask {
            accept: "image/*",
            referer: None,
            local: false,
            timeout: Some(Duration::from_secs(20)),
        },
    )?;
    let mut reader = stream.into_reader();
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        if bytes.len() >= MAX_PREVIEW {
            return Err(Error::new("tooLarge"));
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|error| Error::new("network").detail(error))?;
        if read == 0 {
            break;
        }
        let room = MAX_PREVIEW - bytes.len();
        if read > room {
            return Err(Error::new("tooLarge"));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let mime = match bytes.as_slice() {
        [0xFF, 0xD8, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', ..] if bytes.len() > 12 && &bytes[8..12] == b"WEBP" => {
            "image/webp"
        }
        _ => return Err(Error::new("badImage")),
    };
    Ok((mime.to_owned(), STANDARD.encode(bytes)))
}

fn key_of(source: SearchSource) -> Option<String> {
    source.host().and_then(secrets::get)
}

const PAGE: usize = 24;

fn civitai_sort(sort: &str) -> &'static str {
    match sort {
        "rating" => "Highest Rated",
        "newest" => "Newest",
        _ => "Most Downloaded",
    }
}

fn civitai_period(period: &str) -> &'static str {
    match period {
        "year" => "Year",
        "month" => "Month",
        "week" => "Week",
        "day" => "Day",
        _ => "AllTime",
    }
}

fn archive_sort(sort: &str) -> &'static str {
    match sort {
        "rating" => "rating",
        "newest" => "newest",
        _ => "popular",
    }
}

fn archive_period(period: &str) -> &'static str {
    match period {
        "year" => "year",
        "month" => "month",
        "week" => "week",
        "day" => "day",
        _ => "all",
    }
}

fn huggingface_sort(sort: &str) -> &'static str {
    match sort {
        "rating" => "likes",
        "newest" => "createdAt",
        _ => "downloads",
    }
}

/// The `cursor` value of a `Link: <...>; rel="next"` header.
pub fn next_cursor(link: &str) -> String {
    let Some(start) = link.find("cursor=") else {
        return String::new();
    };
    let raw = &link[start + "cursor=".len()..];
    let raw = raw.split(['&', '>', ';']).next().unwrap_or("").trim();
    if raw.is_empty() {
        return String::new();
    }
    let probe = format!("https://example.invalid/?cursor={raw}");
    url::Url::parse(&probe)
        .ok()
        .and_then(|url| {
            url.query_pairs()
                .find(|(name, _)| name == "cursor")
                .map(|(_, value)| value.into_owned())
        })
        .unwrap_or_else(|| raw.to_owned())
}

/// Asks one catalogue. The caller has already decided the request may use the network.
pub fn search(net: &Net, query: &SearchQuery) -> Result<SearchPage> {
    match query.source {
        SearchSource::Civitai => {
            let mut url = format!(
                "https://civitai.com/api/v1/models?limit={PAGE}&types={}&sort={}&period={}&nsfw={}&query={}",
                enc(civitai_type(query.kind)),
                enc(civitai_sort(&query.sort)),
                enc(civitai_period(&query.period)),
                if query.show_nsfw { "true" } else { "false" },
                enc(&query.text),
            );
            if !query.base_model.is_empty() {
                url.push_str("&baseModels=");
                url.push_str(&enc(&query.base_model));
            }
            if !query.cursor.is_empty() {
                url.push_str("&cursor=");
                url.push_str(&enc(&query.cursor));
            }
            let value = net.get_json(&url, key_of(SearchSource::Civitai).as_deref(), None)?;
            let cursor = value
                .pointer("/metadata/nextCursor")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            Ok(SearchPage {
                hits: civitai_hits(&value, query.kind, query.show_nsfw),
                cursor,
            })
        }
        SearchSource::CivArchive => {
            let page = query.cursor.parse::<u32>().unwrap_or(1).max(1);
            let home = net.get("https://civarchive.com/", None, None)?;
            let html = String::from_utf8_lossy(&home);
            let build = civarchive_build(&html).ok_or_else(|| Error::new("badResponse"))?;
            let url = format!(
                "https://civarchive.com/_next/data/{build}/search.json?platform=civitai&sort={}&rating=all&platform_status=all&kind=models&period={}&page={page}&q={}&type={}",
                enc(archive_sort(&query.sort)),
                enc(archive_period(&query.period)),
                enc(&query.text),
                enc(civitai_type(query.kind)),
            );
            let value = net.get_json(&url, None, None)?;
            let hits = civarchive_hits(&value, query.kind, query.show_nsfw);
            let cursor = if hits.len() >= PAGE {
                (page + 1).to_string()
            } else {
                String::new()
            };
            Ok(SearchPage { hits, cursor })
        }
        SearchSource::HuggingFace => {
            let mut url = format!(
                "https://huggingface.co/api/models?limit={PAGE}&sort={}&direction=-1&search={}",
                huggingface_sort(&query.sort),
                enc(&query.text)
            );
            if !query.cursor.is_empty() {
                url.push_str("&cursor=");
                url.push_str(&enc(&query.cursor));
            }
            let stream = net.open_get(
                &url,
                &Ask {
                    accept: "application/json",
                    referer: None,
                    local: false,
                    timeout: Some(Duration::from_secs(30)),
                },
            )?;
            let cursor = next_cursor(stream.header("link").unwrap_or(""));
            let mut reader = stream.into_reader();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 16 * 1024];
            loop {
                let read = reader
                    .read(&mut buffer)
                    .map_err(|error| Error::new("network").detail(error))?;
                if read == 0 || bytes.len() > 4 * 1024 * 1024 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..read]);
            }
            let value = serde_json::from_slice(&bytes)
                .map_err(|error| Error::new("badResponse").detail(error))?;
            Ok(SearchPage {
                hits: huggingface_hits(&value, query.kind),
                cursor,
            })
        }
        SearchSource::OpenModelDb => {
            if query.kind != ModelKind::Upscale {
                return Ok(SearchPage::default());
            }
            let value = net.get_json("https://openmodeldb.info/api/v1/models.json", None, None)?;
            let offset = query.cursor.parse::<usize>().unwrap_or(0);
            let hits = openmodeldb_hits(&value, &query.text, query.show_nsfw);
            let page: Vec<ModelHit> = hits.into_iter().skip(offset).take(PAGE).collect();
            let cursor = if page.len() == PAGE {
                (offset + PAGE).to_string()
            } else {
                String::new()
            };
            Ok(SearchPage { hits: page, cursor })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_catalogue_picture_is_kept_small_and_foreign_hosts_are_dropped() {
        assert_eq!(
            modest_preview("https://image.civitai.com/a/b/width=1600/pic.jpeg"),
            "https://image.civitai.com/a/b/width=768/pic.jpeg"
        );
        assert_eq!(
            modest_preview("https://image.civitai.com/a/b/original=true/pic.jpeg"),
            "https://image.civitai.com/a/b/width=768/pic.jpeg"
        );
        assert!(modest_preview("http://image.civitai.com/a").is_empty());
        assert!(modest_preview("https://evil.example/pic.jpeg").is_empty());
    }

    #[test]
    fn the_next_page_token_is_read_from_the_link() {
        let link = "<https://example.invalid/models?cursor=abc%2F1>; rel=\"next\"";
        assert_eq!(next_cursor(link), "abc/1");
        assert!(next_cursor("").is_empty());
    }

    #[test]
    fn civitai_keeps_the_safetensors_file_and_skips_adult_models() {
        let value = json!({
            "items": [
                {
                    "name": "Shown",
                    "nsfw": false,
                    "modelVersions": [{
                        "baseModel": "SDXL 1.0",
                        "images": [{ "url": "https://image.civitai.com/a/b/width=1600/pic.jpeg" }],
                        "files": [
                            {"name": "a.ckpt", "sizeKB": 10, "downloadUrl": "https://example.invalid/a", "hashes": {"SHA256": "aa"}},
                            {"name": "a.safetensors", "sizeKB": 4, "downloadUrl": "https://example.invalid/b", "hashes": {"SHA256": "bb"}}
                        ]
                    }]
                },
                {"name": "Hidden", "nsfw": true, "modelVersions": []}
            ]
        });
        let hits = civitai_hits(&value, ModelKind::Checkpoints, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file_name, "a.safetensors");
        assert_eq!(hits[0].sha256, "bb");
        assert!(hits[0].previews[0].contains("/width=768/"));
        assert_eq!(hits[0].base_model, "SDXL 1.0");
    }

    #[test]
    fn the_civarchive_build_id_is_read_from_the_page() {
        let html = r#"<script>{"buildId":"abc-1","page":1}</script>"#;
        assert_eq!(civarchive_build(html), Some("abc-1"));
        assert_eq!(civarchive_build("no id"), None);
    }

    #[test]
    fn civarchive_results_live_under_the_page() {
        let value = json!({
            "pageProps": {"data": {"results": [{
                "name": "Archive",
                "files": [{"name": "m.safetensors", "size": 9, "downloadUrl": "https://example.invalid/m", "hashes": {"SHA256": "cc"}}]
            }]}}
        });
        let hits = civarchive_hits(&value, ModelKind::Loras, false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, ModelKind::Loras);
        assert_eq!(hits[0].bytes, 9);
    }

    #[test]
    fn huggingface_picks_the_weights_file() {
        let value = json!({
            "id": "org/model",
            "siblings": [
                {"rfilename": "README.md"},
                {"rfilename": "model.safetensors", "lfs": {"size": 42, "sha256": "dd"}}
            ]
        });
        let hit = huggingface_file(&value).unwrap();
        assert_eq!(hit.file_name, "model.safetensors");
        assert_eq!(hit.bytes, 42);
        assert!(hit.url.ends_with("/model.safetensors"));
    }

    #[test]
    fn openmodeldb_matches_the_name() {
        let value = json!({
            "one": {
                "name": "Sharp",
                "resources": [
                    {"type": "pth", "size": 8, "sha256": "ee", "urls": ["https://example.invalid/one.pth"]},
                    {"type": "onnx", "size": 9, "sha256": "ff", "urls": ["https://example.invalid/one.onnx"]}
                ]
            },
            "two": {"name": "Other", "resources": []}
        });
        let hits = openmodeldb_hits(&value, "sharp", false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, ModelKind::Upscale);
        assert_eq!(hits[0].file_name, "one.onnx");
        assert_eq!(hits[0].files.len(), 2);
        assert_eq!(hits[0].files[0].format, "onnx");
        assert!(hits[0].files.iter().any(|file| file.format == "pth"));
    }

    #[test]
    fn a_folder_link_is_not_a_file_and_a_blob_page_points_at_the_file() {
        assert!(!direct_file("https://mega.nz/folder/abc"));
        assert!(direct_file("https://example.invalid/a.onnx"));
        assert_eq!(
            huggingface_resolve("https://huggingface.co/org/model/blob/main/a.onnx"),
            "https://huggingface.co/org/model/resolve/main/a.onnx"
        );
        let value = json!({
            "only": {
                "name": "OnlyMega",
                "resources": [{"type": "pth", "urls": ["https://mega.nz/folder/abc"]}]
            }
        });
        assert!(openmodeldb_hits(&value, "mega", false).is_empty());
    }
}
