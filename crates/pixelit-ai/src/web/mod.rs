//! Pictures of the web: what a link leads to, previews of what was found,
//! downloads.
//!
//! A link may point at a picture or at a page. A picture is taken as it is;
//! a page is read for the pictures it names, see [`html`]. Catalogues with a
//! search of their own live in [`sources`]. Whatever the address says, a
//! file is kept only when its first bytes are those of a picture.

pub mod html;
pub mod sources;

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use pixelit_core::{naming, CancelToken, Error, ImageFormat, Result};
use pixelit_engine::fsops::{self, UndoLog, UndoRecord, Written};
use pixelit_engine::journal::{
    Journal, JournalEntry, JournalError, JournalOrigin, ERRORS_LISTED, ORIGINS_LISTED,
};
use pixelit_engine::{store, Paths};

use crate::net::{self, Ask, Net};

pub use self::sources::{search, SearchPage, WebSource};

/// Largest page read for pictures: 4 MiB. A longer page is read up to here.
const PAGE_LIMIT: u64 = 4 * 1024 * 1024;

/// Largest picture downloaded: 256 MiB.
const IMAGE_LIMIT: u64 = 256 * 1024 * 1024;

/// Largest picture taken for a preview: 16 MiB.
const PREVIEW_LIMIT: u64 = 16 * 1024 * 1024;

/// Pictures shown for one page.
pub const MAX_IMAGES: usize = 200;

/// Bytes that tell a picture from everything else.
const HEAD: usize = 4096;

/// Longest file name made from a title, in characters.
const NAME_CHARS: usize = 80;

const PAGE_TIMEOUT: Duration = Duration::from_secs(30);
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(60);

const PAGE_ACCEPT: &str = "text/html,application/xhtml+xml,image/*;q=0.9,*/*;q=0.5";

/// No format is named: a server that sees one tends to send a recoded copy
/// instead of the file as it was published.
const IMAGE_ACCEPT: &str = "image/*,*/*;q=0.5";

/// The host Pinterest serves its pictures from.
const PIN_HOST: &str = "i.pinimg.com";

/// The size of a pin that suits a grid of previews.
const PIN_LIGHT: u32 = 236;

/// A picture of the web with what is known about where it comes from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WebImage {
    /// The address of the picture in its best size.
    pub url: String,
    /// A smaller copy, when the source has one; otherwise the same address.
    pub preview: String,
    pub title: String,
    /// The size in pixels; zero when the source does not tell it.
    pub width: u32,
    pub height: u32,
    /// The page the picture was found on.
    pub page: String,
    pub author: String,
    /// The short name of the licence, such as `CC BY-SA 4.0`.
    pub license: String,
    pub license_url: String,
    /// The catalogue or the site the picture comes from.
    pub source: String,
    /// The picture the page is about: its cover, or the pin itself.
    pub main: bool,
}

/// What a link leads to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspected {
    /// The address that answered, after redirects.
    pub url: String,
    pub title: String,
    /// The link is a picture itself, not a page.
    pub direct: bool,
    pub images: Vec<WebImage>,
}

/// A downloaded picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub path: PathBuf,
    pub bytes: u64,
    /// The address the file came from; not the asked one when only a
    /// smaller copy was given out.
    pub url: String,
}

/// An address as a person types it: the scheme may be left out.
pub fn address(text: &str) -> Result<String> {
    let text = text.trim();
    let bad = || {
        let shown: String = text.chars().take(200).collect();
        Error::new("badUrl").with("url", shown)
    };
    if text.is_empty() || text.contains(char::is_whitespace) {
        return Err(bad());
    }
    let full = if text.contains("://") {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let parsed = Url::parse(&full).map_err(|_| bad())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(bad());
    }
    Ok(parsed.into())
}

/// The format of a picture by its first bytes. A document is not a picture.
fn picture(head: &[u8]) -> Option<ImageFormat> {
    ImageFormat::sniff(head).filter(|format| *format != ImageFormat::Pdf)
}

fn network(error: std::io::Error) -> Error {
    Error::new("network").detail(error)
}

/// The start of a body: enough to tell what it is.
fn read_head(reader: &mut dyn Read, cancel: &CancelToken) -> Result<Vec<u8>> {
    let mut head = vec![0u8; HEAD];
    let mut have = 0;
    while have < HEAD {
        cancel.check()?;
        let read = reader.read(&mut head[have..]).map_err(network)?;
        if read == 0 {
            break;
        }
        have += read;
    }
    head.truncate(have);
    Ok(head)
}

fn host(address: &str) -> String {
    net::host_of(address).unwrap_or_default()
}

// ------------------------------------------------------------- Pinterest

/// How large a copy of a pin is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PinSize {
    /// The file as it was uploaded.
    Original,
    /// Reduced to this width.
    Wide(u32),
    /// Reduced and cut to a fixed frame.
    Cropped,
    /// A picture of a person, not a pin.
    Avatar,
}

impl PinSize {
    /// The larger, the better the copy.
    fn rank(self) -> u32 {
        match self {
            Self::Avatar => 0,
            Self::Cropped => 1,
            Self::Wide(width) => width.saturating_add(1),
            Self::Original => u32::MAX,
        }
    }
}

/// Splits the address of a Pinterest picture into its size and the name the
/// picture has in every size.
fn pin_of(address: &str) -> Option<(String, PinSize)> {
    let parsed = Url::parse(address).ok()?;
    if parsed.host_str() != Some(PIN_HOST) {
        return None;
    }
    let parts: Vec<&str> = parsed.path_segments()?.collect();
    let [size, first, second, third, file] = parts.as_slice() else {
        return None;
    };
    if file.is_empty() {
        return None;
    }
    let size = if *size == "originals" {
        PinSize::Original
    } else if size.ends_with("_RS") {
        PinSize::Avatar
    } else {
        let (width, height) = size.split_once('x')?;
        let width: u32 = width.parse().ok()?;
        if height.is_empty() {
            PinSize::Wide(width)
        } else {
            height.parse::<u32>().ok()?;
            PinSize::Cropped
        }
    };
    Some((format!("{first}/{second}/{third}/{file}"), size))
}

fn pin_address(size: &str, name: &str) -> String {
    format!("https://{PIN_HOST}/{size}/{name}")
}

/// A page of Pinterest names one picture many times, in every size it
/// keeps. The sizes become one picture: the original to download, the
/// largest size the page named to fall back on when the original is not
/// given out. Pictures of people are dropped.
fn fold_pins(images: Vec<WebImage>) -> Vec<WebImage> {
    let mut folded: Vec<WebImage> = Vec::with_capacity(images.len());
    let mut places: HashMap<String, (usize, u32)> = HashMap::new();
    for mut image in images {
        let Some((name, size)) = pin_of(&image.url) else {
            folded.push(image);
            continue;
        };
        if size == PinSize::Avatar {
            continue;
        }
        let rank = size.rank();
        if let Some((place, known)) = places.get_mut(&name) {
            let kept = &mut folded[*place];
            kept.main |= image.main;
            if kept.title.is_empty() {
                kept.title = image.title;
            }
            if rank > *known {
                kept.preview = image.url;
                *known = rank;
            }
        } else {
            places.insert(name.clone(), (folded.len(), rank));
            image.preview = image.url;
            image.url = pin_address("originals", &name);
            // The declared size is the size of the copy, not of the original.
            image.width = 0;
            image.height = 0;
            folded.push(image);
        }
    }
    folded
}

/// A small copy of a pin for the grid, when the address is a large one.
fn light(address: &str) -> Option<String> {
    let (name, size) = pin_of(address)?;
    let large = match size {
        PinSize::Original => true,
        PinSize::Wide(width) => width > PIN_LIGHT,
        PinSize::Cropped | PinSize::Avatar => false,
    };
    large.then(|| pin_address(&format!("{PIN_LIGHT}x"), &name))
}

fn is_pinterest(address: &str) -> bool {
    let host = host(address);
    host == "pin.it" || host.split('.').any(|part| part == "pinterest")
}

/// The picture of a pin by what Pinterest tells sites that embed it.
fn pin_embedded(json: &Value, page: &str) -> Option<WebImage> {
    let picture = json["thumbnail_url"].as_str()?.trim();
    pin_of(picture)?;
    let text = |name: &str| json[name].as_str().unwrap_or_default().trim().to_owned();
    Some(WebImage {
        url: picture.to_owned(),
        preview: picture.to_owned(),
        title: text("title"),
        page: page.to_owned(),
        author: text("author_name"),
        source: host(page),
        main: true,
        ..WebImage::default()
    })
}

/// The spare way to a pin, for the day the markup of its page changes.
fn pin_spare(net: &Net, page: &str) -> Option<Inspected> {
    let ask =
        Url::parse_with_params("https://www.pinterest.com/oembed.json", &[("url", page)]).ok()?;
    let json = net.get_json(ask.as_str(), None, Some(PAGE_TIMEOUT)).ok()?;
    let image = pin_embedded(&json, page)?;
    Some(Inspected {
        url: page.to_owned(),
        title: image.title.clone(),
        direct: false,
        images: fold_pins(vec![image]),
    })
}

// --------------------------------------------------------------- inspect

/// The pictures of a page that was loaded from `address`.
pub fn page(text: &str, address: &str) -> Inspected {
    let scanned = html::scan(text, address);
    let source = host(address);
    let images = scanned
        .images
        .into_iter()
        .map(|found| WebImage {
            preview: found.url.clone(),
            url: found.url,
            title: if found.main && found.title.is_empty() {
                scanned.title.clone()
            } else {
                found.title
            },
            width: found.width,
            height: found.height,
            page: address.to_owned(),
            source: source.clone(),
            main: found.main,
            ..WebImage::default()
        })
        .collect();
    let mut images = fold_pins(images);
    // Stable: the covers stay first after the sizes were folded together.
    images.sort_by_key(|image| !image.main);
    images.truncate(MAX_IMAGES);
    Inspected {
        url: address.to_owned(),
        title: scanned.title,
        direct: false,
        images,
    }
}

/// A link that is a picture itself.
fn direct(address: &str) -> Inspected {
    let image = WebImage {
        url: address.to_owned(),
        preview: address.to_owned(),
        source: host(address),
        main: true,
        ..WebImage::default()
    };
    Inspected {
        url: address.to_owned(),
        title: String::new(),
        direct: true,
        images: fold_pins(vec![image]),
    }
}

/// Looks what a link leads to: a picture, or a page with pictures. With
/// `local` off, addresses of this computer and its network are refused.
pub fn inspect(net: &Net, url: &str, local: bool, cancel: &CancelToken) -> Result<Inspected> {
    let asked = address(url)?;
    let ask = Ask {
        accept: PAGE_ACCEPT,
        referer: None,
        local,
        timeout: Some(PAGE_TIMEOUT),
    };
    let stream = net.open_get(&asked, &ask)?;
    let found = stream.url.clone();
    let is_page = stream.content_type.contains("html");
    let is_image = stream.content_type.starts_with("image/");
    let mut reader = stream.into_reader();
    let mut bytes = read_head(reader.as_mut(), cancel)?;
    if !is_page && picture(&bytes).is_some() {
        return Ok(direct(&found));
    }
    if is_image {
        return Err(Error::new("notPicture").with("url", found));
    }
    let rest = PAGE_LIMIT.saturating_sub(bytes.len() as u64);
    bytes.extend(net::read_body(reader.as_mut(), rest, true, cancel)?);
    drop(reader);
    let mut inspected = page(&String::from_utf8_lossy(&bytes), &found);
    if inspected.images.is_empty() && is_pinterest(&found) {
        if let Some(spare) = pin_spare(net, &found) {
            inspected = spare;
        }
    }
    if inspected.images.is_empty() {
        return Err(Error::new("noWebImages").with("url", found));
    }
    Ok(inspected)
}

// --------------------------------------------------------------- preview

/// Where previews of web pictures are kept until PixelIt is started again.
pub fn previews(paths: &Paths) -> PathBuf {
    paths.temp().join("web")
}

/// Forgets the previews.
pub fn clear_previews(paths: &Paths) {
    let _ = fs::remove_dir_all(previews(paths));
}

fn fetch_preview(
    net: &Net,
    url: &str,
    page: &str,
    local: bool,
    cancel: &CancelToken,
) -> Result<Vec<u8>> {
    let ask = Ask {
        accept: IMAGE_ACCEPT,
        referer: Some(page).filter(|page| !page.is_empty()),
        local,
        timeout: Some(PREVIEW_TIMEOUT),
    };
    let stream = net.open_get(url, &ask)?;
    if stream.length.is_some_and(|length| length > PREVIEW_LIMIT) {
        return Err(Error::new("fileTooLarge"));
    }
    let found = stream.url.clone();
    let mut reader = stream.into_reader();
    let bytes =
        net::read_body(reader.as_mut(), PREVIEW_LIMIT, false, cancel).map_err(
            |error| match error.code.as_str() {
                "responseTooLarge" => Error::new("fileTooLarge"),
                _ => error,
            },
        )?;
    if picture(&bytes[..bytes.len().min(HEAD)]).is_none() {
        return Err(Error::new("notPicture").with("url", found));
    }
    Ok(bytes)
}

/// Downloads a picture for showing it in the list of what was found and
/// returns the file; the next request for the same address gets the same
/// file. `page` is the page the picture was found on.
pub fn preview(
    net: &Net,
    paths: &Paths,
    url: &str,
    page: &str,
    local: bool,
    cancel: &CancelToken,
) -> Result<PathBuf> {
    let asked = address(url)?;
    let target = previews(paths).join(format!("{}.img", net::fingerprint(&asked)));
    if target.is_file() {
        return Ok(target);
    }
    let bytes = match light(&asked) {
        Some(small) => fetch_preview(net, &small, page, local, cancel)
            .or_else(|_| fetch_preview(net, &asked, page, local, cancel)),
        None => fetch_preview(net, &asked, page, local, cancel),
    }?;
    store::write_atomic(&target, &bytes)?;
    Ok(target)
}

// -------------------------------------------------------------- download

/// Replaces `%XX` of an address with the bytes they stand for.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut plain = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let pair = (bytes[at] == b'%')
            .then(|| text.get(at + 1..at + 3))
            .flatten()
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        if let Some(byte) = pair {
            plain.push(byte);
            at += 3;
        } else {
            plain.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&plain).into_owned()
}

/// The name of the file an address points at.
fn name_in(address: &str) -> String {
    Url::parse(address)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()?
                .rfind(|part| !part.is_empty())
                .map(percent_decoded)
        })
        .unwrap_or_default()
}

/// The file name of a downloaded picture, without extension: its title, or
/// the name it has on the server.
fn stem_of(image: &WebImage, found: &str) -> String {
    let title = image.title.trim();
    let name = if title.is_empty() {
        name_in(found)
    } else {
        title.to_owned()
    };
    // A title may be a file name already, as on Wikimedia Commons.
    let name = match name.rsplit_once('.') {
        Some((stem, extension))
            if !stem.trim().is_empty() && ImageFormat::from_extension(extension).is_some() =>
        {
            stem
        }
        _ => name.as_str(),
    };
    let short: String = name.chars().take(NAME_CHARS).collect();
    naming::sanitize(&short)
}

/// Writes the body of a response into a file; returns its size.
fn save(
    part: &Path,
    head: &[u8],
    reader: &mut dyn Read,
    total: u64,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Result<u64> {
    let mut file = File::create(part)?;
    file.write_all(head)?;
    let mut have = head.len() as u64;
    progress(have, total);
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        cancel.check()?;
        let read = reader.read(&mut buffer).map_err(network)?;
        if read == 0 {
            break;
        }
        have += read as u64;
        if have > IMAGE_LIMIT {
            return Err(Error::new("fileTooLarge"));
        }
        file.write_all(&buffer[..read])?;
        progress(have, total);
    }
    if have < total {
        return Err(Error::new("network").detail("connection closed early"));
    }
    file.sync_all()?;
    Ok(have)
}

fn fetch(
    net: &Net,
    url: &str,
    image: &WebImage,
    folder: &Path,
    local: bool,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Result<Saved> {
    let asked = address(url)?;
    let ask = Ask {
        accept: IMAGE_ACCEPT,
        referer: Some(image.page.as_str()).filter(|page| !page.is_empty()),
        local,
        // A large file takes as long as it takes; a silent server is still
        // given up on.
        timeout: None,
    };
    let stream = net.open_get(&asked, &ask)?;
    let total = stream.length.unwrap_or(0);
    if total > IMAGE_LIMIT {
        return Err(Error::new("fileTooLarge"));
    }
    let found = stream.url.clone();
    let mut reader = stream.into_reader();
    let head = read_head(reader.as_mut(), cancel)?;
    let format =
        picture(&head).ok_or_else(|| Error::new("notPicture").with("url", found.as_str()))?;
    fs::create_dir_all(folder)
        .map_err(|error| Error::from(error).with("path", folder.display()))?;
    let part = folder.join(format!(".pixelit-{:08x}.part", fastrand::u32(..)));
    let name = format!("{}.{}", stem_of(image, &found), format.extension());
    let placed = save(&part, &head, reader.as_mut(), total, progress, cancel)
        .map_err(|error| error.with("path", folder.display()))
        .and_then(|bytes| Ok((fsops::place_file(&part, &folder.join(name))?, bytes)));
    match placed {
        Ok((path, bytes)) => Ok(Saved {
            path,
            bytes,
            url: found,
        }),
        Err(error) => {
            let _ = fs::remove_file(&part);
            Err(error)
        }
    }
}

/// Downloads a picture into a folder under a free name and returns the
/// file. `progress` gets the bytes received and the size of the file, zero
/// when the server does not tell it. What arrives is kept only when it is a
/// picture.
pub fn download(
    net: &Net,
    image: &WebImage,
    folder: &Path,
    local: bool,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &CancelToken,
) -> Result<Saved> {
    let first = fetch(net, &image.url, image, folder, local, progress, cancel);
    // Pinterest does not give out every original: the largest size its page
    // named is taken then.
    let spare = pin_of(&image.url)
        .map(|_| image.preview.trim())
        .filter(|spare| !spare.is_empty() && *spare != image.url);
    match (first, spare) {
        (Err(error), Some(spare)) if error.code == "httpStatus" => {
            fetch(net, spare, image, folder, local, progress, cancel)
        }
        (first, _) => first,
    }
}

/// A picture that was not downloaded, with the reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refused {
    pub url: String,
    pub error: Error,
}

/// What downloading several pictures ended with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Downloaded {
    /// The saved files, in the order the pictures were asked for.
    pub files: Vec<PathBuf>,
    pub failed: Vec<Refused>,
    pub folder: PathBuf,
}

/// Where a download of several pictures is at.
#[derive(Clone, Copy, Debug)]
pub struct Step<'a> {
    /// The position of the picture being downloaded, from zero.
    pub index: usize,
    pub count: usize,
    pub image: &'a WebImage,
    /// Bytes of the picture received, and its size when the server tells it.
    pub bytes: u64,
    pub total: u64,
}

/// Downloads pictures into a folder and writes the journal: every saved
/// file is listed with its source, its author and its licence, and the
/// whole download can be undone from there. A picture that fails does not
/// stop the others; when none is saved, the first failure is the error.
pub fn download_all(
    net: &Net,
    paths: &Paths,
    title: &str,
    images: &[WebImage],
    folder: &Path,
    local: bool,
    progress: &mut dyn FnMut(&Step<'_>),
    cancel: &CancelToken,
) -> Result<Downloaded> {
    if images.is_empty() {
        return Err(Error::new("noImages"));
    }
    let started = Instant::now();
    let mut entry = JournalEntry::new("download", title);
    let mut record = UndoRecord::new(title);
    let mut downloaded = Downloaded {
        folder: folder.to_path_buf(),
        ..Downloaded::default()
    };
    let mut cancelled = false;
    for (index, image) in images.iter().enumerate() {
        if cancel.is_cancelled() {
            cancelled = true;
            break;
        }
        let mut step = |bytes: u64, total: u64| {
            progress(&Step {
                index,
                count: images.len(),
                image,
                bytes,
                total,
            });
        };
        match download(net, image, folder, local, &mut step, cancel) {
            Ok(saved) => {
                record.note(&Written::Created(saved.path.clone()), saved.bytes);
                entry.output_bytes += saved.bytes;
                if entry.origins.len() < ORIGINS_LISTED {
                    entry.origins.push(JournalOrigin {
                        path: saved.path.display().to_string(),
                        url: saved.url,
                        page: image.page.clone(),
                        author: image.author.clone(),
                        license: image.license.clone(),
                        license_url: image.license_url.clone(),
                    });
                }
                downloaded.files.push(saved.path);
            }
            Err(error) if error.is_cancelled() => {
                cancelled = true;
                break;
            }
            Err(error) => {
                if entry.errors.len() < ERRORS_LISTED {
                    entry.errors.push(JournalError {
                        path: image.url.clone(),
                        error: Some(error.clone()),
                    });
                }
                downloaded.failed.push(Refused {
                    url: image.url.clone(),
                    error,
                });
            }
        }
    }
    entry.done = downloaded.files.len() as u32;
    entry.failed = downloaded.failed.len() as u32;
    entry.elapsed_ms = started.elapsed().as_millis() as u64;
    entry.cancelled = cancelled;
    entry.folder = folder.display().to_string();
    if !record.is_empty() {
        entry.undo_id = Some(record.id.clone());
        UndoLog::push(paths, record)?;
    }
    if entry.done > 0 || entry.failed > 0 {
        Journal::push(paths, entry)?;
    }
    if cancelled {
        return Err(Error::cancelled());
    }
    match downloaded.failed.first() {
        Some(first) if downloaded.files.is_empty() => Err(first.error.clone()),
        _ => Ok(downloaded),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "a7/66/56/a76656e966b1958f568d63c3f1c05aec.jpg";

    fn pin(size: &str) -> String {
        pin_address(size, PIN)
    }

    fn named(url: &str) -> WebImage {
        WebImage {
            url: url.to_owned(),
            preview: url.to_owned(),
            ..WebImage::default()
        }
    }

    #[test]
    fn addresses_are_completed_and_checked() {
        assert_eq!(address(" pin.it/abc ").unwrap(), "https://pin.it/abc");
        assert_eq!(
            address("http://Example.org/a b.png".replace(' ', "%20").as_str()).unwrap(),
            "http://example.org/a%20b.png"
        );
        assert_eq!(
            address("localhost:3000/a.png").unwrap(),
            "https://localhost:3000/a.png"
        );
        for bad in [
            "",
            "   ",
            "two words",
            "ftp://example.org/a.png",
            "file:///c:/a.png",
            "https://",
        ] {
            assert_eq!(address(bad).unwrap_err().code, "badUrl", "{bad}");
        }
    }

    #[test]
    fn pictures_are_told_by_their_first_bytes() {
        assert_eq!(picture(b"\x89PNG\r\n\x1a\n...."), Some(ImageFormat::Png));
        assert_eq!(picture(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageFormat::Jpeg));
        assert_eq!(
            picture(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            Some(ImageFormat::Svg)
        );
        assert_eq!(picture(b"<!DOCTYPE html><html>"), None);
        assert_eq!(picture(b"%PDF-1.7"), None);
        assert_eq!(picture(b"{\"error\":\"denied\"}"), None);
        assert_eq!(picture(b""), None);
    }

    #[test]
    fn pin_addresses_are_split() {
        assert_eq!(
            pin_of(&pin("736x")),
            Some((PIN.to_owned(), PinSize::Wide(736)))
        );
        assert_eq!(
            pin_of(&pin("originals")),
            Some((PIN.to_owned(), PinSize::Original))
        );
        assert_eq!(
            pin_of(&pin("600x315")),
            Some((PIN.to_owned(), PinSize::Cropped))
        );
        assert_eq!(
            pin_of(&pin("75x75_RS")),
            Some((PIN.to_owned(), PinSize::Avatar))
        );
        for other in [
            "https://i.pinimg.com/upload/99360804217844427_board_thumbnail.jpg",
            "https://i.pinimg.com/videos/thumbnails/originals/a7/66/56/a.jpg",
            "https://i.pinimg.com/wide/a7/66/56/a.jpg",
            "https://example.org/736x/a7/66/56/a.jpg",
            "not an address",
        ] {
            assert_eq!(pin_of(other), None, "{other}");
        }
        assert_eq!(light(&pin("736x")), Some(pin("236x")));
        assert_eq!(light(&pin("originals")), Some(pin("236x")));
        assert_eq!(light(&pin("236x")), None);
        assert_eq!(light(&pin("136x136")), None);
        assert_eq!(light("https://example.org/a.jpg"), None);
    }

    #[test]
    fn sizes_of_a_pin_become_one_picture() {
        let other = "e4/e1/fc/e4e1fc7076e459ff87604d5a0b392f77.jpg";
        let mut cover = named(&pin("736x"));
        cover.main = true;
        cover.title = String::from("The pin");
        cover.width = 640;
        cover.height = 478;
        let folded = fold_pins(vec![
            named(&pin("60x60")),
            cover,
            named(&pin("236x")),
            named(&pin("1200x")),
            named("https://example.org/logo.png"),
            named(&pin_address(
                "30x30_RS",
                "dc/24/76/dc2476722ee6e8c12b6a93d8ac5b6a4d.jpg",
            )),
            named(&pin_address("216x146", other)),
            named(&pin("originals")),
        ]);
        assert_eq!(folded.len(), 3);
        let first = &folded[0];
        assert_eq!(first.url, pin("originals"));
        assert_eq!(first.preview, pin("originals"));
        assert!(first.main);
        assert_eq!(first.title, "The pin");
        assert_eq!((first.width, first.height), (0, 0));
        assert_eq!(folded[1].url, "https://example.org/logo.png");
        assert_eq!(folded[2].url, pin_address("originals", other));
        assert_eq!(folded[2].preview, pin_address("216x146", other));

        // Without the original among the sizes the largest one is the spare.
        let folded = fold_pins(vec![
            named(&pin("236x")),
            named(&pin("736x")),
            named(&pin("474x")),
        ]);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].url, pin("originals"));
        assert_eq!(folded[0].preview, pin("736x"));
    }

    #[test]
    fn a_pin_page_gives_the_pin_first() {
        let html = format!(
            r#"<html><head><title>Pin | Pinterest</title>
<meta content="{cover}" name="og:image" property="og:image"/>
<meta content="478" name="og:image:height" property="og:image:height"/>
<meta content="Goodbye Live Tweeting" name="og:title" property="og:title"/>
</head><body><script type="application/json">{{"related":{{"url":"https://i.pinimg.com/236x/e4/e1/fc/e4e1fc7076e459ff87604d5a0b392f77.jpg"}},
"orig":{{"url":"{original}"}},"pinner":{{"image":"https://i.pinimg.com/75x75_RS/dc/24/76/dc2476722ee6e8c12b6a93d8ac5b6a4d.jpg"}}}}</script></body></html>"#,
            cover = pin("736x"),
            original = pin("originals"),
        );
        let address = "https://www.pinterest.com/pin/99360735500167749/";
        let inspected = page(&html, address);
        assert!(!inspected.direct);
        assert_eq!(inspected.title, "Goodbye Live Tweeting");
        assert_eq!(inspected.images.len(), 2);
        let first = &inspected.images[0];
        assert!(first.main);
        assert_eq!(first.url, pin("originals"));
        assert_eq!(first.title, "Goodbye Live Tweeting");
        assert_eq!(first.page, address);
        assert_eq!(first.source, "www.pinterest.com");
        let second = &inspected.images[1];
        assert!(!second.main);
        assert_eq!(
            second.preview,
            "https://i.pinimg.com/236x/e4/e1/fc/e4e1fc7076e459ff87604d5a0b392f77.jpg"
        );
        assert!(is_pinterest(address));
        assert!(is_pinterest("https://pin.it/4vWmQx"));
        assert!(is_pinterest("https://ru.pinterest.com/pin/1/"));
        assert!(!is_pinterest("https://example.org/pinterest/"));
    }

    #[test]
    fn the_spare_way_to_a_pin_is_read() {
        // What `oembed.json` of Pinterest answers, shortened.
        let json: Value = serde_json::from_str(
            r#"{"version":"1.0","type":"rich","provider_name":"Pinterest","title":"Next stop: Pinterest!","author_name":"Kent Brewster","author_url":"https://www.pinterest.com/kentbrew/","thumbnail_url":"https://i.pinimg.com/236x/a7/66/56/a76656e966b1958f568d63c3f1c05aec.jpg","thumbnail_width":236,"thumbnail_height":176}"#,
        )
        .unwrap();
        let address = "https://www.pinterest.com/pin/99360735500167749/";
        let image = pin_embedded(&json, address).unwrap();
        assert_eq!(image.url, pin("236x"));
        assert_eq!(image.title, "Next stop: Pinterest!");
        assert_eq!(image.author, "Kent Brewster");
        assert!(image.main);
        let folded = fold_pins(vec![image]);
        assert_eq!(folded[0].url, pin("originals"));
        assert_eq!(folded[0].preview, pin("236x"));
        assert_eq!(
            pin_embedded(&serde_json::json!({ "title": "x" }), address),
            None
        );
        assert_eq!(
            pin_embedded(
                &serde_json::json!({ "thumbnail_url": "https://example.org/a.jpg" }),
                address
            ),
            None
        );
    }

    #[test]
    fn pages_are_cut_to_what_fits_a_list() {
        let pictures: Vec<String> = (0..MAX_IMAGES + 50)
            .map(|index| format!("<img src=\"/p/{index}.jpg\">"))
            .collect();
        let pictures = pictures.concat();
        let html = format!("{pictures}<meta property=\"og:image\" content=\"/cover.jpg\">");
        let inspected = page(&html, "https://example.org/gallery");
        assert_eq!(inspected.images.len(), MAX_IMAGES);
        assert_eq!(inspected.images[0].url, "https://example.org/cover.jpg");
        assert_eq!(inspected.images[1].url, "https://example.org/p/0.jpg");
        assert!(page("<p>No pictures here</p>", "https://example.org/")
            .images
            .is_empty());
    }

    #[test]
    fn a_direct_link_is_one_picture() {
        let inspected = direct("https://example.org/files/fox.jpg");
        assert!(inspected.direct);
        assert_eq!(inspected.images.len(), 1);
        assert!(inspected.images[0].main);
        assert_eq!(inspected.images[0].source, "example.org");
        // A direct link to a size of a pin still leads to its original.
        let pinned = direct(&pin("474x"));
        assert_eq!(pinned.images[0].url, pin("originals"));
        assert_eq!(pinned.images[0].preview, pin("474x"));
    }

    #[test]
    fn files_are_named_after_the_title_or_the_address() {
        let mut image = named("https://example.org/files/IMG_0042.JPG?size=large");
        assert_eq!(stem_of(&image, &image.url), "IMG_0042");
        image.title = String::from("Red Fox: at dawn?");
        assert_eq!(stem_of(&image, &image.url), "Red Fox_ at dawn_");
        image.title = String::from("Alaska Red Fox (Vulpes vulpes).jpg");
        assert_eq!(
            stem_of(&image, &image.url),
            "Alaska Red Fox (Vulpes vulpes)"
        );
        image.title = String::from("Version 2.0 of the fox");
        assert_eq!(stem_of(&image, &image.url), "Version 2.0 of the fox");
        image.title = "x".repeat(300);
        assert_eq!(stem_of(&image, &image.url).chars().count(), NAME_CHARS);

        let encoded = named("https://example.org/%D0%BB%D0%B8%D1%81%D0%B0%20%231.png");
        assert_eq!(stem_of(&encoded, &encoded.url), "лиса #1");
        let bare = named("https://example.org/");
        assert_eq!(stem_of(&bare, &bare.url), "image");
        let api = named("https://api.openverse.org/v1/images/a05f/thumb/");
        assert_eq!(stem_of(&api, &api.url), "thumb");
        assert_eq!(percent_decoded("100%25%2"), "100%%2");
        assert_eq!(percent_decoded("%zz%"), "%zz%");
    }

    #[test]
    fn bodies_are_saved_whole_or_not_at_all() {
        let folder = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        let part = folder.path().join("a.part");
        let body = vec![5u8; 200_000];
        let mut seen = Vec::new();
        let mut progress = |done: u64, total: u64| seen.push((done, total));
        let bytes = save(
            &part,
            b"head",
            &mut body.as_slice(),
            200_004,
            &mut progress,
            &cancel,
        )
        .unwrap();
        assert_eq!(bytes, 200_004);
        assert_eq!(fs::metadata(&part).unwrap().len(), 200_004);
        assert_eq!(seen.first(), Some(&(4, 200_004)));
        assert_eq!(seen.last(), Some(&(200_004, 200_004)));

        let mut quiet = |_: u64, _: u64| {};
        let short = save(
            &part,
            b"head",
            &mut body.as_slice(),
            300_000,
            &mut quiet,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(short.code, "network");
        let unknown = save(&part, b"", &mut body.as_slice(), 0, &mut quiet, &cancel).unwrap();
        assert_eq!(unknown, 200_000);
        cancel.cancel();
        let stopped = save(&part, b"", &mut body.as_slice(), 0, &mut quiet, &cancel).unwrap_err();
        assert_eq!(stopped.code, "cancelled");
    }

    #[test]
    fn refused_addresses_fail_before_any_request() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let net = Net::new();
        let cancel = CancelToken::new();
        let inside = "http://192.168.1.1/cam.jpg";
        assert_eq!(
            inspect(&net, inside, false, &cancel).unwrap_err().code,
            "localAddress"
        );
        assert_eq!(
            preview(&net, &paths, inside, "", false, &cancel)
                .unwrap_err()
                .code,
            "localAddress"
        );
        let mut quiet = |_: u64, _: u64| {};
        let refused = download(
            &net,
            &named(inside),
            folder.path(),
            false,
            &mut quiet,
            &cancel,
        );
        assert_eq!(refused.unwrap_err().code, "localAddress");
        assert_eq!(
            inspect(&net, "two words", true, &cancel).unwrap_err().code,
            "badUrl"
        );
        assert!(fs::read_dir(folder.path()).unwrap().next().is_none());
    }

    #[test]
    fn failed_downloads_reach_the_journal() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path().join("data"));
        let target = folder.path().join("out");
        let net = Net::new();
        let cancel = CancelToken::new();
        let mut quiet = |_: &Step<'_>| {};
        let nothing = download_all(&net, &paths, "t", &[], &target, false, &mut quiet, &cancel);
        assert_eq!(nothing.unwrap_err().code, "noImages");
        assert!(Journal::load(&paths).entries.is_empty());

        let images = [named("http://10.0.0.5/a.jpg"), named("not an address")];
        let failed = download_all(
            &net, &paths, "Cameras", &images, &target, false, &mut quiet, &cancel,
        );
        assert_eq!(failed.unwrap_err().code, "localAddress");
        let journal = Journal::load(&paths);
        assert_eq!(journal.entries.len(), 1);
        let entry = &journal.entries[0];
        assert_eq!(
            (entry.kind.as_str(), entry.title.as_str()),
            ("download", "Cameras")
        );
        assert_eq!((entry.done, entry.failed), (0, 2));
        assert_eq!(entry.errors[0].path, "http://10.0.0.5/a.jpg");
        assert_eq!(entry.errors[1].error.as_ref().unwrap().code, "badUrl");
        assert_eq!(entry.undo_id, None);
        assert!(!target.exists());

        cancel.cancel();
        let stopped = download_all(
            &net, &paths, "Cameras", &images, &target, false, &mut quiet, &cancel,
        );
        assert!(stopped.unwrap_err().is_cancelled());
        assert_eq!(Journal::load(&paths).entries.len(), 1);
    }

    #[test]
    fn previews_are_kept_and_forgotten() {
        let folder = tempfile::tempdir().unwrap();
        let paths = Paths::at(folder.path());
        let net = Net::new();
        let cancel = CancelToken::new();
        // A preview that is already there is given out without the network.
        let url = "https://example.invalid/fox.jpg";
        let kept = previews(&paths).join(format!("{}.img", net::fingerprint(url)));
        store::write_atomic(&kept, &[0xFF, 0xD8, 0xFF, 0xE0]).unwrap();
        assert_eq!(preview(&net, &paths, url, "", true, &cancel).unwrap(), kept);
        clear_previews(&paths);
        assert!(!kept.exists());
    }
}
