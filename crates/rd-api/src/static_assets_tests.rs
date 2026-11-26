//! The embedded interface as it is served: which file answers, with which headers.

use axum::{
    body::to_bytes,
    http::{HeaderMap, HeaderName, Method, StatusCode, header},
    response::Response,
};

use super::{WebAssets, serve_with_base};

fn get(path: &str, base: &str, headers: &HeaderMap) -> Response {
    serve_with_base(&Method::GET, &path.parse().expect("URI"), headers, base)
}

fn with(name: HeaderName, value: &str) -> HeaderMap {
    HeaderMap::from_iter([(name, value.parse().expect("header value"))])
}

fn text(response: &Response, name: HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// A copy a client holds is confirmed with an empty `304` that carries what the full answer
/// would; another mount point or a changed file is another tag (RD-1480-19).
#[tokio::test]
async fn a_current_copy_is_confirmed_without_its_body() {
    for (path, base) in [
        ("/", ""),
        ("/queue", "/rd"),
        ("/favicon.svg", ""),
        ("/sw.js", ""),
    ] {
        let full = get(path, base, &HeaderMap::new());
        assert_eq!(full.status(), StatusCode::OK, "{path}");
        let etag = text(&full, header::ETAG).expect("tag");
        let again = get(path, base, &with(header::IF_NONE_MATCH, &etag));
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED, "{path}");
        for name in [
            header::ETAG,
            header::CACHE_CONTROL,
            header::VARY,
            header::CONTENT_SECURITY_POLICY,
        ] {
            assert_eq!(text(&again, name.clone()), text(&full, name), "{path}");
        }
        let body = to_bytes(again.into_body(), usize::MAX).await.expect("body");
        assert!(body.is_empty(), "{path}");
        let other = get(path, base, &with(header::IF_NONE_MATCH, "\"other\""));
        assert_eq!(other.status(), StatusCode::OK, "{path}");
    }
    let tag = |base: &str| text(&get("/", base, &HeaderMap::new()), header::ETAG);
    assert_ne!(tag(""), tag("/rd"), "the base is part of the document");
    assert_ne!(tag("/rd"), tag("/downloads"));
}

/// The build's compressed copy goes to a client that takes gzip, under a tag of its own, and
/// never in place of a document rewritten for the mount point (RD-1480-19).
#[tokio::test]
async fn the_compressed_copy_goes_to_who_takes_it() {
    // A stub `web/dist`, as some CI jobs compile against, holds no copies.
    let Some(copy) =
        WebAssets::iter().find(|name| name.starts_with("assets/") && name.ends_with(".js.gz"))
    else {
        return;
    };
    let path = format!("/{}", copy.trim_end_matches(".gz"));
    let gzip = with(header::ACCEPT_ENCODING, "gzip, deflate");
    let plain = get(&path, "", &HeaderMap::new());
    let packed = get(&path, "", &gzip);
    assert_eq!(text(&plain, header::CONTENT_ENCODING), None);
    assert_eq!(
        text(&packed, header::CONTENT_ENCODING).as_deref(),
        Some("gzip")
    );
    for response in [&plain, &packed] {
        assert_eq!(
            text(response, header::VARY).as_deref(),
            Some("Accept-Encoding")
        );
    }
    assert_eq!(
        text(&plain, header::CONTENT_TYPE),
        text(&packed, header::CONTENT_TYPE),
        "the type is the script's, not the copy's"
    );
    assert_ne!(text(&plain, header::ETAG), text(&packed, header::ETAG));
    let plain = to_bytes(plain.into_body(), usize::MAX).await.expect("body");
    let packed = to_bytes(packed.into_body(), usize::MAX)
        .await
        .expect("body");
    assert!(packed.len() < plain.len());
    assert!(packed.starts_with(&[0x1f, 0x8b]));

    let shell = get("/", "/rd", &gzip);
    assert_eq!(text(&shell, header::CONTENT_ENCODING), None);
    let shell = to_bytes(shell.into_body(), usize::MAX).await.expect("body");
    assert!(shell.starts_with(b"<"), "the rewritten document itself");
}

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
        let response = get(path, "", &HeaderMap::new());
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

/// The shell carries the interface's policy, with the mount-point script's hash when it has
/// one; an asset does not (audit 2026-10-05, S5).
#[tokio::test]
async fn the_shell_carries_the_content_security_policy() {
    let policy = |path: &str, base: &str| {
        get(path, base, &HeaderMap::new())
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let root = policy("/", "").expect("the shell has a policy");
    assert!(root.contains("script-src 'self';"), "{root}");
    let mounted = policy("/queue", "/downloads").expect("the shell has a policy");
    assert!(mounted.contains("script-src 'self' 'sha256-"), "{mounted}");
    assert_eq!(policy("/favicon.svg", ""), None);
}

/// The shell is revalidated on every load, the content-hashed assets are kept for a year,
/// so a page opened after an update gets the new interface without a hard reload
/// (RD-1120-16).
#[tokio::test]
async fn the_shell_is_revalidated_and_hashed_assets_are_immutable() {
    let cache_control = |path: &str| {
        get(path, "", &HeaderMap::new())
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    for shell in ["/", "/index.html", "/linkgrabber", "/settings/system"] {
        assert_eq!(cache_control(shell).as_deref(), Some("no-cache"), "{shell}");
    }
    assert_eq!(cache_control("/sw.js").as_deref(), Some("no-cache"));
    let asset = WebAssets::iter()
        .find(|name| name.starts_with("assets/"))
        .expect("the bundle has content-hashed assets");
    assert_eq!(
        cache_control(&format!("/{asset}")).as_deref(),
        Some("public, max-age=31536000, immutable")
    );
    // Their names outlive a version, so they are revalidated like the shell (RD-1480-19).
    for named in [
        "/favicon.svg",
        "/manifest.webmanifest",
        "/icons/icon-192.png",
    ] {
        assert_eq!(cache_control(named).as_deref(), Some("no-cache"), "{named}");
    }
}

/// What an MCP client asks for after a `401` from `/mcp`: the OAuth metadata, then the
/// client registration. Each must be a JSON `404`, never the shell with a `200`, which the
/// client parsed as JSON and failed on with "Unexpected token '<'".
#[tokio::test]
async fn oauth_discovery_and_non_read_requests_get_a_json_404() {
    for (method, path) in [
        (Method::GET, "/.well-known/oauth-protected-resource/mcp"),
        (Method::GET, "/.well-known/oauth-authorization-server"),
        (Method::GET, "/.well-known/openid-configuration"),
        (Method::POST, "/register"),
        (Method::POST, "/routing"),
        (Method::PUT, "/"),
    ] {
        let response = serve_with_base(&method, &path.parse().expect("URI"), &HeaderMap::new(), "");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&"application/json".parse().expect("content type")),
            "{method} {path}"
        );
    }
    let head = serve_with_base(
        &Method::HEAD,
        &"/routing".parse().expect("URI"),
        &HeaderMap::new(),
        "",
    );
    assert_eq!(head.status(), StatusCode::OK);
}

/// An unknown address below an endpoint prefix is a JSON `404` for a read as well, with and
/// without a mount point (RD-1480-08); a page whose name only begins alike stays a page.
#[tokio::test]
async fn an_unknown_endpoint_is_a_json_404_and_never_the_shell() {
    for base in ["", "/rd"] {
        for path in [
            "/api/v1/no-such-route",
            "/api/v1/downloads/",
            "/api/v3/app/version",
            "//api/v1/no-such-route",
            "/mcp/sse",
            "/sabnzbd/unknown",
        ] {
            for method in [Method::GET, Method::HEAD] {
                let response = serve_with_base(
                    &method,
                    &path.parse().expect("URI"),
                    &HeaderMap::new(),
                    base,
                );
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
                let body = to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body");
                let body: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
                assert_eq!(body["code"], "web.route_not_found", "{method} {path}");
            }
        }
        for page in ["/apis", "/sabnzbd"] {
            assert_eq!(
                get(page, base, &HeaderMap::new()).status(),
                StatusCode::OK,
                "{page}"
            );
        }
    }
}

#[tokio::test]
async fn spa_routes_are_served_as_html() {
    let response = get("/routing", "", &HeaderMap::new());
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE),
        Some(&"text/html".parse().expect("content type"))
    );
}
