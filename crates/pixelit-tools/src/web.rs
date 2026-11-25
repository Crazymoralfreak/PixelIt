//! Tools that find pictures on the web and download them.
//!
//! Both need the permission to download. An agent reaches only the open
//! web: addresses of this computer and of its local network are refused,
//! whatever a page or a model asks for.

use serde_json::{json, Value};

use pixelit_ai::web::{self, Step};
use pixelit_ai::{WebImage, WebSource};
use pixelit_core::{Error, Result};

use crate::args::{self, choice, integer, object, string};
use crate::{Tool, ToolContext};

/// Pictures of one page listed for a model; the count stays exact.
const LISTED: usize = 40;

/// Agents never reach the local network.
const LOCAL: bool = false;

const SOURCES: [&str; 2] = ["openverse", "commons"];

fn check_downloads(context: &ToolContext) -> Result<()> {
    if context.access().allow_downloads {
        Ok(())
    } else {
        Err(Error::new("agentNotAllowed").with("action", "downloads"))
    }
}

/// A picture as a model sees it: what `download_web_image` takes back.
fn listed(image: &WebImage) -> Value {
    let mut item = json!({ "url": image.url });
    let mut put = |name: &str, value: &str| {
        if !value.is_empty() {
            item[name] = json!(value);
        }
    };
    put("title", &image.title);
    put("page", &image.page);
    put("author", &image.author);
    put("license", &image.license);
    put("licenseUrl", &image.license_url);
    if image.preview != image.url {
        put("preview", &image.preview);
    }
    if image.width > 0 && image.height > 0 {
        item["width"] = json!(image.width);
        item["height"] = json!(image.height);
    }
    if image.main {
        item["main"] = json!(true);
    }
    item
}

fn find_schema() -> Value {
    object(
        json!({
            "query": string("What to look for, a few words; searches a catalogue of openly licensed pictures"),
            "source": choice(
                "Catalogue for the query: openverse (default) or commons (Wikimedia Commons)",
                &SOURCES,
            ),
            "page": integer("Page of the search results, from 1"),
            "url": string("Instead of a query: the address of a web page, a Pinterest pin or a picture; lists the pictures found there"),
        }),
        &[],
    )
}

fn find_web_images(context: &ToolContext, arguments: &Value) -> Result<Value> {
    check_downloads(context)?;
    let net = context.ai.net();
    if let Some(url) = args::optional_text(arguments, "url") {
        let found = web::inspect(net, &url, LOCAL, &context.cancel)?;
        let images: Vec<Value> = found.images.iter().take(LISTED).map(listed).collect();
        return Ok(json!({
            "url": found.url,
            "title": found.title,
            "direct": found.direct,
            "total": found.images.len(),
            "images": images,
        }));
    }
    let query = args::text(arguments, "query")?;
    let source = match args::optional_text(arguments, "source") {
        Some(name) => WebSource::from_name(&name)
            .ok_or_else(|| Error::new("badArguments").with("name", "source"))?,
        None => WebSource::Openverse,
    };
    let page = args::whole(arguments, "page").unwrap_or(1).clamp(1, 500) as u32;
    let found = web::search(net, source, &query, page)?;
    let images: Vec<Value> = found.images.iter().map(listed).collect();
    Ok(json!({
        "source": source.name(),
        "page": page,
        "more": found.more,
        "images": images,
    }))
}

fn download_schema() -> Value {
    object(
        json!({
            "url": string("Address of the picture, as find_web_images gave it"),
            "folder": string("Absolute path of the folder to save into"),
            "title": string("Title of the picture; becomes the file name"),
            "page": string("The page the picture was found on"),
            "author": string("Author, kept in the journal"),
            "license": string("Licence, kept in the journal"),
            "licenseUrl": string("Address of the licence text"),
            "preview": string("The preview address of the picture, when find_web_images gave one"),
        }),
        &["url", "folder"],
    )
}

fn download_web_image(context: &ToolContext, arguments: &Value) -> Result<Value> {
    check_downloads(context)?;
    let folder = args::path(arguments, "folder")?;
    context.check_path(&folder)?;
    let url = args::text(arguments, "url")?;
    let text = |name: &str| args::optional_text(arguments, name).unwrap_or_default();
    let image = WebImage {
        preview: args::optional_text(arguments, "preview").unwrap_or_else(|| url.clone()),
        title: text("title"),
        page: text("page"),
        author: text("author"),
        license: text("license"),
        license_url: text("licenseUrl"),
        url,
        ..WebImage::default()
    };
    let title = if image.title.is_empty() {
        image.url.as_str()
    } else {
        image.title.as_str()
    };
    let downloaded = web::download_all(
        context.ai.net(),
        &context.engine.paths,
        title,
        std::slice::from_ref(&image),
        &folder,
        LOCAL,
        &mut |_: &Step<'_>| {},
        &context.cancel,
    )?;
    let path = downloaded
        .files
        .first()
        .ok_or_else(|| Error::new("notPicture").with("url", &image.url))?;
    let bytes = std::fs::metadata(path).map_or(0, |metadata| metadata.len());
    Ok(json!({
        "path": path,
        "bytes": bytes,
        "author": image.author,
        "license": image.license,
    }))
}

pub static TOOLS: &[Tool] = &[
    Tool {
        name: "find_web_images",
        description: "Find pictures on the web: search Openverse or Wikimedia Commons by a query (every result has an author and a licence), or list the pictures of a web page or a Pinterest pin by its address. Needs the permission to download.",
        schema: find_schema,
        run: find_web_images,
    },
    Tool {
        name: "download_web_image",
        description: "Download a picture found by find_web_images into a folder. The source, the author and the licence are written into the journal. Needs the permission to download.",
        schema: download_schema,
        run: download_web_image,
    },
];

#[cfg(test)]
mod tests {
    use pixelit_engine::journal::Journal;

    use crate::call;
    use crate::testing::context;

    use super::*;

    fn allow(context: &ToolContext) {
        context
            .engine
            .update_settings(|settings| settings.agents.allow_downloads = true)
            .unwrap();
    }

    #[test]
    fn the_web_needs_the_permission_to_download() {
        let (_folder, context, allowed) = context();
        let find = call(&context, "find_web_images", &json!({ "query": "fox" })).unwrap_err();
        assert_eq!(find.code, "agentNotAllowed");
        assert_eq!(find.params["action"], "downloads");
        let download = call(
            &context,
            "download_web_image",
            &json!({ "url": "https://example.org/a.jpg", "folder": allowed }),
        )
        .unwrap_err();
        assert_eq!(download.code, "agentNotAllowed");
    }

    #[test]
    fn arguments_are_checked_before_any_request() {
        let (folder, context, allowed) = context();
        allow(&context);
        let nothing = call(&context, "find_web_images", &json!({})).unwrap_err();
        assert_eq!(nothing.code, "badArguments");
        assert_eq!(nothing.params["name"], "query");
        let source = call(
            &context,
            "find_web_images",
            &json!({ "query": "fox", "source": "pinterest" }),
        )
        .unwrap_err();
        assert_eq!(source.params["name"], "source");
        let denied = call(
            &context,
            "download_web_image",
            &json!({ "url": "https://example.org/a.jpg", "folder": folder.path().join("other") }),
        )
        .unwrap_err();
        assert_eq!(denied.code, "agentPathDenied");
        let missing = call(
            &context,
            "download_web_image",
            &json!({ "folder": allowed }),
        )
        .unwrap_err();
        assert_eq!(missing.params["name"], "url");
    }

    #[test]
    fn the_local_network_is_closed_for_agents() {
        let (_folder, context, allowed) = context();
        allow(&context);
        for url in [
            "http://127.0.0.1:8188/view?filename=a.png",
            "http://192.168.1.1/",
            "http://router.local/logo.png",
        ] {
            let find = call(&context, "find_web_images", &json!({ "url": url })).unwrap_err();
            assert_eq!(find.code, "localAddress", "{url}");
            let download = call(
                &context,
                "download_web_image",
                &json!({ "url": url, "folder": allowed, "title": "Inside" }),
            )
            .unwrap_err();
            assert_eq!(download.code, "localAddress", "{url}");
        }
        assert!(std::fs::read_dir(&allowed).unwrap().next().is_none());
        // What an agent tried is in the journal.
        let journal = Journal::load(&context.engine.paths);
        assert_eq!(journal.entries.len(), 3);
        assert_eq!(journal.entries[0].kind, "download");
        assert_eq!(journal.entries[0].title, "Inside");
        assert_eq!(journal.entries[0].failed, 1);
    }

    #[test]
    fn pictures_are_listed_for_a_model() {
        let image = WebImage {
            url: String::from("https://example.org/fox.jpg"),
            preview: String::from("https://example.org/fox-small.jpg"),
            title: String::from("Fox"),
            width: 1024,
            height: 685,
            author: String::from("Someone"),
            license: String::from("CC BY 2.0"),
            main: true,
            ..WebImage::default()
        };
        assert_eq!(
            listed(&image),
            json!({
                "url": "https://example.org/fox.jpg",
                "preview": "https://example.org/fox-small.jpg",
                "title": "Fox",
                "author": "Someone",
                "license": "CC BY 2.0",
                "width": 1024,
                "height": 685,
                "main": true,
            })
        );
        let bare = WebImage {
            url: String::from("https://example.org/a.png"),
            preview: String::from("https://example.org/a.png"),
            ..WebImage::default()
        };
        assert_eq!(listed(&bare), json!({ "url": "https://example.org/a.png" }));
    }
}
