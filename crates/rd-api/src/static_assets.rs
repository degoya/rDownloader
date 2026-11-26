use std::borrow::Cow;

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::{EmbeddedFile, RustEmbed};

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebAssets;

/// Paths that must resolve to a real file or fail.
///
/// The SPA fallback answers any unknown path with `index.html` and HTTP 200, which is right
/// for a client-side route and wrong for these: a browser asking for the manifest or the
/// service worker would receive an HTML page with a 200 and either silently refuse to
/// install the app or, worse, register the page as a service worker script.
const MUST_EXIST: &[&str] = &["manifest.webmanifest", "sw.js"];

/// Prefixes the service's own endpoints live under; an address below one that no route answered is
/// a client looking for an endpoint, never a page (RD-1480-08).
///
/// The shell with a `200` read as a success there: a misspelt or withdrawn `GET` route, or a
/// client newer than the service, parsed HTML as JSON. Matched on the path the mount point was
/// stripped from, so it holds behind a reverse proxy's prefix as well.
const ENDPOINT_PREFIXES: &[&str] = &["api/", "mcp/", "sabnzbd/"];

/// Prefixes with the same rule as [`MUST_EXIST`], for the same reason.
///
/// `.well-known/` is asked for by programs, not browsers: an MCP client that gets a `401` from
/// `/mcp` looks for OAuth metadata there and skips a location only on a `404`. The shell with a
/// `200` was parsed as JSON and ended the connection with "Unexpected token '<'".
const MUST_EXIST_PREFIXES: &[&str] = &["icons/", "assets/", ".well-known/"];

/// The documents that carry absolute references and are rewritten under a base path.
const REWRITTEN: &[&str] = &["index.html", "manifest.webmanifest"];

/// Serves the built web application, rewritten for the configured mount point.
///
/// ## Why the rewriting happens here
///
/// Vite emits absolute references — `/assets/index-….js`, `/manifest.webmanifest` — because
/// the mount point is not known when the frontend is built and the bundle is embedded in the
/// binary. Under a base path every one of those is a 404, and the failure is the worst kind:
/// the HTML arrives, so the server looks fine, and the page is simply blank.
///
/// Relative paths would not fix it either. `./assets/…` resolves against the *current* URL, so
/// it works at `/downloads` and breaks at `/downloads/queue` — a deep link or a reload rather
/// than a first visit, which is exactly the case nobody tests by hand.
///
/// So the three documents that carry absolute references are prefixed as they are served. The
/// base is also handed to the application as `window.__RD_BASE__`, because the router and the
/// API client need to know it and cannot read it from anywhere else.
///
/// The file is looked up by the request's own URI, the one the mount point was stripped from —
/// never by `OriginalUri`. The outer router that puts the stripping in front of the routing
/// records `OriginalUri` before it runs, so under a base it still reads `/downloads/assets/…`:
/// every file missed and was answered with the shell as `text/html`, and the browser refused
/// the module script — a blank page (RD-1240-33).
///
/// Only `GET` and `HEAD` are answered with the interface: a page is never posted to, and an
/// unknown path asked for with another method is a client looking for an endpoint — the OAuth
/// client registration at `POST /register` an MCP client tries after the metadata — that must
/// see a `404` rather than an HTML page with a `200`. The same holds for any method below
/// [`ENDPOINT_PREFIXES`].
pub(crate) async fn serve(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let base = state.access.proxy.read().await.base_path().to_owned();
    serve_with_base(&method, &uri, &headers, &base)
}

fn serve_with_base(method: &Method, uri: &Uri, headers: &HeaderMap, base: &str) -> Response {
    let requested = uri.path().trim_start_matches('/');
    let endpoint = ENDPOINT_PREFIXES
        .iter()
        .any(|prefix| requested.starts_with(prefix));
    if endpoint || (method != Method::GET && method != Method::HEAD) {
        return crate::ApiError::not_found(
            "web.route_not_found",
            "Nothing at this address answers this request",
        )
        .into_response();
    }
    let requested_asset = WebAssets::get(requested);
    if requested_asset.is_none() && must_exist(requested) {
        return crate::ApiError::not_found(
            "web.asset_not_found",
            "This file is not part of the web interface",
        )
        .into_response();
    }
    let is_spa_fallback = requested_asset.is_none();
    let asset = requested_asset.or_else(|| WebAssets::get("index.html"));
    let Some(asset) = asset else {
        return crate::ApiError::not_found(
            "web.interface_missing",
            "The web interface is not built into this binary",
        )
        .into_response();
    };
    let mime = mime_guess::from_path(if requested.is_empty() || is_spa_fallback {
        "index.html"
    } else {
        requested
    })
    .first_or_octet_stream();
    let served_as = if requested.is_empty() || is_spa_fallback {
        "index.html"
    } else {
        requested
    };
    let rewritten = !base.is_empty() && REWRITTEN.contains(&served_as);
    // The build's own compressed copy (`web/src/precompress.ts`); a document that is rewritten for
    // the mount point has none that is right.
    let packed = (!rewritten)
        .then(|| WebAssets::get(&format!("{served_as}.gz")))
        .flatten();
    let varies = packed.is_some();
    let packed = packed.filter(|_| crate::conditional::accepts_gzip(headers));
    let etag = entity_tag(&asset, rewritten.then_some(base), packed.is_some());
    let encoded = packed.is_some();
    let current = crate::conditional::has_current(headers, &etag);
    let mut response = if current {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let data = packed.map_or(asset.data, |packed| packed.data);
        let body = if rewritten {
            Bytes::from(rewrite_for_base(served_as, data.into_owned(), base))
        } else {
            // Served out of the binary itself, without a copy per request.
            match data {
                Cow::Borrowed(data) => Bytes::from_static(data),
                Cow::Owned(data) => Bytes::from(data),
            }
        };
        ([(header::CONTENT_TYPE, mime.as_ref())], Body::from(body)).into_response()
    };
    let answered = response.headers_mut();
    if encoded && !current {
        answered.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    if varies {
        answered.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }
    if let Ok(value) = HeaderValue::from_str(&etag) {
        answered.insert(header::ETAG, value);
    }
    answered.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(cache_policy(served_as)),
    );
    if served_as == "index.html" {
        let script = (!base.is_empty()).then(|| base_script(base));
        let policy = crate::security_headers::interface_policy(script.as_deref());
        if let Ok(value) = HeaderValue::from_str(&policy) {
            answered.insert(header::CONTENT_SECURITY_POLICY, value);
        }
    }
    response
}

/// The tag of one representation: the hash the build took of the file, the mount point its
/// references were rewritten for, and whether the body is the compressed copy.
fn entity_tag(asset: &EmbeddedFile, base: Option<&str>, packed: bool) -> String {
    let hash = hex::encode(&asset.metadata.sha256_hash()[..16]);
    let base = base.map_or_else(String::new, |base| format!("-{}", hex::encode(base)));
    let packed = if packed { "-gz" } else { "" };
    format!("\"{hash}{base}{packed}\"")
}

/// How long a browser or proxy may keep what was served as `served_as` (RD-1120-16).
///
/// The shell — `index.html` at the root, by name and as the SPA fallback — is revalidated on
/// every load: it names the bundle's files, and a cached one kept an old interface running after
/// an update until a hard reload. A cached service worker does the same; browsers already bypass
/// the HTTP cache for the worker script, but proxies do not. The icons and the manifest keep their
/// names across versions, so they are revalidated too; with a tag that is an empty `304`
/// (RD-1480-19). The files under `assets/` carry their content hash in the name, so a cached copy
/// is right forever.
fn cache_policy(served_as: &str) -> &'static str {
    if served_as.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

/// Whether a path must resolve to a real asset rather than the SPA shell.
fn must_exist(path: &str) -> bool {
    MUST_EXIST.contains(&path)
        || MUST_EXIST_PREFIXES
            .iter()
            .any(|prefix| path.starts_with(prefix))
}

/// Prefixes the absolute references in the documents that carry them.
///
/// Only these three: everything else the bundle serves is either binary or already relative.
/// A blanket search-and-replace over every asset would rewrite strings inside the JavaScript
/// that happen to look like paths, which is a far larger blast radius than the problem.
fn rewrite_for_base(name: &str, data: Vec<u8>, base: &str) -> Vec<u8> {
    if base.is_empty() {
        return data;
    }
    match name {
        "index.html" => {
            let Ok(html) = String::from_utf8(data.clone()) else {
                return data;
            };
            let html = html
                .replace("href=\"/", &format!("href=\"{base}/"))
                .replace("src=\"/", &format!("src=\"{base}/"));
            // Injected before the module script so the application can read it during start-up
            // rather than after its router has already been built at the wrong base.
            let marker = "<script type=\"module\"";
            let injected = format!("<script>{}</script>{marker}", base_script(base));
            html.replacen(marker, &injected, 1).into_bytes()
        }
        "manifest.webmanifest" => {
            let Ok(text) = String::from_utf8(data.clone()) else {
                return data;
            };
            text.replace("\"/", &format!("\"{base}/")).into_bytes()
        }
        _ => data,
    }
}

/// The inline script that hands the mount point to the application. One function, because the
/// shell's content security policy allows it by the hash of exactly these bytes.
fn base_script(base: &str) -> String {
    format!(
        "window.__RD_BASE__={};",
        serde_json::to_string(base).unwrap_or_else(|_| "\"\"".to_owned())
    )
}

#[cfg(test)]
mod base_path_tests {
    use super::rewrite_for_base;

    const INDEX: &str = r#"<link rel="icon" href="/favicon.svg" /><script type="module" src="/assets/app.js"></script>"#;

    /// At the root nothing is touched, so the common deployment pays nothing.
    #[test]
    fn an_empty_base_leaves_the_document_alone() {
        let out = rewrite_for_base("index.html", INDEX.as_bytes().to_vec(), "");
        assert_eq!(out, INDEX.as_bytes());
    }

    #[test]
    fn absolute_references_are_prefixed_with_the_base() {
        let out = rewrite_for_base("index.html", INDEX.as_bytes().to_vec(), "/downloads");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(out.contains(r#"href="/downloads/favicon.svg""#), "{out}");
        assert!(out.contains(r#"src="/downloads/assets/app.js""#), "{out}");
    }

    /// The application has to learn the base before it builds its router.
    #[test]
    fn the_base_is_handed_to_the_application_before_the_module_script() {
        let out = rewrite_for_base("index.html", INDEX.as_bytes().to_vec(), "/downloads");
        let out = String::from_utf8(out).expect("utf-8");
        let injected = out.find("__RD_BASE__").expect("the base was not injected");
        let module = out.find("<script type=\"module\"").expect("module script");
        assert!(
            injected < module,
            "the base is injected after the app starts"
        );
        assert!(out.contains(r#"window.__RD_BASE__="/downloads";"#), "{out}");
    }

    #[test]
    fn the_manifest_paths_are_prefixed_too() {
        let manifest = r#"{"start_url":"/downloads","icons":[{"src":"/icons/icon-192.png"}]}"#;
        let out = rewrite_for_base("manifest.webmanifest", manifest.as_bytes().to_vec(), "/app");
        let out = String::from_utf8(out).expect("utf-8");
        assert!(out.contains(r#""start_url":"/app/downloads""#), "{out}");
        assert!(out.contains(r#""src":"/app/icons/icon-192.png""#), "{out}");
    }

    /// A blanket rewrite over the bundle would corrupt strings inside the JavaScript that
    /// merely look like paths.
    #[test]
    fn other_assets_are_left_exactly_as_they_are() {
        let script = br#"const separator = "/";"#.to_vec();
        assert_eq!(
            rewrite_for_base("assets/app.js", script.clone(), "/downloads"),
            script
        );
    }
}

#[cfg(test)]
#[path = "static_assets_tests.rs"]
mod tests;
