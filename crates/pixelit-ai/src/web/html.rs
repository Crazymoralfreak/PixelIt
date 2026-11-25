//! Finds the pictures of a web page without a browser.
//!
//! The page is read tag by tag: the cover a page names for previews in
//! messengers, the pictures of its text, and the addresses its scripts
//! carry, for pages that draw themselves. Nothing of the page is run.

use std::collections::HashSet;

use serde_json::Value;
use url::Url;

/// A picture with a declared side below this is an icon or a counter.
const MIN_SIDE: u32 = 64;

/// Pictures taken from one page before the repeats of one picture in several
/// sizes are folded together.
const MAX_FOUND: usize = 2000;

/// Longest address read out of a script.
const MAX_ADDRESS: usize = 2048;

/// Longest title kept, in characters.
const MAX_TITLE: usize = 200;

/// How deep linked data is followed.
const MAX_DEPTH: u32 = 16;

/// Extensions that make a link a link to a picture.
const LINKED: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "avif", "bmp", "tif", "tiff", "jxl", "heic", "svg",
];

/// Extensions looked for inside scripts, where most addresses are not
/// pictures; vector drawings there are icons of the interface.
const LOOSE: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "avif"];

/// A picture a page names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// The full address.
    pub url: String,
    /// The text the page gives the picture.
    pub title: String,
    /// The declared size; zero when the page does not tell it.
    pub width: u32,
    pub height: u32,
    /// The picture the page calls its cover.
    pub main: bool,
}

/// What a page tells about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Page {
    pub title: String,
    /// The pictures, covers first, otherwise in the order of the page.
    pub images: Vec<Found>,
}

struct Tag {
    /// Lower-cased.
    name: String,
    /// Names lower-cased, values with their entities replaced.
    attributes: Vec<(String, String)>,
    /// Where the text after the tag starts.
    end: usize,
}

impl Tag {
    /// A non-empty attribute.
    fn get(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
    }
}

/// Reads the tag that starts at `start`, where a `<` is followed by a letter.
fn read_tag(html: &str, start: usize) -> Tag {
    let bytes = html.as_bytes();
    let length = bytes.len();
    let space = |at: usize| at < length && bytes[at].is_ascii_whitespace();
    let mut at = start + 1;
    while at < length && (bytes[at].is_ascii_alphanumeric() || matches!(bytes[at], b'-' | b':')) {
        at += 1;
    }
    let name = html[start + 1..at].to_ascii_lowercase();
    let mut attributes = Vec::new();
    loop {
        while space(at) || (at < length && bytes[at] == b'/') {
            at += 1;
        }
        if at >= length {
            break;
        }
        if bytes[at] == b'>' {
            at += 1;
            break;
        }
        let key_start = at;
        while at < length && !space(at) && !matches!(bytes[at], b'=' | b'>' | b'/') {
            at += 1;
        }
        let key = html[key_start..at].to_ascii_lowercase();
        while space(at) {
            at += 1;
        }
        let mut value = "";
        if at < length && bytes[at] == b'=' {
            at += 1;
            while space(at) {
                at += 1;
            }
            if at < length && matches!(bytes[at], b'"' | b'\'') {
                let quote = bytes[at];
                let value_start = at + 1;
                at = value_start;
                while at < length && bytes[at] != quote {
                    at += 1;
                }
                value = &html[value_start..at];
                at = (at + 1).min(length);
            } else {
                let value_start = at;
                while at < length && !space(at) && bytes[at] != b'>' {
                    at += 1;
                }
                value = &html[value_start..at];
            }
        }
        if !key.is_empty() {
            attributes.push((key, decode(value)));
        }
    }
    Tag {
        name,
        attributes,
        end: at,
    }
}

/// Where the closing tag of an element with raw text starts.
fn find_close(html: &str, from: usize, name: &str) -> usize {
    let mut at = from;
    while let Some(offset) = html[at..].find("</") {
        let start = at + offset;
        let after = &html.as_bytes()[start + 2..];
        if after.len() >= name.len() && after[..name.len()].eq_ignore_ascii_case(name.as_bytes()) {
            return start;
        }
        at = start + 2;
    }
    html.len()
}

fn entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        _ => {
            let code = name.strip_prefix('#')?;
            let number = match code.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => code.parse().ok()?,
            };
            char::from_u32(number)
        }
    }
}

/// Replaces the character entities of HTML text.
pub fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest
            .char_indices()
            .take(12)
            .find(|&(_, character)| character == ';')
            .map(|(index, _)| index);
        let known = end.and_then(|end| Some((end, entity(&rest[1..end])?)));
        if let Some((end, character)) = known {
            result.push(character);
            rest = &rest[end + 1..];
        } else {
            result.push('&');
            rest = &rest[1..];
        }
    }
    result.push_str(rest);
    result
}

/// Text as a person reads it: entities replaced, runs of spaces folded.
fn clean(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    decode(&words.join(" "))
        .trim()
        .chars()
        .take(MAX_TITLE)
        .collect()
}

/// The text of a piece of HTML without its tags.
pub fn text_of(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut inside = false;
    for character in html.chars() {
        match character {
            '<' => inside = true,
            '>' if inside => {
                inside = false;
                text.push(' ');
            }
            _ if !inside => text.push(character),
            _ => {}
        }
    }
    clean(&text)
}

/// A declared side in pixels; a share of the parent tells nothing.
fn side(value: Option<&str>) -> u32 {
    let Some(value) = value else {
        return 0;
    };
    if value.ends_with('%') {
        return 0;
    }
    let digits: String = value
        .chars()
        .take_while(char::is_ascii_digit)
        .take(6)
        .collect();
    digits.parse().unwrap_or(0)
}

/// The largest variant of a `srcset` list.
fn largest(srcset: &str) -> Option<&str> {
    let gap = |character: char| character.is_ascii_whitespace() || character == ',';
    let mut best: Option<(&str, f64)> = None;
    let mut rest = srcset;
    loop {
        rest = rest.trim_start_matches(gap);
        if rest.is_empty() {
            break;
        }
        let end = rest
            .find(|character: char| character.is_ascii_whitespace())
            .unwrap_or(rest.len());
        let mut address = &rest[..end];
        rest = &rest[end..];
        let mut weight = 1.0;
        if address.ends_with(',') {
            address = address.trim_end_matches(',');
        } else {
            let stop = rest.find(',').unwrap_or(rest.len());
            let descriptor = rest[..stop].trim();
            rest = &rest[stop..];
            // `640w` and `2x` never share a list, so the plain numbers compare.
            weight = descriptor
                .trim_end_matches(|character: char| character.is_ascii_alphabetic())
                .parse()
                .unwrap_or(1.0);
        }
        if !address.is_empty() && best.is_none_or(|(_, known)| weight > known) {
            best = Some((address, weight));
        }
    }
    best.map(|(address, _)| address)
}

/// Whether the file an address names has one of the extensions.
fn has_extension(address: &Url, extensions: &[&str]) -> bool {
    address
        .path()
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, extension)| extensions.contains(&extension.to_ascii_lowercase().as_str()))
}

/// An address as it stands inside a script: slashes and ampersands escaped,
/// sometimes twice.
fn unescape(raw: &str) -> String {
    let mut text = raw.to_owned();
    for (escaped, plain) in [
        ("\\u002F", "/"),
        ("\\u002f", "/"),
        ("\\u0026", "&"),
        ("\\u003D", "="),
        ("\\u003d", "="),
        ("&amp;", "&"),
    ] {
        if text.contains(escaped) {
            text = text.replace(escaped, plain);
        }
    }
    // An address has no backslashes of its own.
    text.retain(|character| character != '\\');
    text
}

fn number(value: Option<&Value>) -> u32 {
    match value {
        Some(Value::Number(number)) => number.as_u64().map_or(0, |side| side.min(1 << 20) as u32),
        Some(Value::String(text)) => side(Some(text.trim())),
        _ => 0,
    }
}

struct Collector {
    /// What relative addresses are joined with.
    base: Option<Url>,
    title: String,
    /// The title the page gives for previews in messengers.
    cover_title: String,
    images: Vec<Found>,
    seen: HashSet<String>,
    /// The cover whose size the following tags may tell.
    cover: Option<usize>,
    /// Addresses met inside scripts; they go after the pictures of the tags.
    loose: Vec<String>,
}

impl Collector {
    fn new(address: &str) -> Self {
        Self {
            base: Url::parse(address).ok(),
            title: String::new(),
            cover_title: String::new(),
            images: Vec::new(),
            seen: HashSet::new(),
            cover: None,
            loose: Vec::new(),
        }
    }

    /// The full form of an address of the page, when it is a web address.
    fn resolve(&self, address: &str) -> Option<Url> {
        let address = address.trim();
        if address.is_empty() || address.len() > MAX_ADDRESS {
            return None;
        }
        let mut full = match &self.base {
            Some(base) => base.join(address).ok()?,
            None => Url::parse(address).ok()?,
        };
        if !matches!(full.scheme(), "http" | "https") || full.host_str().is_none() {
            return None;
        }
        full.set_fragment(None);
        Some(full)
    }

    /// Takes a picture; returns whether the address is one the page can be
    /// asked for, new or already known.
    fn add(&mut self, address: &str, title: &str, width: u32, height: u32, main: bool) -> bool {
        let Some(full) = self.resolve(address) else {
            return false;
        };
        self.keep(full, title, width, height, main)
    }

    fn keep(&mut self, full: Url, title: &str, width: u32, height: u32, main: bool) -> bool {
        let small = |side: u32| side > 0 && side < MIN_SIDE;
        if !main && (small(width) || small(height)) {
            return true;
        }
        let url = String::from(full);
        if self.seen.contains(&url) {
            if main {
                if let Some(index) = self.images.iter().position(|known| known.url == url) {
                    self.images[index].main = true;
                    self.cover = Some(index);
                }
            }
            return true;
        }
        if self.images.len() >= MAX_FOUND {
            return true;
        }
        self.seen.insert(url.clone());
        if main {
            self.cover = Some(self.images.len());
        }
        self.images.push(Found {
            url,
            title: clean(title),
            width,
            height,
            main,
        });
        true
    }

    fn full(&self) -> bool {
        self.images.len() >= MAX_FOUND
    }

    fn meta(&mut self, tag: &Tag) {
        let Some(content) = tag.get("content") else {
            return;
        };
        let key = tag
            .get("property")
            .or_else(|| tag.get("name"))
            .or_else(|| tag.get("itemprop"))
            .unwrap_or_default()
            .to_ascii_lowercase();
        match key.as_str() {
            "og:image"
            | "og:image:url"
            | "og:image:secure_url"
            | "twitter:image"
            | "twitter:image:src"
            | "image" => {
                self.add(content, "", 0, 0, true);
            }
            "og:image:width" => {
                if let Some(cover) = self.cover.and_then(|index| self.images.get_mut(index)) {
                    cover.width = side(Some(content));
                }
            }
            "og:image:height" => {
                if let Some(cover) = self.cover.and_then(|index| self.images.get_mut(index)) {
                    cover.height = side(Some(content));
                }
            }
            "og:title" | "twitter:title" if self.cover_title.is_empty() => {
                self.cover_title = clean(content);
            }
            _ => {}
        }
    }

    fn link(&mut self, tag: &Tag) {
        let rel = tag.get("rel").unwrap_or_default().to_ascii_lowercase();
        let is = |name: &str| rel.split_ascii_whitespace().any(|part| part == name);
        if is("image_src") {
            if let Some(address) = tag.get("href") {
                self.add(address, "", 0, 0, true);
            }
        } else if is("preload")
            && tag
                .get("as")
                .is_some_and(|kind| kind.eq_ignore_ascii_case("image"))
        {
            let address = tag
                .get("imagesrcset")
                .and_then(largest)
                .or_else(|| tag.get("href"));
            if let Some(address) = address {
                self.add(address, "", 0, 0, false);
            }
        }
    }

    fn img(&mut self, tag: &Tag) {
        let width = side(tag.get("width"));
        let height = side(tag.get("height"));
        let title = tag.get("alt").or_else(|| tag.get("title")).unwrap_or("");
        // A page that loads pictures as they scroll into view keeps the real
        // address in a `data-` attribute and a stand-in in `src`.
        let lists = ["srcset", "data-srcset", "data-lazy-srcset"]
            .into_iter()
            .filter_map(|name| tag.get(name).and_then(largest));
        let singles = ["data-src", "data-original", "data-lazy-src", "src"]
            .into_iter()
            .filter_map(|name| tag.get(name));
        for address in lists.chain(singles) {
            if self.add(address, title, width, height, false) {
                return;
            }
        }
    }

    fn tag(&mut self, tag: &Tag) {
        match tag.name.as_str() {
            "meta" => self.meta(tag),
            "link" => self.link(tag),
            "img" | "amp-img" => self.img(tag),
            "base" => {
                if let Some(base) = tag.get("href").and_then(|address| self.resolve(address)) {
                    self.base = Some(base);
                }
            }
            "source" => {
                if let Some(address) = tag.get("srcset").and_then(largest) {
                    self.add(address, "", 0, 0, false);
                }
            }
            "video" => {
                if let Some(address) = tag.get("poster") {
                    self.add(address, "", 0, 0, false);
                }
            }
            "a" => {
                // A wiki links every picture to a page about it, named
                // `File:picture.jpg`: a page, not a picture.
                let linked = tag
                    .get("href")
                    .and_then(|address| self.resolve(address))
                    .filter(|address| has_extension(address, LINKED))
                    .filter(|address| {
                        !address
                            .path()
                            .rsplit('/')
                            .next()
                            .unwrap_or("")
                            .contains(':')
                    });
                if let Some(address) = linked {
                    self.keep(address, tag.get("title").unwrap_or(""), 0, 0, false);
                }
            }
            _ => {}
        }
    }

    /// Follows linked data (JSON-LD) to the pictures it names.
    fn linked(&mut self, value: &Value, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        match value {
            Value::Array(list) => {
                for item in list {
                    self.linked(item, depth + 1);
                }
            }
            Value::Object(map) => {
                for (key, item) in map {
                    if key == "image" {
                        self.linked_image(item, depth + 1);
                    } else {
                        self.linked(item, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }

    fn linked_image(&mut self, value: &Value, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        match value {
            Value::String(address) => {
                self.add(address, "", 0, 0, false);
            }
            Value::Array(list) => {
                for item in list {
                    self.linked_image(item, depth + 1);
                }
            }
            Value::Object(map) => {
                let address = map
                    .get("url")
                    .or_else(|| map.get("contentUrl"))
                    .and_then(Value::as_str);
                if let Some(address) = address {
                    let width = number(map.get("width"));
                    let height = number(map.get("height"));
                    self.add(address, "", width, height, false);
                }
            }
            _ => {}
        }
    }

    /// Notes the addresses of pictures a script carries as text.
    fn script(&mut self, tag: &Tag, body: &str) {
        let kind = tag.get("type").unwrap_or_default().to_ascii_lowercase();
        if kind == "application/ld+json" {
            if let Ok(value) = serde_json::from_str::<Value>(body.trim()) {
                self.linked(&value, 0);
            }
            return;
        }
        let stop = |character: char| {
            character.is_whitespace() || matches!(character, '"' | '\'' | '<' | '>' | '`' | ')')
        };
        let mut at = 0;
        while let Some(offset) = body[at..].find("http") {
            if self.loose.len() >= MAX_FOUND {
                return;
            }
            let rest = &body[at + offset..];
            let end = rest.find(stop).unwrap_or(rest.len());
            at += offset + end.max(4);
            let raw = &rest[..end];
            if raw.len() > MAX_ADDRESS {
                continue;
            }
            let lower = raw.to_ascii_lowercase();
            // Most addresses of a script are not pictures: a cheap look first.
            let promising = LOOSE.iter().any(|extension| {
                lower
                    .match_indices(extension)
                    .any(|(found, _)| lower[..found].ends_with('.'))
            });
            if !promising {
                continue;
            }
            let address = unescape(raw);
            let picture = Url::parse(&address).is_ok_and(|parsed| has_extension(&parsed, LOOSE));
            if picture {
                self.loose.push(address);
            }
        }
    }

    fn finish(mut self) -> Page {
        for address in std::mem::take(&mut self.loose) {
            self.add(&address, "", 0, 0, false);
        }
        // Stable: covers first, the rest in the order of the page.
        self.images.sort_by_key(|image| !image.main);
        let title = if self.cover_title.is_empty() {
            self.title
        } else {
            self.cover_title
        };
        Page {
            title,
            images: self.images,
        }
    }
}

/// Reads a page; `address` is where it was loaded from.
pub fn scan(html: &str, address: &str) -> Page {
    let bytes = html.as_bytes();
    let mut collector = Collector::new(address);
    let mut at = 0;
    while let Some(offset) = html[at..].find('<') {
        let start = at + offset;
        if html[start..].starts_with("<!--") {
            at = html[start..]
                .find("-->")
                .map_or(html.len(), |end| start + end + 3);
            continue;
        }
        if !bytes.get(start + 1).is_some_and(u8::is_ascii_alphabetic) {
            // A closing tag, a declaration or a stray sign.
            at = start + 1;
            continue;
        }
        let tag = read_tag(html, start);
        at = tag.end;
        match tag.name.as_str() {
            name @ ("script" | "style" | "title" | "textarea") => {
                let close = find_close(html, at, name);
                let body = &html[at..close];
                at = close;
                match name {
                    "title" if collector.title.is_empty() => collector.title = clean(body),
                    "script" => collector.script(&tag, body),
                    _ => {}
                }
            }
            _ => collector.tag(&tag),
        }
        if collector.full() {
            break;
        }
    }
    collector.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(page: &Page) -> Vec<&str> {
        page.images.iter().map(|image| image.url.as_str()).collect()
    }

    #[test]
    fn an_ordinary_page_gives_its_cover_and_pictures() {
        let html = r#"<!DOCTYPE html>
<html><head>
<title>  Foxes &amp; friends
  </title>
<meta property="og:title" content="Foxes of the North">
<meta property="og:image" content="/covers/fox.jpg?size=big&amp;v=2">
<meta property="og:image:width" content="1200">
<meta property="og:image:height" content="630">
<link rel="icon" href="/favicon.png">
<link rel="stylesheet" href="/site.css">
</head><body>
<!-- <img src="/hidden.jpg"> -->
<img src="/pictures/one.jpg" alt="A fox in the snow" width="800" height="600">
<IMG SRC='two.png' alt=Second>
<img src="/pixel.gif" width="1" height="1">
<img src="/icons/star.png" width="32">
<img src="data:image/png;base64,AAAA">
<img src="/pictures/one.jpg">
<a href="/files/poster.TIFF" title="Poster">poster</a>
<a href="/about.html">about</a>
<a href="/wiki/File:Fox.jpg">about the picture</a>
<video poster="//cdn.example.org/poster.webp"></video>
<img src="javascript:alert(1)">
</body></html>"#;
        let page = scan(html, "https://example.org/articles/foxes/");
        assert_eq!(page.title, "Foxes of the North");
        assert_eq!(
            urls(&page),
            [
                "https://example.org/covers/fox.jpg?size=big&v=2",
                "https://example.org/pictures/one.jpg",
                "https://example.org/articles/foxes/two.png",
                "https://example.org/files/poster.TIFF",
                "https://cdn.example.org/poster.webp",
            ]
        );
        let cover = &page.images[0];
        assert!(cover.main);
        assert_eq!((cover.width, cover.height), (1200, 630));
        let first = &page.images[1];
        assert!(!first.main);
        assert_eq!(first.title, "A fox in the snow");
        assert_eq!((first.width, first.height), (800, 600));
        assert_eq!(page.images[2].title, "Second");
        assert_eq!(page.images[3].title, "Poster");
    }

    #[test]
    fn the_title_of_the_page_is_used_without_a_cover_title() {
        let page = scan(
            "<title>Plain &quot;page&quot;</title><img src=a.jpg>",
            "https://example.org/",
        );
        assert_eq!(page.title, "Plain \"page\"");
        assert_eq!(urls(&page), ["https://example.org/a.jpg"]);
        assert!(!page.images[0].main);
    }

    #[test]
    fn the_largest_variant_of_a_set_is_taken() {
        assert_eq!(
            largest("small.jpg 320w, large.jpg 1280w, medium.jpg 640w"),
            Some("large.jpg")
        );
        assert_eq!(largest("one.jpg, two.jpg 2x"), Some("two.jpg"));
        assert_eq!(largest("only.jpg"), Some("only.jpg"));
        assert_eq!(
            largest("https://cdn.example.org/w_300,h_200/a.jpg 300w, https://cdn.example.org/w_900,h_600/a.jpg 900w"),
            Some("https://cdn.example.org/w_900,h_600/a.jpg")
        );
        assert_eq!(largest("  ,  "), None);

        let html = r#"<picture>
<source type="image/webp" srcset="/img/fox-400.webp 400w, /img/fox-1600.webp 1600w">
<img src="/img/fox-400.jpg" srcset="/img/fox-400.jpg 400w, /img/fox-1600.jpg 1600w, /img/fox-800.jpg 800w" alt="Fox">
</picture>
<img src="data:image/gif;base64,R0lGOD" data-src="/img/lazy.jpg">
<noscript><img src="/img/fallback.jpg"></noscript>
<link rel="preload" as="image" href="/img/hero-small.jpg" imagesrcset="/img/hero-small.jpg 1x, /img/hero-big.jpg 2x">"#;
        let page = scan(html, "https://example.org/");
        assert_eq!(
            urls(&page),
            [
                "https://example.org/img/fox-1600.webp",
                "https://example.org/img/fox-1600.jpg",
                "https://example.org/img/lazy.jpg",
                "https://example.org/img/fallback.jpg",
                "https://example.org/img/hero-big.jpg",
            ]
        );
    }

    #[test]
    fn relative_addresses_follow_the_base_of_the_page() {
        let html = r#"<base href="https://static.example.net/site/">
<img src="a.jpg"><img src="../b.jpg"><img src="/c.jpg"><img src="//other.example.com/d.jpg#top">
<img src="https://example.org/e.jpg">"#;
        let page = scan(html, "https://example.org/deep/page.html");
        assert_eq!(
            urls(&page),
            [
                "https://static.example.net/site/a.jpg",
                "https://static.example.net/b.jpg",
                "https://static.example.net/c.jpg",
                "https://other.example.com/d.jpg",
                "https://example.org/e.jpg",
            ]
        );
    }

    #[test]
    fn linked_data_and_scripts_are_read() {
        let html = r#"<script type="application/ld+json">
{"@context":"https://schema.org","@type":"Article","image":["https://example.org/ld/one.jpg",
{"@type":"ImageObject","url":"https://example.org/ld/two.jpg","width":1200,"height":"800"}],
"author":{"@type":"Person","image":{"contentUrl":"https://example.org/ld/author.jpg","width":48}}}
</script>
<script>window.__STATE__ = {"cover":"https:\/\/cdn.example.org\/state\/cover.jpg?w=10\u0026h=20",
"page":"https://example.org/next.html","api":"https://example.org/api/v1","twice":"https:\\/\\/cdn.example.org\\/state\\/twice.png\\"};
var style = "url(https://cdn.example.org/state/back.webp)";</script>
<style>.a { background: url(https://example.org/style.jpg) }</style>
<img src="/tag.jpg">"#;
        let page = scan(html, "https://example.org/");
        assert_eq!(
            urls(&page),
            [
                "https://example.org/ld/one.jpg",
                "https://example.org/ld/two.jpg",
                "https://example.org/tag.jpg",
                "https://cdn.example.org/state/cover.jpg?w=10&h=20",
                "https://cdn.example.org/state/twice.png",
                "https://cdn.example.org/state/back.webp",
            ]
        );
        assert_eq!((page.images[1].width, page.images[1].height), (1200, 800));
    }

    #[test]
    fn a_pin_names_its_picture_in_the_head_only() {
        // The shape of a Pinterest pin page: no `img` tags at all.
        let html = r#"<!DOCTYPE html><html><head><title>Goodbye Live Tweeting | Pinterest</title>
<meta content="Pinterest" property="og:site_name"/>
<meta content="https://i.pinimg.com/736x/a7/66/56/a76656e966b1958f568d63c3f1c05aec.jpg" data-app="true" name="og:image" property="og:image"/>
<meta content="478" data-app="true" name="og:image:height" property="og:image:height"/>
<meta content="640" data-app="true" name="og:image:width" property="og:image:width"/>
<meta content="https://i.pinimg.com/736x/a7/66/56/a76656e966b1958f568d63c3f1c05aec.jpg" data-app="true" name="twitter:image:src" property="twitter:image:src"/>
<meta content="Goodbye Live Tweeting, Hello Live Pinning" data-app="true" name="og:title" property="og:title"/>
<link as="image" href="https://i.pinimg.com/736x/a7/66/56/a76656e966b1958f568d63c3f1c05aec.jpg" rel="preload"/>
<style>:root{--instagram-background-url:url(https://i.pinimg.com/originals/d5/3b/01/d53b014d86a6b6761bf649a0ed813c2b.png)}</style>
</head><body><div id="__PWS_ROOT__"></div>
<script id="__PWS_INITIAL_PROPS__" type="application/json">{"pin":{"images_236x":{"url":"https://i.pinimg.com/236x/a7/66/56/a76656e966b1958f568d63c3f1c05aec.jpg","height":176,"width":236},
"images_orig":{"url":"https://i.pinimg.com/originals/a7/66/56/a76656e966b1958f568d63c3f1c05aec.jpg","height":478,"width":640},
"pinner":{"imageSmallUrl":"https://i.pinimg.com/30x30_RS/dc/24/76/dc2476722ee6e8c12b6a93d8ac5b6a4d.jpg"},
"board":{"coverImageSpec_236x":{"url":"https://i.pinimg.com/236x/e4/e1/fc/e4e1fc7076e459ff87604d5a0b392f77.jpg"}}}}</script>
</body></html>"#;
        let page = scan(html, "https://www.pinterest.com/pin/99360735500167749/");
        assert_eq!(page.title, "Goodbye Live Tweeting, Hello Live Pinning");
        assert_eq!(
            urls(&page),
            [
                "https://i.pinimg.com/736x/a7/66/56/a76656e966b1958f568d63c3f1c05aec.jpg",
                "https://i.pinimg.com/236x/a7/66/56/a76656e966b1958f568d63c3f1c05aec.jpg",
                "https://i.pinimg.com/originals/a7/66/56/a76656e966b1958f568d63c3f1c05aec.jpg",
                "https://i.pinimg.com/30x30_RS/dc/24/76/dc2476722ee6e8c12b6a93d8ac5b6a4d.jpg",
                "https://i.pinimg.com/236x/e4/e1/fc/e4e1fc7076e459ff87604d5a0b392f77.jpg",
            ]
        );
        assert!(page.images[0].main);
        assert_eq!((page.images[0].width, page.images[0].height), (640, 478));
        assert!(page.images[1..].iter().all(|image| !image.main));
    }

    #[test]
    fn broken_markup_does_not_stop_the_reading() {
        for html in [
            "<",
            "<img",
            "<img src=",
            "<img src=\"a.jpg",
            "<script>var a = 'https://example.org/a.jpg",
            "<!-- never closed <img src=a.jpg>",
            "<a href=>< img src=a.jpg><=><img =a src=b.jpg>",
            "<title>Заголовок без конца",
            "<img alt=\"é\" src=\"ü.jpg\">&#x1F98A;&unknown;&",
        ] {
            let page = scan(html, "https://example.org/");
            assert!(page.images.len() <= 1, "{html}");
        }
        assert!(scan("<img src=a.jpg>", "not an address").images.is_empty());
        assert_eq!(
            decode("a &amp; b &#39;c&#x27; &nope; &"),
            "a & b 'c' &nope; &"
        );
    }

    #[test]
    fn tags_are_stripped_from_text() {
        assert_eq!(
            text_of("<a rel=\"nofollow\" href=\"https://x\">Gregory \"Slobirdr\"   Smith</a>"),
            "Gregory \"Slobirdr\" Smith"
        );
        assert_eq!(text_of("Plain &amp; simple"), "Plain & simple");
        assert_eq!(text_of(""), "");
    }
}
