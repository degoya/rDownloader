//! MEGA end to end, through both bundled components (RD-103-02, ADR 0011).
//!
//! What is proven here is what a person pasting a MEGA address is promised: a folder comes
//! back with names, sizes and structure; a file comes back with its name, its size, the
//! address its ciphertext is at and the whole key schedule the host needs to write plaintext;
//! a key that does not belong to a file is refused before a byte is fetched; and **no request
//! this plugin makes ever carries the key**, because MEGA is the party that must never see it.
//!
//! **The provider is a mock.** It answers at the host boundary, so no socket is opened and
//! mega.nz is not contacted. Every body it answers with was recorded from the public API on
//! 2026-09-22 and sanitised: the two public examples the job file names, from somebody else's
//! published documentation, with the storage path replaced. No account, no private link and
//! no real credential is in this tree.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, HostHttpRequest, HostHttpResponse, ResolveRequest, ResolverHost,
};
use rd_plugin_host::{
    PluginManifest,
    extension::{FolderCrawler, StreamTransformProvider},
};

const TRANSFORM_MANIFEST: &str = include_str!("../../../plugins/mega/manifest.toml");
const CRAWLER_MANIFEST: &str = include_str!("../../../plugins/mega-crawler/manifest.toml");

/// The public example file and its fragment key, from `justaprudev/pymegatools`.
const FILE_HANDLE: &str = "yuZ0QJ6J";
const FILE_KEY: &str = "jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc";
/// The public example folder and its share key, from `tonikelope/megabasterd` issue 215.
const FOLDER_HANDLE: &str = "e4diDZ7T";
const FOLDER_KEY: &str = "iJnegBO_m6OXBQp27lHCrg";

fn file_url() -> String {
    format!("https://mega.nz/file/{FILE_HANDLE}#{FILE_KEY}")
}

fn folder_url() -> String {
    format!("https://mega.nz/folder/{FOLDER_HANDLE}#{FOLDER_KEY}")
}

fn child_url() -> String {
    format!("https://mega.nz/folder/{FOLDER_HANDLE}#{FOLDER_KEY}/file/KlVgwR4B")
}

/// The `a=g` answer for the public example file, storage path replaced.
const FILE_ANSWER: &str = r#"[{"s":10000000,"at":"TKoehGVWQMtDWZz2bJxYF608OUaRdsuZ_FUtTWIo3Go4aCyL_5BXAvF7Vp4wge8uFCzGzOB18FN1QCVojMBj2w","msd":1,"g":"https://gfs262n326.userstorage.mega.co.nz/dl/PLACEHOLDER","ip":["192.0.2.1"],"fh":"2fkJyP3kzJY"}]"#;

/// The `a=f` answer for the public example folder.
const FOLDER_ANSWER: &str = r#"[{"f":[{"h":"G5NikTgR","p":"39FwkLpK","u":"6W7cY6mgeJM","t":1,"a":"rFX1jqYOqf_FQim2hwhNx4e0RARSkwhVjpySSe5welQ","k":"G5NikTgR:pR93bkC1OGslo_O5ugTeWw","ts":1632475428},{"h":"KlVgwR4B","p":"G5NikTgR","u":"6W7cY6mgeJM","t":0,"a":"2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag","k":"G5NikTgR:IGAHl28DUprdBdTeyLINGudDKvL51FH5NNTTHdSqC4Q","s":523265,"ts":1632475461},{"h":"zwNiSB7J","p":"G5NikTgR","u":"6W7cY6mgeJM","t":1,"a":"bAMOwUGKrJzOHaWpoJEta9ATY54OrnrM1MdM18UevI4","k":"G5NikTgR:jRCDoNOtdI1WwR6-rOtbWg/zwNiSB7J:Gc71mTjFO44hyqSjItIL9g","ts":1632475524}],"sn":"54_AmP_AxTw","noc":1}]"#;

/// The `a=g` answer for the file inside that folder, storage path replaced.
const CHILD_ANSWER: &str = r#"[{"s":523265,"at":"2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag","msd":1,"g":"https://gfs270n505.userstorage.mega.co.nz/dl/PLACEHOLDER"}]"#;

/// The command endpoint, as far as an unauthenticated caller reaches it.
struct MockMega {
    /// Every request, as `<method> <url> <body>`.
    requests: Mutex<Vec<String>>,
    /// When set, every call answers with this MEGA error number instead.
    error: Option<i64>,
    /// When set, every call answers with this HTTP status instead.
    status: Option<u16>,
    /// When set, every call answers with this body instead.
    body: Option<String>,
    /// When set, the call never answers at all.
    hang: bool,
    /// A header every refusal carries, for the countdowns a provider states in one.
    wait_header: Option<(String, String)>,
}

impl MockMega {
    fn new() -> Arc<Self> {
        Arc::new(Self::bare())
    }

    fn answering(body: String) -> Arc<Self> {
        Arc::new(Self {
            body: Some(body),
            ..Self::bare()
        })
    }

    /// A provider that takes the request and never comes back.
    fn hanging() -> Arc<Self> {
        Arc::new(Self {
            hang: true,
            ..Self::bare()
        })
    }

    fn bare() -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            error: None,
            status: None,
            body: None,
            hang: false,
            wait_header: None,
        }
    }

    fn failing(error: i64) -> Arc<Self> {
        Arc::new(Self {
            error: Some(error),
            ..Self::bare()
        })
    }

    fn refusing(status: u16) -> Arc<Self> {
        Arc::new(Self {
            status: Some(status),
            ..Self::bare()
        })
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }
}

#[async_trait]
impl ResolverHost for MockMega {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, Failure> {
        let body = String::from_utf8_lossy(&request.body).into_owned();
        self.requests
            .lock()
            .expect("requests")
            .push(format!("{} {} {body}", request.method, request.url));
        let headers: Vec<_> = self.wait_header.clone().into_iter().collect();
        let answer = |status: u16, text: &str| {
            Ok(HostHttpResponse {
                status,
                final_url: request.url.clone(),
                headers: headers
                    .iter()
                    .map(|(name, value)| rd_plugin_api::ResolvedHeader {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect(),
                body: text.as_bytes().to_vec(),
            })
        };
        if self.hang {
            // Never answers. What ends the call is the caller deciding to stop waiting.
            std::future::pending::<()>().await;
        }
        if let Some(status) = self.status {
            return answer(status, "<html>no</html>");
        }
        if let Some(body) = &self.body {
            return answer(200, body);
        }
        if let Some(error) = self.error {
            return answer(200, &format!("[{error}]"));
        }
        // Every answer is `200`; which one it is follows from the command, exactly as the
        // real endpoint decides it.
        if body.contains(r#""a":"f""#) {
            return answer(200, FOLDER_ANSWER);
        }
        if body.contains(r#""n":"KlVgwR4B""#) {
            return answer(200, CHILD_ANSWER);
        }
        if body.contains(&format!(r#""p":"{FILE_HANDLE}""#)) {
            return answer(200, FILE_ANSWER);
        }
        answer(200, "[-9]")
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }
}

fn transform(host: Arc<MockMega>) -> StreamTransformProvider {
    let manifest: PluginManifest = toml::from_str(TRANSFORM_MANIFEST).expect("the manifest");
    let bytes = rd_plugin_host::artifact::component("rd-plugin-mega");
    StreamTransformProvider::new(manifest, &bytes, Some(host)).expect("compile the plugin")
}

fn crawler(host: Arc<MockMega>) -> FolderCrawler {
    let manifest: PluginManifest = toml::from_str(CRAWLER_MANIFEST).expect("the manifest");
    let bytes = rd_plugin_host::artifact::component("rd-plugin-mega-crawler");
    FolderCrawler::new(manifest, &bytes, Some(host)).expect("compile the crawler")
}

/// The same crawler, from a manifest a test narrowed. The component is the shipped one; only
/// the limits it runs under differ, which is exactly what a bound has to be tested against.
fn crawler_limited(host: Arc<MockMega>, replace: &[(&str, &str)]) -> FolderCrawler {
    let mut text = CRAWLER_MANIFEST.to_owned();
    for (from, to) in replace {
        assert!(text.contains(from), "the manifest no longer says {from:?}");
        text = text.replace(from, to);
    }
    let manifest: PluginManifest = toml::from_str(&text).expect("the manifest");
    let bytes = rd_plugin_host::artifact::component("rd-plugin-mega-crawler");
    FolderCrawler::new(manifest, &bytes, Some(host)).expect("compile the crawler")
}

/// A folder listing of `files` files, built out of the one real node the fixture holds.
///
/// Only the handles differ. A node's key and attribute block decrypt on their own, without
/// reference to which handle they sit under, so repeating them is a listing of the right
/// *shape* rather than ciphertext nobody's key opens. What is being measured is the count.
fn crowded_folder(files: usize) -> String {
    let mut nodes = vec![
        r#"{"h":"G5NikTgR","p":"39FwkLpK","u":"6W7cY6mgeJM","t":1,"a":"rFX1jqYOqf_FQim2hwhNx4e0RARSkwhVjpySSe5welQ","k":"G5NikTgR:pR93bkC1OGslo_O5ugTeWw","ts":1632475428}"#
            .to_owned(),
    ];
    for index in 0..files {
        nodes.push(format!(
            r#"{{"h":"N{index:07}","p":"G5NikTgR","u":"6W7cY6mgeJM","t":0,"a":"2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag","k":"G5NikTgR:IGAHl28DUprdBdTeyLINGudDKvL51FH5NNTTHdSqC4Q","s":523265,"ts":1632475461}}"#
        ));
    }
    format!(
        r#"[{{"f":[{}],"sn":"54_AmP_AxTw","noc":1}}]"#,
        nodes.join(",")
    )
}

fn request(url: &str) -> ResolveRequest {
    ResolveRequest {
        url: url.parse().expect("an address"),
        client: ClientIdentity {
            account_id: None,
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

#[tokio::test]
async fn each_plugin_claims_its_own_half_and_nothing_else() {
    let host = MockMega::new();
    let transform = transform(host.clone());
    let crawler = crawler(host.clone());

    assert!(transform.claims(&file_url()).await.expect("asked"));
    assert!(transform.claims(&child_url()).await.expect("asked"));
    assert!(!transform.claims(&folder_url()).await.expect("asked"));

    assert!(crawler.claims(&folder_url()).await.expect("asked"));
    assert!(!crawler.claims(&file_url()).await.expect("asked"));
    assert!(!crawler.claims(&child_url()).await.expect("asked"));

    for foreign in [
        "https://example.com/file/abc#key",
        "https://mega.nz/register",
        // No fragment, no key: nothing here can be decrypted, so neither plugin claims it.
        "https://mega.nz/file/yuZ0QJ6J",
    ] {
        assert!(
            !transform.claims(foreign).await.expect("asked"),
            "{foreign}"
        );
        assert!(!crawler.claims(foreign).await.expect("asked"), "{foreign}");
    }
    // Asking cost nothing: a plugin that does not own an address never fetches it.
    assert!(host.requests().is_empty(), "{:?}", host.requests());
}

#[tokio::test]
async fn a_public_file_resolves_to_an_address_and_a_whole_key_schedule() {
    let host = MockMega::new();
    let answer = transform(host.clone())
        .resolve(&request(&file_url()))
        .await
        .expect("the call finished")
        .expect("a description");

    assert_eq!(answer.download.file_name.as_deref(), Some("10MB.bin"));
    assert_eq!(
        answer.download.size.map(rd_core::ByteCount::get),
        Some(10_000_000)
    );
    assert!(
        answer
            .download
            .url
            .host_str()
            .expect("a host")
            .ends_with(".userstorage.mega.co.nz")
    );
    // Nothing rides along in a header: a header is replayed on every chunk request and
    // written to the log with it.
    assert!(answer.download.headers.is_empty());

    assert_eq!(answer.transform.cipher.algorithm, "aes-128-ctr");
    assert_eq!(answer.transform.cipher.first_block, 0);
    assert_eq!(answer.transform.cipher.nonce.len(), 8);
    // The description never carries the key; the host puts it away and keeps a reference.
    assert_eq!(answer.transform.cipher.key_reference, None);
    assert_eq!(hex(answer.key.expose()), "0c4c44e128eaee7a40bcbd4ffec19617");

    let integrity = answer.transform.integrity.expect("an expected value");
    assert_eq!(integrity.algorithm, "cbc-mac-chain");
    // MEGA's own chunk layout for a 10 000 000-byte file: fourteen chunks.
    assert_eq!(integrity.boundaries.len(), 14);
    assert_eq!(*integrity.boundaries.last().expect("last"), 10_000_000);
    assert_eq!(integrity.iv.len(), 16);
    assert_eq!(hex(&integrity.expected), "95ef644bf6dbda07");
    // One call, and the key was not in it.
    let requests = host.requests();
    assert_eq!(requests.len(), 1);
    assert_no_key(&requests);
}

#[tokio::test]
async fn a_public_folder_comes_back_with_names_sizes_and_structure() {
    let host = MockMega::new();
    let links = crawler(host.clone())
        .crawl(&folder_url(), None)
        .await
        .expect("the call finished")
        .expect("a listing");

    assert_eq!(links.len(), 1, "one file, two folders");
    assert_eq!(links[0].file_name.as_deref(), Some("SharedFile.jpg"));
    assert_eq!(links[0].size, Some(523_265));
    // The shared folder's own name, which the host reads as the package suggestion.
    assert_eq!(links[0].package_hint.as_deref(), Some("SharedFolder"));
    // The address keeps the folder form, because a node's key lives in the listing and in no
    // address MEGA defines.
    assert_eq!(links[0].url.as_str(), child_url());
    assert_no_key(&host.requests());
}

#[tokio::test]
async fn a_file_inside_a_folder_takes_its_key_out_of_the_listing() {
    let host = MockMega::new();
    let answer = transform(host.clone())
        .resolve(&request(&child_url()))
        .await
        .expect("the call finished")
        .expect("a description");

    assert_eq!(answer.download.file_name.as_deref(), Some("SharedFile.jpg"));
    assert_eq!(
        answer.download.size.map(rd_core::ByteCount::get),
        Some(523_265)
    );
    assert_eq!(answer.key.len(), 16);
    let integrity = answer.transform.integrity.expect("an expected value");
    assert_eq!(*integrity.boundaries.last().expect("last"), 523_265);
    // Two calls: the listing for the key, then the address.
    let requests = host.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(requests[0].contains(r#""a":"f""#), "{requests:?}");
    assert!(requests[1].contains(r#""n":"KlVgwR4B""#), "{requests:?}");
    // Both carried the folder handle as the query parameter, which is what makes a folder
    // child reachable at all -- measured: `p=` answers `-9` for one.
    assert!(
        requests.iter().all(|line| line.contains("?n=e4diDZ7T")),
        "{requests:?}"
    );
    assert_no_key(&requests);
}

#[tokio::test]
async fn a_key_that_does_not_belong_to_the_file_is_refused_before_any_byte_is_fetched() {
    let host = MockMega::new();
    // A well-formed 43-character key that is not this file's: the attribute block turns to
    // noise under it, which is the check.
    let wrong = format!("https://mega.nz/file/{FILE_HANDLE}#{}", "A".repeat(43));
    let failure = transform(host.clone())
        .resolve(&request(&wrong))
        .await
        .expect("the call finished")
        .expect_err("a refusal");
    assert_eq!(failure.code.as_deref(), Some("mega.attributes_unreadable"));
}

#[tokio::test]
async fn the_measured_refusals_reach_the_interface_with_their_own_codes() {
    for (number, code) in [
        (-9_i64, "mega.not_found"),
        (-11, "mega.access_denied"),
        (-15, "mega.session_required"),
        (-4, "mega.rate_limited"),
        (-17, "mega.quota_exceeded"),
        (-18, "mega.unavailable"),
    ] {
        let failure = transform(MockMega::failing(number))
            .resolve(&request(&file_url()))
            .await
            .expect("the call finished")
            .expect_err("a refusal");
        assert_eq!(failure.code.as_deref(), Some(code), "for {number}");
    }
    let failure = crawler(MockMega::failing(-9))
        .crawl(&folder_url(), None)
        .await
        .expect("the call finished")
        .expect_err("a refusal");
    assert_eq!(
        failure.code.as_deref(),
        Some("mega_crawler.folder_unreachable")
    );
}

#[tokio::test]
async fn a_status_that_is_not_an_answer_is_reported_as_one() {
    let failure = transform(MockMega::refusing(503))
        .resolve(&request(&file_url()))
        .await
        .expect("the call finished")
        .expect_err("a refusal");
    assert_eq!(failure.code.as_deref(), Some("mega.http_error"));
}

/// Every spelling the key could survive as, in anything the plugin sent to MEGA.
///
/// This is the half of "decryption keys never appear in logs" that no host-side canary can
/// prove: MEGA is the one party that must never learn the key, and a request body is where a
/// careless plugin would put it.
fn assert_no_key(requests: &[String]) {
    let raw = mega_common::crypto::b64_decode(FILE_KEY).expect("the fragment");
    let key = mega_common::crypto::FileKey::from_raw(&raw).expect("it folds");
    let spellings = [
        FILE_KEY.to_owned(),
        FOLDER_KEY.to_owned(),
        hex(&key.key),
        hex(&key.key).to_uppercase(),
        mega_common::crypto::b64_encode(&key.key),
        hex(&key.nonce),
    ];
    for line in requests {
        for spelling in &spellings {
            assert!(
                !line.contains(spelling),
                "a request carried {spelling:?}: {line}"
            );
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// **The key comes back** (RD-110-38), through the real components and nothing stubbed.
///
/// The whole chain, in the order the application runs it: the manifest declares the hosts
/// whose fragment is key material, the provider registry is filled from that declaration
/// alone, the intake splits the pasted address into the one it stores and the one thing it
/// must not store, the vault takes the second under a reference — and the resolver is asked
/// with the address rebuilt from it. What the plugin answers with is the same key schedule
/// the address carried in, which is the only proof that the detour through the vault changed
/// nothing but where the key was in the meantime.
#[tokio::test]
async fn a_vaulted_fragment_reaches_the_resolver_and_yields_the_same_key() {
    let manifest: PluginManifest = toml::from_str(TRANSFORM_MANIFEST).expect("the manifest");
    // Nothing here names a service: the hosts come out of the manifest the plugin ships with.
    assert!(
        manifest
            .secret_fragment_domains
            .contains(&"mega.nz".to_owned()),
        "the MEGA manifest has to declare its fragment: {:?}",
        manifest.secret_fragment_domains
    );
    rd_provider_registry::replace_secret_fragment_hosts(manifest.secret_fragment_domains.clone());

    let pasted: url::Url = file_url().parse().expect("an address");
    let declared = rd_provider_registry::fragment_is_secret(&pasted);
    assert!(declared, "the registry has to answer from the manifest");
    let (stored, secret) = rd_core::split_candidate_url(&pasted, declared);

    // What a candidate row would hold: the handle, and no key at all.
    assert_eq!(stored.as_str(), "https://mega.nz/file/yuZ0QJ6J");
    assert!(!stored.as_str().contains(FILE_KEY));

    let directory = tempfile::tempdir().expect("tempdir");
    let vault = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("a vault");
    let reference = vault
        .put_bytes(secret.expect("the fragment is kept").as_bytes())
        .await
        .expect("put");
    assert!(reference.starts_with("vault://"));

    // Resolve time: the host reads the reference back and restores the fragment onto the
    // address a single call before it hands it to the plugin.
    let fragment =
        String::from_utf8(vault.get_bytes(&reference).await.expect("get")).expect("utf-8");
    let mut restored = stored.clone();
    restored.set_fragment(Some(&fragment));

    let host = MockMega::new();
    let answer = transform(host.clone())
        .resolve(&request(restored.as_str()))
        .await
        .expect("the call finished")
        .expect("a description");

    // Byte for byte what the same address resolves to when it is pasted whole.
    assert_eq!(hex(answer.key.expose()), "0c4c44e128eaee7a40bcbd4ffec19617");
    assert_eq!(answer.transform.cipher.algorithm, "aes-128-ctr");
    assert_eq!(answer.download.file_name.as_deref(), Some("10MB.bin"));
    let integrity = answer.transform.integrity.expect("an expected value");
    assert_eq!(hex(&integrity.expected), "95ef644bf6dbda07");
    // And MEGA still never saw the key.
    assert_no_key(&host.requests());
}

// -- criterion 3: a continuation has to know the file it is continuing ----
//
// "Resume validates node and file identity" is decided by one value: the fingerprint of the
// description, which `rd-http` compares a checkpoint's chunk MACs against before adopting a
// single one. What it has to get right is both directions at once. MEGA's storage address is
// answered fresh per call and is *never* the same twice, so a resume that keyed on the
// address would restart every time. Node, key and size are what actually say which file this
// is, and all three are in the fingerprint through the nonce, the vault reference, the
// condensed value and the chunk boundaries.

/// The reference the host would have put the key away under. The same for both sides of a
/// comparison, so what is compared is the file's identity and not the vault's bookkeeping.
const KEY_REFERENCE: &str = "vault://019d0000-0000-7000-8000-0000000000aa";

async fn fingerprint_of(host: Arc<MockMega>, url: &str) -> (String, String) {
    let answer = transform(host)
        .resolve(&request(url))
        .await
        .expect("the call finished")
        .expect("a description");
    let address = answer.download.url.to_string();
    (
        answer
            .transform
            .with_key_reference(KEY_REFERENCE)
            .fingerprint(),
        address,
    )
}

/// MEGA hands out a different storage path every time it is asked -- measured, twice, and
/// recorded in the job file. A resume must survive that, or no MEGA download ever continues.
#[tokio::test]
async fn a_second_resolve_of_the_same_file_keeps_its_fingerprint_though_the_address_moved() {
    let (first, first_address) = fingerprint_of(MockMega::new(), &file_url()).await;
    let moved = FILE_ANSWER.replace(
        "https://gfs262n326.userstorage.mega.co.nz/dl/PLACEHOLDER",
        "https://gfs919n007.userstorage.mega.co.nz/dl/SOMEWHERE-ELSE",
    );
    let (second, second_address) = fingerprint_of(MockMega::answering(moved), &file_url()).await;

    assert_ne!(
        first_address, second_address,
        "the fixture has to move the address, or this proves nothing"
    );
    assert_eq!(
        first, second,
        "the same file at a new address is the same file; a resume keeps its chunk MACs"
    );
}

/// The other direction, and the one the criterion is actually about: the row stayed, the file
/// behind it did not. Three ways that happens, and each has to change the fingerprint.
#[tokio::test]
async fn a_different_node_key_or_size_is_a_different_fingerprint() {
    let (baseline, _) = fingerprint_of(MockMega::new(), &file_url()).await;

    // A different node entirely -- the file inside the shared folder.
    let (other_node, _) = fingerprint_of(MockMega::new(), &child_url()).await;
    assert_ne!(baseline, other_node, "another node is another file");

    // The same node, re-resolved at a different length. MEGA's chunk layout follows the size,
    // so the boundaries move and the condensed value is computed over a different partition.
    let resized = FILE_ANSWER.replace(r#""s":10000000"#, r#""s":9000000"#);
    let (other_size, _) = fingerprint_of(MockMega::answering(resized), &file_url()).await;
    assert_ne!(
        baseline, other_size,
        "the same handle at another length is not the file the part file holds"
    );

    // And the vault reference, which is where a changed *key* reaches the fingerprint: the
    // host writes a new reference for a key that is not the stored one (RD-120-11).
    let answer = transform(MockMega::new())
        .resolve(&request(&file_url()))
        .await
        .expect("the call finished")
        .expect("a description");
    assert_ne!(
        answer
            .transform
            .with_key_reference("vault://019d0000-0000-7000-8000-0000000000bb")
            .fingerprint(),
        baseline,
        "a key put away under a new reference is a new fingerprint"
    );
}

// -- criterion 4: a folder that is too big, and one nobody waits for ------
//
// MEGA answers `a=f` with every node at once and offers no cursor -- measured on 2026-09-21
// and again on 2026-09-22. So "paginated" here cannot mean following pages; it means the one
// answer is bounded, and bounded in both of the places it can grow: the bytes the host will
// accept before the guest sees them, and the files the guest will hand back afterwards.

/// The guest's own bound. Five thousand is well past any folder shared by hand.
#[tokio::test]
async fn a_folder_past_the_local_limit_is_refused_rather_than_handed_over() {
    let host = MockMega::answering(crowded_folder(1_001));
    let refusal = crawler(host)
        .crawl(&folder_url(), None)
        .await
        .expect("the call finished")
        .expect_err("a refusal");
    assert_eq!(
        refusal.code.as_deref(),
        Some("mega_crawler.too_many_files"),
        "{refusal:?}"
    );
    assert!(!refusal.not_mine);
}

/// A folder exactly at the limit comes back **whole**, and that is the point of the limit
/// being the host's own: one more file is a refusal the person reads, never a package quietly
/// missing its tail. The host trims a crawler's answer without saying so, so the only way the
/// two can disagree is if the plugin's number is the larger one -- and now it is not.
#[tokio::test]
async fn a_folder_exactly_at_the_limit_comes_back_whole_and_untrimmed() {
    let host = MockMega::answering(crowded_folder(rd_plugin_host::extension::MAX_CRAWLED_LINKS));
    let links = crawler(host)
        .crawl(&folder_url(), None)
        .await
        .expect("the call finished")
        .expect("a listing");
    assert_eq!(links.len(), rd_plugin_host::extension::MAX_CRAWLED_LINKS);
    assert!(links.iter().all(|link| link.size == Some(523_265)));
}

/// The other bound, and the earlier one: the host counts the bytes and stops before a single
/// one of them reaches the guest. Measured against a manifest narrowed for this test, because
/// the shipped 16 MiB would mean allocating 16 MiB to prove a comparison.
#[tokio::test]
async fn a_listing_larger_than_the_manifest_allows_never_reaches_the_guest() {
    let host = MockMega::answering(crowded_folder(400));
    let crawler = crawler_limited(
        host.clone(),
        &[("max_response_bytes = 16777216", "max_response_bytes = 4096")],
    );
    let refusal = crawler
        .crawl(&folder_url(), None)
        .await
        .expect("the call finished")
        .expect_err("a refusal");
    // Whatever it is called, it is not a listing: the guest was handed a failure by the host
    // and passed it on rather than parsing a truncated document.
    assert_ne!(refusal.code.as_deref(), Some("mega_crawler.folder_empty"));
    assert_eq!(host.requests().len(), 1, "one request, and it was refused");
}

/// Cancellable: the call is one `await`, so a caller that stops waiting ends it. Nothing in
/// the guest loops between requests and nothing has to be told to stop -- there is exactly one
/// request, and dropping the future drops the store with it.
#[tokio::test]
async fn a_crawl_the_caller_stops_waiting_for_is_abandoned() {
    let host = MockMega::hanging();
    let crawler = crawler(host.clone());
    let outcome = tokio::time::timeout(
        std::time::Duration::from_millis(250),
        crawler.crawl(&folder_url(), None),
    )
    .await;
    assert!(outcome.is_err(), "the crawl must not have finished");
    assert_eq!(host.requests().len(), 1, "it got as far as asking");
}

// -- the cross-cutting rate-limit rule, over the real components ----------

/// A `509` is MEGA's bandwidth refusal and it carries its own countdown. Passing that number
/// on is the difference between a scheduled retry and a loop.
#[tokio::test]
async fn the_bandwidth_refusal_carries_the_wait_the_provider_asked_for() {
    let host = Arc::new(MockMega {
        status: Some(509),
        wait_header: Some(("X-MEGA-Time-Left".to_owned(), "1800".to_owned())),
        ..MockMega::bare()
    });
    let failure = transform(host)
        .resolve(&request(&file_url()))
        .await
        .expect("the call finished")
        .expect_err("a refusal");
    assert_eq!(failure.code.as_deref(), Some("mega.quota_exceeded"));
    assert_eq!(
        failure.category,
        rd_core::FailureKind::RateLimited {
            retry_after_seconds: Some(1800)
        }
    );
}

#[tokio::test]
async fn a_plain_rate_limit_passes_the_standard_header_on() {
    let host = Arc::new(MockMega {
        status: Some(429),
        wait_header: Some(("Retry-After".to_owned(), "45".to_owned())),
        ..MockMega::bare()
    });
    let refusal = crawler_limited(host, &[])
        .crawl(&folder_url(), None)
        .await
        .expect("the call finished")
        .expect_err("a refusal");
    assert_eq!(refusal.code.as_deref(), Some("mega_crawler.rate_limited"));
}

/// **The seam this job found broken**, end to end and with nothing stubbed.
///
/// RD-110-33 built both ends of the key's journey and left the middle out. The host adapter
/// answers `key_reference: None` with a note that the caller fills it in once the bytes are
/// in the vault; no caller did. `StreamTransform::new` refuses a description without one --
/// deliberately, because the reference is what the fingerprint stands the key on -- so every
/// transformed download ended as `transform.key_missing` before a byte was fetched, and
/// nothing said so, because no test ran the real component's answer into the real engine.
///
/// This is that test. The refusal is asserted first, so the gap stays visible as a property
/// rather than as a bug somebody once fixed.
#[tokio::test]
async fn what_the_component_answers_becomes_a_transform_the_engine_accepts() {
    let answer = transform(MockMega::new())
        .resolve(&request(&file_url()))
        .await
        .expect("the call finished")
        .expect("a description");

    // As the plugin host hands it over: the key, and nowhere to have put it.
    // `.err()` rather than `expect_err`: a `StreamTransform` has no `Debug` at all, which is
    // itself part of the criterion below -- the type cannot be printed because it holds a key.
    let refusal = rd_http::StreamTransform::new(answer.transform.clone(), &answer.key)
        .err()
        .expect("a description with no reference is refused");
    assert_eq!(refusal.code.as_deref(), Some("transform.key_missing"));

    // What the scheduler now does before it builds one: the key goes to the vault, and the
    // reference goes into the description.
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("a vault");
    let reference = vault
        .put_bytes(answer.key.expose())
        .await
        .expect("put the key away");
    let described = answer
        .transform
        .clone()
        .with_key_reference(reference.clone());
    let stream = rd_http::StreamTransform::new(described, &answer.key).expect("a usable transform");

    // It is the file the fixture describes, and it knows MEGA's own chunk layout.
    assert_eq!(stream.expected_size(), Some(10_000_000));
    assert!(stream.mac_walker(0).is_some());
    // The fingerprint is the reference's as well as the file's -- which is what makes a
    // re-resolve under the same stored key resume rather than start over.
    let again =
        rd_http::StreamTransform::new(answer.transform.with_key_reference(reference), &answer.key)
            .expect("a usable transform");
    assert_eq!(stream.fingerprint(), again.fingerprint());

    // And the key is still not anywhere MEGA could have seen it.
    assert!(
        !format!("{:?}", stream.description()).contains(&hex(answer.key.expose())),
        "the description must not render the key"
    );
}
