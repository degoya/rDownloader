use axum::{
    body::Body,
    extract::OriginalUri,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;

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

/// Prefixes with the same rule, for the same reason.
const MUST_EXIST_PREFIXES: &[&str] = &["icons/", "assets/"];

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
pub(crate) async fn serve(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let base = state.proxy.read().await.base_path().to_owned();
    serve_with_base(&uri, &base)
}

fn serve_with_base(uri: &axum::http::Uri, base: &str) -> Response {
    let requested = uri.path().trim_start_matches('/');
    let requested_asset = WebAssets::get(requested);
    if requested_asset.is_none() && must_exist(requested) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let is_spa_fallback = requested_asset.is_none();
    let asset = requested_asset.or_else(|| WebAssets::get("index.html"));
    let Some(asset) = asset else {
        return StatusCode::NOT_FOUND.into_response();
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
    let body = rewrite_for_base(served_as, asset.data.into_owned(), base);
    let mut response = ([(header::CONTENT_TYPE, mime.as_ref())], Body::from(body)).into_response();
    if requested == "sw.js" {
        // A cached service worker keeps serving an old shell after an update. Browsers
        // already bypass the HTTP cache for the worker script, but proxies do not.
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-cache"),
        );
    }
    response
}

/// Whether a path must resolve to a real asset rather than the SPA shell.
fn must_exist(path: &str) -> bool {
    MUST_EXIST.contains(&path)
        || MUST_EXIST_PREFIXES
            .iter()
            .any(|prefix| path.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use axum::http::header;

    use super::serve_with_base;

    #[tokio::test]
    async fn a_missing_manifest_or_worker_is_not_answered_with_the_app_shell() {
        // These are fetched by the browser, not by a router. Answering an HTML page with a
        // 200 would make an install failure look like a success and hide the real problem.
        for path in [
            "/manifest.webmanifest",
            "/sw.js",
            "/icons/icon-192.png",
            "/assets/does-not-exist.js",
        ] {
            let response = serve_with_base(&path.parse().expect("URI"), "");
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            assert!(
                !content_type.starts_with("text/html"),
                "{path} was answered with the app shell"
            );
        }
    }

    #[tokio::test]
    async fn spa_routes_are_served_as_html() {
        let response = serve_with_base(&"/routing".parse().expect("URI"), "");
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&"text/html".parse().expect("content type"))
        );
    }
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
            let injected = format!(
                "<script>window.__RD_BASE__={};</script>{marker}",
                serde_json::to_string(base).unwrap_or_else(|_| "\"\"".to_owned())
            );
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
