//! `doculens://` custom URI scheme (served as `http://doculens.localhost/…`
//! on Windows): page images and thumbnails straight from the workspace cache,
//! and the HTML bundles the hidden print window loads. No file outside those
//! folders can be addressed — ids are validated and paths are built by the
//! core, never taken from the URL.
//!
//! * `/pages/<doc_id>/<page>.jpg`, `/thumbs/<doc_id>/<page>.jpg`
//! * `/print/<token>/<relative file>` (token registered by the printer)
//!
//! Serving a page / thumbnail touches the folder's `.last_used` marker (the retention LRU
//! evicts by last use); a page that is gone (evicted) queues a re-render of its document.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::http::{header, Response, StatusCode};
use tauri::{AppHandle, Manager, UriSchemeContext};

use doculens_core::documents::DocumentCache;
use doculens_core::retention;

use crate::docs_bridge;
use crate::state::AppState;

pub const SCHEME: &str = "doculens";

fn is_safe_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn respond(status: StatusCode, body: &'static str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(body.as_bytes().to_vec())
        .expect("static response")
}

fn mime_for(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// True at most once a minute per key (process-wide; stale entries are dropped as the table
/// grows). Throttles the `.last_used` touches and the re-render requests per document.
fn once_per_minute(key: &str) -> bool {
    static SEEN: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    const WINDOW: Duration = Duration::from_secs(60);
    let mut seen = SEEN.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if seen.get(key).is_some_and(|t| now.duration_since(*t) < WINDOW) {
        return false;
    }
    if seen.len() >= 4096 {
        seen.retain(|_, t| now.duration_since(*t) < WINDOW);
    }
    seen.insert(key.to_string(), now);
    true
}

/// Records that a page / thumbnail folder was viewed (`.last_used` marker, at most once a
/// minute per folder) so the retention LRU evicts by last use rather than by render time.
fn note_use(dir: &Path) {
    if once_per_minute(&dir.to_string_lossy()) {
        retention::touch_last_used(dir);
    }
}

fn serve_file(path: &Path, cache_control: &'static str) -> Response<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime_for(path))
            .header(header::CACHE_CONTROL, cache_control)
            .header(header::CONTENT_LENGTH, bytes.len())
            .body(bytes)
            .unwrap_or_else(|_| respond(StatusCode::INTERNAL_SERVER_ERROR, "response")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => respond(StatusCode::NOT_FOUND, "not found"),
        Err(_) => respond(StatusCode::INTERNAL_SERVER_ERROR, "read error"),
    }
}

/// Resolves a cache image URL path to a file inside the open workspace.
fn cache_file(app: &AppHandle, kind: &str, doc_id: &str, file: &str) -> Option<PathBuf> {
    if !is_safe_id(doc_id) {
        return None;
    }
    let page: u32 = file.strip_suffix(".jpg").and_then(|s| s.strip_prefix('p')).and_then(|s| s.parse().ok())?;
    let state = app.state::<AppState>();
    let paths = state.with_workspace(|ws| ws.paths.clone()).ok()?;
    let cache = DocumentCache::for_document(&paths, doc_id);
    Some(match kind {
        "pages" => cache.page_file(page),
        "thumbs" => cache.thumb_file(page),
        _ => return None,
    })
}

pub fn handle(ctx: UriSchemeContext<'_, tauri::Wry>, request: tauri::http::Request<Vec<u8>>) -> Response<Vec<u8>> {
    let app = ctx.app_handle();
    let path = request.uri().path().to_string();
    let decoded = percent_decode(&path);
    let parts: Vec<&str> = decoded.trim_start_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        [kind @ ("pages" | "thumbs"), doc_id, file] => match cache_file(app, kind, doc_id, file) {
            // Page images change when re-rendered; the viewer busts the cache with `?v=<rendered_at>`.
            Some(p) => {
                let response = serve_file(&p, "private, max-age=3600");
                if response.status() == StatusCode::OK {
                    if let Some(dir) = p.parent() {
                        note_use(dir);
                    }
                } else if response.status() == StatusCode::NOT_FOUND && once_per_minute(&format!("render:{doc_id}")) {
                    // Evicted by the retention LRU (or deleted by hand): bring the pages back for
                    // the next request; this one stays a 404.
                    docs_bridge::rerender_if_missing(app, doc_id);
                }
                response
            }
            None => respond(StatusCode::BAD_REQUEST, "bad request"),
        },
        ["print", token, rest @ ..] if !rest.is_empty() => {
            let state = app.state::<AppState>();
            let root = state.print_bundles.lock().ok().and_then(|m| m.get(*token).cloned());
            let Some(root) = root else { return respond(StatusCode::NOT_FOUND, "unknown print bundle") };
            if rest.iter().any(|seg| *seg == ".." || seg.contains('\\')) {
                return respond(StatusCode::BAD_REQUEST, "bad path");
            }
            let mut file = root.clone();
            for seg in rest {
                file.push(seg);
            }
            // Stay inside the bundle even through symlinks.
            match (std::fs::canonicalize(&root), std::fs::canonicalize(&file)) {
                (Ok(r), Ok(f)) if f.starts_with(&r) => serve_file(&f, "no-store"),
                _ => respond(StatusCode::NOT_FOUND, "not found"),
            }
        }
        _ => respond(StatusCode::NOT_FOUND, "not found"),
    }
}

/// Minimal percent-decoding (UTF-8); invalid sequences are kept verbatim.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            // Byte slice, not `&str[..]`: a multibyte char after `%` must not panic.
            if let Ok(v) =
                std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or(())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Origin of the custom scheme in the form the platform webview expects
/// (`http://doculens.localhost` on Windows, `doculens://localhost` elsewhere).
pub fn base_url() -> String {
    if cfg!(windows) {
        format!("http://{SCHEME}.localhost")
    } else {
        format!("{SCHEME}://localhost")
    }
}

/// URL (platform form) for a cached page image.
pub fn page_url(doc_id: &str, page_no: u32, thumb: bool, version: Option<&str>) -> String {
    let kind = if thumb { "thumbs" } else { "pages" };
    let base = format!("{}/{kind}/{doc_id}/p{page_no:04}.jpg", base_url());
    match version {
        Some(v) => format!("{base}?v={v}"),
        None => base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_decoding() {
        assert!(is_safe_id("0123abcdef"));
        assert!(!is_safe_id("../x"));
        assert!(!is_safe_id(""));
        assert_eq!(percent_decode("/print/t/%EA%B2%AC%EC%A0%81.png"), "/print/t/견적.png");
        assert_eq!(percent_decode("/a%2"), "/a%2");
        assert_eq!(percent_decode("/a%한글"), "/a%한글");
        assert!(page_url("abc", 3, true, Some("v1")).contains("/thumbs/abc/p0003.jpg?v=v1"));
    }

    #[test]
    fn throttle_is_once_per_minute_per_key() {
        assert!(once_per_minute("protocol-test-a"));
        assert!(!once_per_minute("protocol-test-a"));
        assert!(once_per_minute("protocol-test-b"));
    }

    #[test]
    fn note_use_touches_the_marker() {
        let dir = std::env::temp_dir().join(format!("doculens-protocol-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        note_use(&dir);
        assert!(dir.join(retention::LAST_USED_MARKER).is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
