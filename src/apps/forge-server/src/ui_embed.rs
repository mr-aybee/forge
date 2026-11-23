//! The web console compiled into the binary (see `build.rs`).

use axum::body::Body;
use axum::http::{HeaderValue, Response, StatusCode, Uri, header};

include!(concat!(env!("OUT_DIR"), "/ui_files.rs"));

/// Whether this binary carries the console.
#[must_use]
pub fn available() -> bool {
    UI_FILES.iter().any(|(p, _, _)| *p == "index.html")
}

fn find(path: &str) -> Option<&'static (&'static str, &'static str, &'static [u8])> {
    UI_FILES.iter().find(|(p, _, _)| *p == path)
}

/// Serves an embedded file; unknown paths get `index.html` (client-side
/// routes). Hashed assets are cached for a year, everything else revalidated.
pub async fn serve(uri: Uri) -> Response<Body> {
    let path = uri.path().trim_start_matches('/');
    // Unknown API, agent and OAuth paths are 404s, never the console page.
    let reserved = ["api/", "agent/", "oauth/", "install.sh"].iter().any(|p| path.starts_with(p));
    let (file, immutable) = match find(path) {
        _ if reserved => (None, false),
        Some(f) if !path.is_empty() => (Some(f), path.starts_with("assets/")),
        _ if path.starts_with("assets/") => (None, false),
        _ => (find("index.html"), false),
    };
    let Some((_, mime, bytes)) = file else {
        let mut r = Response::new(Body::from("not found"));
        *r.status_mut() = StatusCode::NOT_FOUND;
        return r;
    };
    let mut r = Response::new(Body::from(*bytes));
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if immutable { "public, max-age=31536000, immutable" } else { "no-cache" }),
    );
    r
}
