//! Picture search in catalogues that ask for no account and no key:
//! Openverse and Wikimedia Commons. Both tell the author and the licence of
//! every picture, and both are kept with what is found.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use pixelit_core::{Error, Result};

use super::{html, WebImage};
use crate::net::Net;

/// Pictures asked for at a time; the most Openverse gives without an account.
pub const PAGE_SIZE: u32 = 20;

/// Longest query sent.
const MAX_QUERY: usize = 200;

/// The last page asked for; both catalogues stop long before it.
const MAX_PAGE: u32 = 500;

const TIMEOUT: Duration = Duration::from_secs(30);

const OPENVERSE: &str = "https://api.openverse.org/v1/images/";
const COMMONS: &str = "https://commons.wikimedia.org/w/api.php";

/// Kinds of Commons files PixelIt opens; the catalogue also holds sound,
/// video and documents.
const COMMONS_KINDS: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/tiff",
    "image/bmp",
    "image/avif",
    "image/svg+xml",
];

/// A catalogue of pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WebSource {
    Openverse,
    Commons,
}

impl WebSource {
    pub const ALL: &'static [Self] = &[Self::Openverse, Self::Commons];

    /// Stable lower-case name, equal to the serialized value.
    pub fn name(self) -> &'static str {
        match self {
            Self::Openverse => "openverse",
            Self::Commons => "commons",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|source| source.name().eq_ignore_ascii_case(name.trim()))
    }

    /// The host that receives the query.
    pub fn host(self) -> &'static str {
        match self {
            Self::Openverse => "api.openverse.org",
            Self::Commons => "commons.wikimedia.org",
        }
    }
}

/// One page of search results.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub images: Vec<WebImage>,
    /// Whether the next page has something.
    pub more: bool,
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().trim().to_owned()
}

fn side(value: &Value) -> u32 {
    value.as_u64().map_or(0, |side| side.min(1 << 20) as u32)
}

/// The address a query is sent to; `page` starts at 1.
fn address(source: WebSource, query: &str, page: u32) -> Result<String> {
    let page = page.clamp(1, MAX_PAGE);
    let number = page.to_string();
    let size = PAGE_SIZE.to_string();
    let offset = ((page - 1) * PAGE_SIZE).to_string();
    let built = match source {
        WebSource::Openverse => Url::parse_with_params(
            OPENVERSE,
            &[
                ("q", query),
                ("page", number.as_str()),
                ("page_size", size.as_str()),
            ],
        ),
        WebSource::Commons => Url::parse_with_params(
            COMMONS,
            &[
                ("action", "query"),
                ("format", "json"),
                ("formatversion", "2"),
                ("generator", "search"),
                ("gsrsearch", query),
                // The namespace of files.
                ("gsrnamespace", "6"),
                ("gsrlimit", size.as_str()),
                ("gsroffset", offset.as_str()),
                ("prop", "imageinfo"),
                ("iiprop", "url|size|mime|extmetadata"),
                ("iiurlwidth", "400"),
                ("iiextmetadatafilter", "Artist|LicenseShortName|LicenseUrl"),
            ],
        ),
    };
    built.map(String::from).map_err(|error| {
        Error::new("badUrl")
            .with("url", source.host())
            .detail(error)
    })
}

/// The name of a Creative Commons licence as people write it.
fn license_name(code: &str, version: &str) -> String {
    let name = match code {
        "" => return String::new(),
        "pdm" => String::from("Public Domain Mark"),
        "cc0" => String::from("CC0"),
        other => format!("CC {}", other.to_ascii_uppercase()),
    };
    if version.is_empty() {
        name
    } else {
        format!("{name} {version}")
    }
}

fn openverse(json: &Value, page: u32) -> SearchPage {
    let images = json["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let url = text(&item["url"]);
            if url.is_empty() {
                return None;
            }
            Some(WebImage {
                preview: text(&item["thumbnail"]),
                title: text(&item["title"]),
                width: side(&item["width"]),
                height: side(&item["height"]),
                page: text(&item["foreign_landing_url"]),
                author: text(&item["creator"]),
                license: license_name(&text(&item["license"]), &text(&item["license_version"])),
                license_url: text(&item["license_url"]),
                source: String::from(WebSource::Openverse.name()),
                main: false,
                url,
            })
        })
        .collect();
    SearchPage {
        images,
        more: u64::from(page.max(1)) < json["page_count"].as_u64().unwrap_or(0),
    }
}

/// Wikimedia marks the addresses it hands out with where they were taken
/// from; the marks are not part of the address of a file.
fn without_marks(address: &str) -> String {
    let Ok(mut parsed) = Url::parse(address) else {
        return address.to_owned();
    };
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(name, _)| !name.starts_with("utm_"))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.query_pairs_mut().clear().extend_pairs(kept);
    }
    parsed.into()
}

fn commons(json: &Value) -> SearchPage {
    let mut pages: Vec<&Value> = json["query"]["pages"]
        .as_array()
        .map_or_default(|pages| pages.iter().collect());
    // The pages come in the order of their numbers, not of the search.
    pages.sort_by_key(|page| page["index"].as_u64().unwrap_or(u64::MAX));
    let images = pages
        .into_iter()
        .filter_map(|page| {
            let info = &page["imageinfo"][0];
            let url = text(&info["url"]);
            let kind = text(&info["mime"]);
            if url.is_empty() || !COMMONS_KINDS.contains(&kind.as_str()) {
                return None;
            }
            let title = text(&page["title"]);
            let about = |name: &str| html::text_of(&text(&info["extmetadata"][name]["value"]));
            Some(WebImage {
                url: without_marks(&url),
                preview: without_marks(&text(&info["thumburl"])),
                title: title.strip_prefix("File:").unwrap_or(&title).to_owned(),
                width: side(&info["width"]),
                height: side(&info["height"]),
                page: text(&info["descriptionurl"]),
                author: about("Artist"),
                license: about("LicenseShortName"),
                license_url: text(&info["extmetadata"]["LicenseUrl"]["value"]),
                source: String::from(WebSource::Commons.name()),
                main: false,
            })
        })
        .collect();
    SearchPage {
        images,
        more: json["continue"]["gsroffset"].is_number(),
    }
}

/// Asks a catalogue for pictures; `page` starts at 1. Only the text of the
/// query leaves the computer.
pub fn search(net: &Net, source: WebSource, query: &str, page: u32) -> Result<SearchPage> {
    let query: String = query.trim().chars().take(MAX_QUERY).collect();
    if query.is_empty() {
        return Err(Error::new("emptyQuery"));
    }
    let json = net.get_json(&address(source, &query, page)?, None, Some(TIMEOUT))?;
    Ok(match source {
        WebSource::Openverse => openverse(&json, page),
        WebSource::Commons => commons(&json),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shortened answer of Openverse, saved from the live service.
    const OPENVERSE_ANSWER: &str = r#"{"result_count":240,"page_count":80,"page_size":3,"page":1,"results":[
{"id":"a05f2b1b-6703-47f8-b788-5f12a5e53829","title":"Red Fox","foreign_landing_url":"https://www.flickr.com/photos/28004047@N04/5738555148","url":"https://live.staticflickr.com/5306/5738555148_e8e2f83f14_b.jpg","creator":"-Category 5-","creator_url":"https://www.flickr.com/photos/28004047@N04","license":"by-nc-nd","license_version":"2.0","license_url":"https://creativecommons.org/licenses/by-nc-nd/2.0/","provider":"flickr","source":"flickr","category":null,"filesize":null,"filetype":null,"mature":false,"height":685,"width":1024,"thumbnail":"https://api.openverse.org/v1/images/a05f2b1b-6703-47f8-b788-5f12a5e53829/thumb/"},
{"id":"728834f0-247a-497a-97b4-c5e1ef68a4a9","title":"Red fox hunting near Mud Volcano","foreign_landing_url":"https://www.flickr.com/photos/80223459@N05/26037387548","url":"https://live.staticflickr.com/4754/26037387548_9b66d1b063_b.jpg","creator":"YellowstoneNPS","license":"pdm","license_version":"1.0","license_url":"https://creativecommons.org/publicdomain/mark/1.0/","mature":false,"height":683,"width":1024,"thumbnail":"https://api.openverse.org/v1/images/728834f0-247a-497a-97b4-c5e1ef68a4a9/thumb/"},
{"id":"no-address","title":"Lost","url":null}]}"#;

    /// A shortened answer of Wikimedia Commons, saved from the live service.
    const COMMONS_ANSWER: &str = r#"{"batchcomplete":true,"continue":{"gsroffset":3,"continue":"gsroffset||"},"query":{"pages":[
{"pageid":361,"ns":6,"title":"File:Vulpes vulpes laying in snow.jpg","index":2,"imageinfo":[{"size":579242,"width":3072,"height":2040,"thumburl":"https://thumb.wikimedia.org/wikipedia/commons/thumb/0/03/Vulpes_vulpes_laying_in_snow.jpg/500px-Vulpes_vulpes_laying_in_snow.jpg?utm_source=commons.wikimedia.org&utm_campaign=imageinfo&utm_content=thumbnail","url":"https://upload.wikimedia.org/wikipedia/commons/0/03/Vulpes_vulpes_laying_in_snow.jpg?utm_source=commons.wikimedia.org&utm_campaign=imageinfo&utm_content=original","descriptionurl":"https://commons.wikimedia.org/wiki/File:Vulpes_vulpes_laying_in_snow.jpg","extmetadata":{"Artist":{"value":"Shiretoko-Shari Tourist Association","source":"commons-desc-page"},"LicenseShortName":{"value":"Attribution","source":"commons-desc-page","hidden":""}},"mime":"image/jpeg"}]},
{"pageid":40573988,"ns":6,"title":"File:Alaska Red Fox (Vulpes vulpes).jpg","index":1,"imageinfo":[{"size":7306999,"width":3805,"height":2750,"thumburl":"https://thumb.wikimedia.org/wikipedia/commons/thumb/8/8e/Alaska_Red_Fox_%28Vulpes_vulpes%29.jpg/500px-Alaska_Red_Fox_%28Vulpes_vulpes%29.jpg?utm_source=commons.wikimedia.org&utm_campaign=imageinfo&utm_content=thumbnail","url":"https://upload.wikimedia.org/wikipedia/commons/8/8e/Alaska_Red_Fox_%28Vulpes_vulpes%29.jpg?utm_source=commons.wikimedia.org&utm_campaign=imageinfo&utm_content=original","descriptionurl":"https://commons.wikimedia.org/wiki/File:Alaska_Red_Fox_(Vulpes_vulpes).jpg","extmetadata":{"Artist":{"value":"<a rel=\"nofollow\" class=\"external text\" href=\"https://www.flickr.com/people/22170893@N06\">Gregory \"Slobirdr\" Smith</a>","source":"commons-desc-page"},"LicenseShortName":{"value":"CC BY-SA 2.0","source":"commons-desc-page","hidden":""},"LicenseUrl":{"value":"https://creativecommons.org/licenses/by-sa/2.0","source":"commons-desc-page"}},"mime":"image/jpeg"}]},
{"pageid":77,"ns":6,"title":"File:Fox call.ogg","index":3,"imageinfo":[{"url":"https://upload.wikimedia.org/wikipedia/commons/7/77/Fox_call.ogg","mime":"application/ogg"}]},
{"pageid":78,"ns":6,"title":"File:Missing.jpg","index":4}]}}"#;

    #[test]
    fn openverse_answers_are_read() {
        let json: Value = serde_json::from_str(OPENVERSE_ANSWER).unwrap();
        let page = openverse(&json, 1);
        assert!(page.more);
        assert!(!openverse(&json, 80).more);
        assert_eq!(page.images.len(), 2);
        let first = &page.images[0];
        assert_eq!(
            first.url,
            "https://live.staticflickr.com/5306/5738555148_e8e2f83f14_b.jpg"
        );
        assert_eq!(
            first.preview,
            "https://api.openverse.org/v1/images/a05f2b1b-6703-47f8-b788-5f12a5e53829/thumb/"
        );
        assert_eq!(first.title, "Red Fox");
        assert_eq!((first.width, first.height), (1024, 685));
        assert_eq!(
            first.page,
            "https://www.flickr.com/photos/28004047@N04/5738555148"
        );
        assert_eq!(first.author, "-Category 5-");
        assert_eq!(first.license, "CC BY-NC-ND 2.0");
        assert_eq!(
            first.license_url,
            "https://creativecommons.org/licenses/by-nc-nd/2.0/"
        );
        assert_eq!(first.source, "openverse");
        assert_eq!(page.images[1].license, "Public Domain Mark 1.0");
        assert_eq!(openverse(&Value::Null, 1), SearchPage::default());
    }

    #[test]
    fn commons_answers_are_read_in_the_order_of_the_search() {
        let json: Value = serde_json::from_str(COMMONS_ANSWER).unwrap();
        let page = commons(&json);
        assert!(page.more);
        assert_eq!(page.images.len(), 2);
        let first = &page.images[0];
        assert_eq!(first.title, "Alaska Red Fox (Vulpes vulpes).jpg");
        assert_eq!(
            first.url,
            "https://upload.wikimedia.org/wikipedia/commons/8/8e/Alaska_Red_Fox_%28Vulpes_vulpes%29.jpg"
        );
        assert_eq!(
            first.preview,
            "https://thumb.wikimedia.org/wikipedia/commons/thumb/8/8e/Alaska_Red_Fox_%28Vulpes_vulpes%29.jpg/500px-Alaska_Red_Fox_%28Vulpes_vulpes%29.jpg"
        );
        assert_eq!((first.width, first.height), (3805, 2750));
        assert_eq!(
            first.page,
            "https://commons.wikimedia.org/wiki/File:Alaska_Red_Fox_(Vulpes_vulpes).jpg"
        );
        assert_eq!(first.author, "Gregory \"Slobirdr\" Smith");
        assert_eq!(first.license, "CC BY-SA 2.0");
        assert_eq!(
            first.license_url,
            "https://creativecommons.org/licenses/by-sa/2.0"
        );
        assert_eq!(first.source, "commons");
        let second = &page.images[1];
        assert_eq!(second.title, "Vulpes vulpes laying in snow.jpg");
        assert_eq!(second.author, "Shiretoko-Shari Tourist Association");
        assert_eq!(second.license, "Attribution");
        assert_eq!(second.license_url, "");

        // Nothing found: the answer has no `query` at all.
        let empty: Value = serde_json::from_str(r#"{"batchcomplete":true}"#).unwrap();
        assert_eq!(commons(&empty), SearchPage::default());
    }

    #[test]
    fn queries_are_put_into_addresses() {
        let openverse = address(WebSource::Openverse, "red fox & co", 2).unwrap();
        assert_eq!(
            openverse,
            "https://api.openverse.org/v1/images/?q=red+fox+%26+co&page=2&page_size=20"
        );
        let commons = address(WebSource::Commons, "лиса", 3).unwrap();
        assert!(commons.starts_with("https://commons.wikimedia.org/w/api.php?action=query&"));
        assert!(commons.contains("gsrsearch=%D0%BB%D0%B8%D1%81%D0%B0"));
        assert!(commons.contains("gsroffset=40"));
        assert!(commons.contains("gsrnamespace=6"));
        let first = address(WebSource::Commons, "fox", 0).unwrap();
        assert!(first.contains("gsroffset=0"));
    }

    #[test]
    fn marks_are_cut_from_addresses() {
        assert_eq!(
            without_marks("https://example.org/a.jpg?utm_source=x&utm_content=y"),
            "https://example.org/a.jpg"
        );
        assert_eq!(
            without_marks("https://example.org/a.jpg?width=300&utm_source=x"),
            "https://example.org/a.jpg?width=300"
        );
        assert_eq!(without_marks("not an address"), "not an address");
    }

    #[test]
    fn sources_have_names() {
        for source in WebSource::ALL {
            assert_eq!(WebSource::from_name(source.name()), Some(*source));
            assert_eq!(
                serde_json::to_value(source).unwrap(),
                Value::from(source.name())
            );
        }
        assert_eq!(WebSource::from_name(" Commons "), Some(WebSource::Commons));
        assert_eq!(WebSource::from_name("pinterest"), None);
        let net = Net::new();
        let empty = search(&net, WebSource::Openverse, "  ", 1).unwrap_err();
        assert_eq!(empty.code, "emptyQuery");
    }
}
