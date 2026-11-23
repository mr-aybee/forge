//! Embeds the built web console (`src/ui/dist`) into the binary, so a Forge
//! release is one self-contained executable. When the UI has not been built
//! (plain `cargo build` during development), the table is empty and the
//! server serves `ui_dir` from disk if configured.
//!
//! Set `FORGE_UI_DIST` to embed another directory.
//!
//! Also writes the API route inventory (`build_routes.rs`).

#[path = "build_routes.rs"]
mod build_routes;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::path);
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            walk(&p, root, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            let key = rel.to_string_lossy().replace('\\', "/");
            out.push((key, p));
        }
    }
}

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=FORGE_UI_DIST");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let dist = std::env::var("FORGE_UI_DIST").map(PathBuf::from).unwrap_or_else(|_| manifest.join("../../ui/dist"));
    println!("cargo:rerun-if-changed={}", dist.display());
    let index = dist.join("index.html");
    println!("cargo:rerun-if-changed={}", index.display());

    let mut files = Vec::new();
    if index.is_file() {
        walk(&dist, &dist, &mut files);
    }
    let mut code = String::from(
        "/// Embedded web console: (path, content type, bytes).\npub static UI_FILES: &[(&str, &str, &[u8])] = &[\n",
    );
    for (key, path) in &files {
        let abs = path.canonicalize().unwrap_or_else(|_| path.clone());
        let _ = writeln!(code, "    ({key:?}, {:?}, include_bytes!({:?})),", mime(key), abs.to_string_lossy());
    }
    code.push_str("];\n");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    let _ = std::fs::write(out_dir.join("ui_files.rs"), code);

    // API route inventory for /api/openapi.json (issue #5, phase 0).
    let api = manifest.join("src/api");
    println!("cargo:rerun-if-changed={}", api.display());
    if let Ok(rd) = std::fs::read_dir(&api) {
        for e in rd.flatten() {
            println!("cargo:rerun-if-changed={}", e.path().display());
        }
    }
    println!("cargo:rerun-if-changed=build_routes.rs");
    let _ = std::fs::write(out_dir.join("routes.rs"), build_routes::render(&build_routes::scan(&api)));
}
