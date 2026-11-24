//! The `pixelit://` protocol: pictures for the interface without passing
//! megabytes through JSON.
//!
//! - `/thumb?path=<file>&size=<pixels>`: a cached thumbnail;
//! - `/preview?path=<file>&size=<pixels>&page=<number>`: a file reduced for the screen;
//! - `/editor/<id>/base` and `/editor/<id>/render`: pictures of an editor session.
//!
//! Only decoded and re-encoded pictures leave this module, never the bytes of
//! a file, so the protocol cannot be used to read arbitrary files.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use rayon::{ThreadPool, ThreadPoolBuilder};
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager, UriSchemeResponder, Url};

use pixelit_core::{CancelToken, Error, Result};
use pixelit_engine::thumbs;

use crate::app::{lock, Core, Encoded};

pub const SCHEME: &str = "pixelit";

/// Pictures are decoded on a few threads of their own: a folder with a
/// thousand files asks for a thousand thumbnails at once.
const WORKERS: usize = 4;
const DEFAULT_THUMB: u32 = 256;
const DEFAULT_PREVIEW: u32 = 2048;
const MAX_SIDE: u32 = 8192;

static POOL: LazyLock<Option<ThreadPool>> = LazyLock::new(|| {
    ThreadPoolBuilder::new()
        .num_threads(WORKERS)
        .thread_name(|index| format!("pixelit-image-{index}"))
        .build()
        .ok()
});

/// A request taken apart: the path segments and the query values.
struct Query {
    segments: Vec<String>,
    path: Option<PathBuf>,
    size: Option<u32>,
    page: u32,
}

fn parse(uri: &str) -> Option<Query> {
    let url = Url::parse(uri).ok()?;
    let segments = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect();
    let mut query = Query {
        segments,
        path: None,
        size: None,
        page: 0,
    };
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "path" => query.path = Some(PathBuf::from(value.as_ref())),
            "size" => query.size = value.parse().ok(),
            "page" => query.page = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    Some(query)
}

fn serve(core: &Core, query: &Query) -> Result<Encoded> {
    let not_found = || Error::new("notFound");
    let cancel = CancelToken::new();
    let segments: Vec<&str> = query.segments.iter().map(String::as_str).collect();
    match segments.as_slice() {
        ["thumb"] => {
            let path = query.path.as_ref().ok_or_else(not_found)?;
            let side = query.size.unwrap_or(DEFAULT_THUMB).clamp(16, MAX_SIDE);
            thumbs::thumbnail(core.paths(), &core.engine.runner(), path, side, &cancel)
        }
        ["preview"] => {
            let path = query.path.as_ref().ok_or_else(not_found)?;
            let side = query.size.unwrap_or(DEFAULT_PREVIEW).clamp(16, MAX_SIDE);
            thumbs::preview_bytes(&core.engine.runner(), path, side, query.page, &cancel)
        }
        ["editor", id, kind] => {
            let editor = core.editor(id.parse().map_err(|_| not_found())?)?;
            match *kind {
                "base" => Ok(editor.base.clone()),
                "render" => Ok(lock(&editor.rendered).clone()),
                _ => Err(not_found()),
            }
        }
        _ => Err(not_found()),
    }
}

fn respond(responder: UriSchemeResponder, result: Result<Encoded>) {
    let response = match result {
        Ok((bytes, mime)) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            // The interface puts the change time of the file, or the version
            // of an editor render, into the address: equal addresses are
            // equal pictures.
            .header(header::CACHE_CONTROL, "private, max-age=3600")
            .body(bytes),
        Err(error) => {
            let status = if error.code == "notFound" || error.code == "sessionMissing" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            };
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(error.code.into_bytes())
        }
    };
    if let Ok(response) = response {
        responder.respond(response);
    }
}

/// Answers a request of the web view. Called on the main thread, so the work
/// goes to the pool.
pub fn handle(app: &AppHandle, request: &Request<Vec<u8>>, responder: UriSchemeResponder) {
    let Some(core) = app.try_state::<Arc<Core>>().map(|core| Arc::clone(&core)) else {
        respond(responder, Err(Error::new("notFound")));
        return;
    };
    let Some(query) = parse(&request.uri().to_string()) else {
        respond(responder, Err(Error::new("notFound")));
        return;
    };
    let work = move || respond(responder, serve(&core, &query));
    match POOL.as_ref() {
        Some(pool) => pool.spawn(work),
        None => work(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_taken_apart() {
        let query = parse("http://pixelit.localhost/thumb?path=C%3A%5CPhotos%5Ca%20b.jpg&size=320")
            .unwrap();
        assert_eq!(query.segments, ["thumb"]);
        assert_eq!(query.path, Some(PathBuf::from(r"C:\Photos\a b.jpg")));
        assert_eq!(query.size, Some(320));
        let editor = parse("pixelit://localhost/editor/7/render?v=3").unwrap();
        assert_eq!(editor.segments, ["editor", "7", "render"]);
        assert_eq!(editor.page, 0);
        assert!(parse("not a url").is_none());
    }
}
