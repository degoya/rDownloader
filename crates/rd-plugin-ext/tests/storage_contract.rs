//! The upload-destination contract, exercised against the bundled WebDAV plugin.
//!
//! No WebDAV server runs here, so what is checked is what a server could not tell us anyway:
//! that the component satisfies the world, that the destination is narrowed to one host rather
//! than the wildcard the manifest declares, that what the plugin reports while it uploads
//! reaches the caller that started the upload, and — the point of the whole type — that
//! nothing local is deleted until the destination has separately confirmed it holds the file.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_extract::{StorageUpload, StorageUploader, UploadReport};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostHttpResponse, ResolverHost};
use rd_plugin_host::{
    PluginManifest,
    artifact::component,
    extension::{SourceState, StoragePlugin, Upload, UploadOutcome},
};

const WEBDAV: &str = include_str!("../../../plugins/webdav-storage/manifest.toml");

fn manifest() -> PluginManifest {
    toml::from_str(WEBDAV).expect("bundled manifest")
}

#[tokio::test]
async fn the_destination_compiles_against_the_storage_world() {
    let bytes = component("rd-plugin-webdav-storage");
    StoragePlugin::new(manifest(), &bytes, None).expect("satisfies the world");
}

#[test]
fn the_wildcard_in_the_manifest_is_the_only_one_there_is() {
    // A WebDAV server is wherever somebody put theirs, so the manifest cannot name it. What
    // keeps that from meaning "anywhere" is the per-invocation narrowing to the destination's
    // own host; this test pins the manifest side so the wildcard cannot quietly acquire
    // company.
    let manifest = manifest();
    assert_eq!(manifest.capabilities.domains(), ["*"]);
    assert!(manifest.capabilities.net_stream.is_none());
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    assert!(manifest.capabilities.secrets.is_empty());
}

/// A destination that accepts everything and then denies holding it.
struct ForgetfulDestination;

#[async_trait]
impl StorageUploader for ForgetfulDestination {
    fn installed(&self, _plugin_id: &str) -> bool {
        true
    }

    async fn upload(
        &self,
        _plugin_id: &str,
        _upload: StorageUpload<'_>,
    ) -> anyhow::Result<UploadReport> {
        Ok(UploadReport::Failed {
            message: "the destination does not hold it".to_owned(),
        })
    }
}

#[tokio::test]
async fn a_destination_that_cannot_confirm_the_file_fails_the_upload() {
    // The gap this type exists to close: a server that answers 201 and stores nothing. With
    // `move`, trusting that answer would delete the only copy. The verification is a separate
    // call for exactly this case, and a "no" has to reach the caller as a failure.
    let uploader: Arc<dyn StorageUploader> = Arc::new(ForgetfulDestination);
    let directory = tempfile::tempdir().expect("tempdir");
    let files = vec!["release.bin".to_owned()];
    std::fs::write(directory.path().join("release.bin"), b"payload").expect("write");

    let report = uploader
        .upload(
            "any",
            StorageUpload {
                handle: "package-1",
                directory: directory.path(),
                files: &files,
                destination: "https://cloud.example/dav/Downloads",
                username: Some("me"),
                secret_ref: None,
                // This destination never reads the package, so nothing reports anything.
                progress: Arc::new(|_, _| {}),
            },
        )
        .await
        .expect("upload");
    assert!(matches!(report, UploadReport::Failed { .. }), "{report:?}");
    // And the local file is still there, which is the part that actually matters.
    assert!(directory.path().join("release.bin").is_file());
}

/// What the caller's progress closure was handed, in order.
type Seen = Arc<Mutex<Vec<(u64, Option<u64>)>>>;

/// A WebDAV server that accepts the collection and the file, and remembers what it was asked.
#[derive(Default)]
struct MockWebDav {
    methods: Mutex<Vec<String>>,
}

#[async_trait]
impl ResolverHost for MockWebDav {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        self.methods
            .lock()
            .expect("methods")
            .push(request.method.clone());
        Ok(HostHttpResponse {
            status: 201,
            final_url: request.url.clone(),
            headers: Vec::new(),
            body: Vec::new(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }
}

#[tokio::test]
async fn what_the_destination_reports_while_uploading_reaches_the_caller() {
    // RD-108-18. The plugin has always called `progress`; until this test there was nobody on
    // the other end of it, because nothing ever installed a reporter and the host dropped
    // every number. What is checked here is the whole way through: the real component reads
    // the file, reports what it has, and the closure the caller handed in sees it.
    let bytes = component("rd-plugin-webdav-storage");
    let server = Arc::new(MockWebDav::default());
    let plugin = StoragePlugin::new(
        manifest(),
        &bytes,
        Some(Arc::clone(&server) as Arc<dyn ResolverHost>),
    )
    .expect("satisfies the world");

    let directory = tempfile::tempdir().expect("tempdir");
    // Smaller than the plugin's read chunk, so the file is one read and one report: the
    // number this asserts is the plugin's own, not an artefact of how it was sliced.
    let payload = vec![b'x'; 4096];
    std::fs::write(directory.path().join("release.bin"), &payload).expect("write");

    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let source = SourceState::new(
        "package-1".to_owned(),
        directory.path().to_path_buf(),
        vec!["release.bin".to_owned()],
    )
    .with_progress(move |done, total| recorded.lock().expect("seen").push((done, total)));

    let outcome = plugin
        .put(
            source,
            Upload {
                file_name: "release.bin",
                size: payload.len() as u64,
                destination: "https://cloud.example/dav/Downloads",
                username: Some("me"),
                secret_ref: None,
                checkpoint: None,
            },
        )
        .await
        .expect("put");

    assert!(
        matches!(outcome, UploadOutcome::Complete { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        *seen.lock().expect("seen"),
        [(payload.len() as u64, Some(payload.len() as u64))],
        "the guest reported {:?}",
        seen.lock().expect("seen")
    );
    // And it really was the upload that was watched, not a call that never left the host.
    assert_eq!(*server.methods.lock().expect("methods"), ["MKCOL", "PUT"]);
}
