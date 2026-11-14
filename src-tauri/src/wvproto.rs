//! Serves extension webview panels over a real origin.
//!
//! A VS Code webview is an ordinary HTML document with a stable origin. The
//! first version of this used `<iframe srcdoc>`, which is *almost* the same
//! thing and fails in ways that are hard to see:
//!
//!   * `document.baseURI` is the host page, so every relative URL resolves
//!     against the IDE instead of the extension.
//!   * `location.origin` is inherited rather than owned, and a sandboxed
//!     `about:srcdoc` document cannot own storage.
//!   * resources referenced with `file://` (what `asWebviewUri` returns here)
//!     cannot be fetched from a non-file origin.
//!
//! So each panel is published to `irixen-wv://<id>` instead, and anything the
//! page needs from disk is proxied through the same scheme at `/__asset?p=…`.
//! Same origin, real paths, no CORS, no sandbox surprises — which is what makes
//! panels like Live Preview's actually work rather than merely render.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use tauri::http::{Request, Response};

/// `panel id -> html`. Small by nature: a panel is a few KB of markup, and the
/// document itself lives on the extension's own HTTP server.
static PANELS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn panels() -> &'static Mutex<HashMap<String, String>> {
    PANELS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn put(id: &str, html: &str) {
    let mut map = panels().lock().unwrap_or_else(|e| e.into_inner());
    map.insert(id.to_string(), html.to_string());
    // Deliberately no hash-order eviction. `HashMap::keys().next()` may be the
    // panel on screen: removing it turns the iframe's next navigation into a
    // 404 (a blank panel that only an extension restart used to fix) and also
    // breaks the manual reload button. `remove()` is called on every close, so
    // these few-KB documents do not accumulate in practice.
}

pub fn remove(id: &str) {
    panels()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
}

fn not_found() -> Response<Vec<u8>> {
    Response::builder()
        .status(404)
        .header("Content-Type", "text/plain; charset=utf-8")
        .body(b"not found".to_vec())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

fn mime_of(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" | "md" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Decode `%xx` sequences. `+` is left alone: it is a literal in a path and
/// `encodeURIComponent` never produces it as a space.
fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            };
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Percent-encode one path segment, leaving unreserved characters alone.
fn pct_encode_segment(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Turn an absolute disk path into a same-origin asset URL.
///
/// Root-relative (`/__asset/…`) rather than relative: a stylesheet is itself
/// served from `/__asset/<dir>/<file>`, so `url("__asset/…")` would resolve to
/// `/__asset/<dir>/__asset/…` and 404. This is also what JS needs.
fn asset_path(raw: &str) -> String {
    let p = raw
        .strip_prefix("file:///")
        .or_else(|| raw.strip_prefix("file://"))
        .unwrap_or(raw);
    let mut decoded = pct_decode(p);
    if decoded.starts_with('/') && decoded.as_bytes().get(2) == Some(&b':') {
        decoded.remove(0); // /C:/dir → C:/dir
    }
    let clean = decoded.split('?').next().unwrap_or(&decoded);
    let encoded: Vec<String> = clean.split('/').map(pct_encode_segment).collect();
    format!("/__asset/{}", encoded.join("/"))
}

/// Replace bare absolute resource URLs anywhere in a script (or stylesheet).
///
/// `asWebviewUri` results are frequently embedded in JS template strings and
/// used to build `<script>`/`<link>` elements at runtime. Those never pass
/// through the panel HTML rewrite, so without this they stay `file://` and the
/// panel loads an unstyled, inert shell.
fn rewrite_script(text: &str) -> String {
    const CDN_A: &str = "https://file+.vscode-resource.vscode-cdn.net/";
    const CDN_B: &str = "http://file+.vscode-resource.vscode-cdn.net/";
    const FILE: &str = "file://";
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < text.len() {
        let a = lower[i..].find(FILE).map(|p| i + p);
        let b = lower[i..].find(CDN_A).map(|p| i + p);
        let c = lower[i..].find(CDN_B).map(|p| i + p);
        let Some(start) = [a, b, c].into_iter().flatten().min() else {
            out.push_str(&text[i..]);
            break;
        };
        out.push_str(&text[i..start]);
        let path_at = if Some(start) == a {
            let mut at = start + FILE.len();
            if text.as_bytes().get(at) == Some(&b'/') {
                at += 1; // file:///C:/…
            }
            at
        } else if Some(start) == b {
            start + CDN_A.len()
        } else {
            start + CDN_B.len()
        };
        let end = text[path_at..]
            .find(|ch: char| ch.is_whitespace() || "\"'`,;(){}<>\\".contains(ch))
            .map(|p| path_at + p)
            .unwrap_or(text.len());
        if end == path_at {
            out.push_str(&text[start..start + 1]);
            i = start + 1;
            continue;
        }
        out.push_str(&asset_path(&text[path_at..end]));
        i = end;
    }
    out
}

/// Rewrite absolute `file://` references inside a stylesheet.
///
/// Extensions generate CSS that points at disk (`url("file:///c:/…/icon.svg")`,
/// typically for `background-image`). A stylesheet served from this scheme
/// cannot read `file://`, so those references are redirected to the asset
/// route. Only absolute `file://` is touched — relative `url(...)` already
/// resolves against the stylesheet's own `/__asset/…` URL, which is the other
/// half of why the icon font loads.
fn rewrite_css(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len() + 64);
    let mut i = 0usize;
    while let Some(rel) = lower[i..].find("url(") {
        let at = i + rel;
        out.push_str(&text[i..at]);
        let after = at + 4;
        let Some(close) = text[after..].find(')') else {
            out.push_str(&text[at..]);
            return out;
        };
        let inner = text[after..after + close]
            .trim()
            .trim_matches(|c: char| c == '"' || c == '\'');
        // `url(...)` up to and including its own closing paren; the extra `)`
        // that used to be appended here turned `src: url(…)` into
        // `src: url(…))`, which invalidates the whole declaration and leaves an
        // `@font-face` with no source — the icon font then silently never loads.
        match inner
            .strip_prefix("file:///")
            .or_else(|| inner.strip_prefix("file://"))
        {
            Some(p) => out.push_str(&format!("url(\"{}\")", asset_path(p))),
            None => out.push_str(&format!("url({inner})")),
        }
        i = after + close + 1;
    }
    out.push_str(&text[i..]);
    out
}

/// Serve one file from disk with the right content type.
fn serve_file(file: &str) -> Response<Vec<u8>> {
    if file.is_empty() {
        return not_found();
    }
    let plain = crate::util::plain_path(file);
    let file_path = Path::new(&plain);
    let bytes = match std::fs::read(file_path) {
        Ok(b) => b,
        Err(_) => return not_found(),
    };
    let mime = mime_of(file_path);
    let body = if mime.starts_with("text/css") {
        rewrite_script(&rewrite_css(&String::from_utf8_lossy(&bytes))).into_bytes()
    } else if mime.starts_with("text/javascript") {
        rewrite_script(&String::from_utf8_lossy(&bytes)).into_bytes()
    } else {
        bytes
    };
    Response::builder()
        .status(200)
        .header("Content-Type", mime)
        // The scheme is per-app; nothing else can reach it either way.
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "no-cache")
        .body(body)
        .unwrap_or_else(|_| not_found())
}

/// Serve a panel document or an asset it references.
pub fn handle(request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let uri = request.uri();
    let path = uri.path().trim_start_matches('/');
    let query = uri.query().unwrap_or("");

    // `/__asset/<percent-encoded absolute path>`
    //
    // The path form (not `?p=`) matters: a stylesheet is fetched as
    // `/__asset/C%3A%2F…%2Fcodicon.css`, so its own `url(./codicon.ttf)` resolves
    // to `/__asset/C%3A%2F…%2Fcodicon.ttf` and loads. With a query string the
    // relative reference would drop the query and 404, which is exactly how the
    // icon font vanished.
    if let Some(rest) = path.strip_prefix("__asset/") {
        let file = pct_decode(rest.split('?').next().unwrap_or(""));
        return serve_file(&file);
    }
    if path == "__asset" {
        let raw = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("p="))
            .unwrap_or("");
        return serve_file(&pct_decode(raw));
    }

    let id = pct_decode(path);
    let html = panels()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned();
    match html {
        Some(body) => Response::builder()
            .status(200)
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Cache-Control", "no-cache")
            .body(body.into_bytes())
            .unwrap_or_else(|_| not_found()),
        None => not_found(),
    }
}

/// The base URL the renderer should use for this platform.
///
/// Windows maps custom schemes to `http://<scheme>.localhost`, everywhere else
/// the scheme is used directly. The renderer asks once and caches it.
pub fn base_for(user_agent: &str) -> String {
    if user_agent.contains("Windows") {
        "http://irixen-wv.localhost".to_string()
    } else {
        "irixen-wv://localhost".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(uri: &str) -> Request<Vec<u8>> {
        Request::builder().uri(uri).body(Vec::new()).unwrap()
    }

    fn encode(path: &str) -> String {
        let mut out = String::new();
        for b in path.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                    out.push(b as char)
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    #[test]
    fn serves_a_published_panel() {
        put("wv-test-1", "<html><body>ok</body></html>");
        let res = handle(&req("/wv-test-1"));
        assert_eq!(res.status(), 200);
        assert_eq!(
            String::from_utf8_lossy(res.body()),
            "<html><body>ok</body></html>"
        );
        assert_eq!(
            res.headers().get("Content-Type").unwrap(),
            "text/html; charset=utf-8"
        );

        remove("wv-test-1");
        assert_eq!(handle(&req("/wv-test-1")).status(), 404);
    }

    #[test]
    fn proxies_extension_assets_with_the_right_mime() {
        let dir = std::env::temp_dir().join("irixen-wv-asset-test");
        let _ = std::fs::create_dir_all(&dir);
        let css = dir.join("main.css");
        std::fs::write(&css, "body{color:red}").unwrap();
        let js = dir.join("main.js");
        std::fs::write(&js, "console.log(1)").unwrap();

        let encoded = encode(&css.to_string_lossy().replace('\\', "/"));
        let res = handle(&req(&format!("/__asset?p={encoded}")));
        assert_eq!(res.status(), 200);
        assert_eq!(
            res.headers().get("Content-Type").unwrap(),
            "text/css; charset=utf-8"
        );
        assert_eq!(String::from_utf8_lossy(res.body()), "body{color:red}");

        let encoded = encode(&js.to_string_lossy().replace('\\', "/"));
        let res = handle(&req(&format!("/__asset?p={encoded}")));
        assert_eq!(
            res.headers().get("Content-Type").unwrap(),
            "text/javascript; charset=utf-8"
        );

        // A path that does not exist must 404 rather than hang the panel.
        let missing = encode("C:/definitely/not/here.css");
        assert_eq!(handle(&req(&format!("/__asset?p={missing}"))).status(), 404);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn percent_decoding_keeps_plus_literal() {
        assert_eq!(pct_decode("a%20b"), "a b");
        assert_eq!(pct_decode("a+b"), "a+b");
        assert_eq!(pct_decode("C%3A%5Cdir"), "C:\\dir");
        // A truncated escape is left as-is instead of panicking.
        assert_eq!(pct_decode("bad%"), "bad%");
    }

    #[test]
    fn base_url_matches_the_platform() {
        assert_eq!(base_for("Mozilla/5.0 (Windows NT 10.0)"), "http://irixen-wv.localhost");
        assert_eq!(base_for("Mozilla/5.0 (Macintosh)"), "irixen-wv://localhost");
    }

    #[test]
    fn rewrites_absolute_urls_in_css_and_js() {
        let css = r#"@font-face{src:url("file:///c:/ext/media/icon.ttf") format("truetype");}"#;
        let out = rewrite_css(css);
        assert!(out.contains(r#"url("/__asset/c%3A/ext/media/icon.ttf")"#), "{out}");
        // A relative rewrite used to produce /__asset/<dir>/__asset/… and 404.
        assert!(!out.contains("/__asset/__asset/"), "{out}");

        let js = r#"const a="file:///c:/ext/media/main.js"; const b='https://file+.vscode-resource.vscode-cdn.net/c:/ext/media/x.css';"#;
        let out = rewrite_script(js);
        assert!(out.contains(r#""/__asset/c%3A/ext/media/main.js""#), "{out}");
        assert!(out.contains(r#"'/__asset/c%3A/ext/media/x.css'"#), "{out}");
        assert!(!out.contains("file://"), "{out}");
        // The live-preview server URL must be left alone.
        assert_eq!(rewrite_script("fetch('http://127.0.0.1:3000/')"), "fetch('http://127.0.0.1:3000/')");
    }
}
