//! A file of the signed-in MEGA account, end to end through the shipped components
//! (RD-120-30, closing RD-120-11's criterion 1).
//!
//! **The provider is a mock and the account is invented.** Nothing opens a socket and mega.nz
//! is not contacted. The account's node list is the public example folder and file the job
//! file names, with one change: every key is filed under an invented owner and re-wrapped under
//! an invented master key, the way MEGA files the keys of an account's own nodes. The names,
//! sizes, attribute blocks and `a=g` answers are the ones recorded from the public API on
//! 2026-09-22, so the file key that comes back is checked against a measurement and the bytes
//! at the end decrypt to a vector computed outside this repository. No real account, no real
//! session and no credential of any person is in this tree.
//!
//! The mock is also the vault: `derive_from_secret` holds the master key, applies the host's
//! own rule for sign-in key material (`keyderive::validate` with `SecretOrigin::SignIn`) and
//! runs the host's own arithmetic. That the *real* host decides the origin from the slot and
//! reads the key from `auth_flows.key_ref` is proved in `rd-plugin-host`'s
//! `native::signin::signin_tests`; this file proves the components ask for exactly that.

use std::sync::{Arc, Mutex};

use aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit},
};
use async_trait::async_trait;
use mega_common::crypto;
use rd_core::{AccountId, Failure, FailureKind};
use rd_plugin_api::{
    ClientIdentity, DerivationStep, HostHttpRequest, HostHttpResponse, ResolveRequest, ResolverHost,
};
use rd_plugin_host::{
    PluginManifest,
    extension::{FolderCrawler, StreamTransformProvider},
    keyderive::{self, SecretOrigin},
};

const TRANSFORM_MANIFEST: &str = include_str!("../../../plugins/mega/manifest.toml");
const CRAWLER_MANIFEST: &str = include_str!("../../../plugins/mega-crawler/manifest.toml");

const OWNER: &str = "6W7cY6mgeJM";
const MASTER_KEY: [u8; 16] = *b"invented-master!";
const SESSION_MARKER: &str = "{{secret:mega_session}}";
/// What the host would substitute for the marker. Never handed to the mock: the point is that
/// no request the component builds carries it.
const SESSION_ID: &str = "an-invented-session-identifier";

/// The public example file: its handle, the 32-byte key its link carried, its `at` block.
const FILE: &str = "yuZ0QJ6J";
const FILE_KEY: &str = "jFc2HL6rIoDVU9kECBpMEIAbcv2WQcz6le9kS_bb2gc";
const FILE_ATTRIBUTES: &str =
    "TKoehGVWQMtDWZz2bJxYF608OUaRdsuZ_FUtTWIo3Go4aCyL_5BXAvF7Vp4wge8uFCzGzOB18FN1QCVojMBj2w";
const FILE_ANSWER: &str = r#"[{"s":10000000,"at":"TKoehGVWQMtDWZz2bJxYF608OUaRdsuZ_FUtTWIo3Go4aCyL_5BXAvF7Vp4wge8uFCzGzOB18FN1QCVojMBj2w","msd":1,"g":"https://gfs262n326.userstorage.mega.co.nz/dl/PLACEHOLDER"}]"#;

/// The public example folder and the file in it, with the share key their link carried.
const FOLDER: &str = "G5NikTgR";
const SHARE_KEY: &str = "iJnegBO_m6OXBQp27lHCrg";
const FOLDER_ATTRIBUTES: &str = "rFX1jqYOqf_FQim2hwhNx4e0RARSkwhVjpySSe5welQ";
const FOLDER_KEY_UNDER_SHARE: &str = "pR93bkC1OGslo_O5ugTeWw";
const CHILD: &str = "KlVgwR4B";
const CHILD_ATTRIBUTES: &str =
    "2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag";
const CHILD_KEY_UNDER_SHARE: &str = "IGAHl28DUprdBdTeyLINGudDKvL51FH5NNTTHdSqC4Q";
const CHILD_ANSWER: &str = r#"[{"s":523265,"at":"2gzMr8ViBX94FUcSKuQGUpeq_B2THl9G9WkR_bMhrICeNvqGho5177W8ZZv9GmGwpuINMqNTrtkzu4boD_Kbag","msd":1,"g":"https://gfs270n505.userstorage.mega.co.nz/dl/PLACEHOLDER"}]"#;

fn ecb(key: &[u8; 16], data: &[u8], encrypt: bool) -> Vec<u8> {
    let cipher = Aes128::new(key.into());
    let mut out = data.to_vec();
    for block in out.as_chunks_mut::<16>().0 {
        if encrypt {
            cipher.encrypt_block(block.into());
        } else {
            cipher.decrypt_block(block.into());
        }
    }
    out
}

fn b64(text: &str) -> Vec<u8> {
    crypto::b64_decode(text).expect("a fixture in MEGA's base64")
}

/// A node key as the account files it: under the owner, wrapped under the master key.
fn filed(raw: &[u8]) -> String {
    format!(
        "{OWNER}:{}",
        crypto::b64_encode(&ecb(&MASTER_KEY, raw, true))
    )
}

/// The account's node list, as `a=f` with the session would answer it.
fn account_listing() -> String {
    let share: [u8; 16] = b64(SHARE_KEY).try_into().expect("a share key");
    let folder_key = ecb(&share, &b64(FOLDER_KEY_UNDER_SHARE), false);
    let child_key = ecb(&share, &b64(CHILD_KEY_UNDER_SHARE), false);
    let nodes = serde_json::json!([
        { "h": "RootNode", "p": "", "u": OWNER, "t": 2 },
        { "h": FOLDER, "p": "RootNode", "u": OWNER, "t": 1, "a": FOLDER_ATTRIBUTES,
          "k": filed(&folder_key) },
        { "h": CHILD, "p": FOLDER, "u": OWNER, "t": 0, "a": CHILD_ATTRIBUTES,
          "k": filed(&child_key), "s": 523_265 },
        { "h": FILE, "p": FOLDER, "u": OWNER, "t": 0, "a": FILE_ATTRIBUTES,
          "k": filed(&b64(FILE_KEY)), "s": 10_000_000 },
    ]);
    serde_json::json!([{ "f": nodes }]).to_string()
}

/// One derivation the host performed: the reference named, the chain, the answer.
type Derivation = (String, Vec<DerivationStep>, Vec<u8>);

/// One crawled link as the assertions compare it: address, name, size, package hint.
type LinkSummary = (String, Option<String>, Option<u64>, Option<String>);

/// MEGA's command endpoint and the host's vault, at the boundary the components see.
struct MockMega {
    /// Every request, as `<method> <url> <query templates> <body>`.
    requests: Mutex<Vec<String>>,
    /// Every derivation: the reference named, the chain, the answer.
    derivations: Mutex<Vec<Derivation>>,
    /// When set, the vault holds no session and says so, the way the real host does.
    signed_out: bool,
}

impl MockMega {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            derivations: Mutex::new(Vec::new()),
            signed_out: false,
        })
    }

    fn signed_out() -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            derivations: Mutex::new(Vec::new()),
            signed_out: true,
        })
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }

    fn derivations(&self) -> Vec<Derivation> {
        self.derivations.lock().expect("derivations").clone()
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
        let query: Vec<String> = request
            .query
            .iter()
            .map(|value| format!("{}={}", value.name, value.value_template))
            .collect();
        self.requests.lock().expect("requests").push(format!(
            "{} {} {} {body}",
            request.method,
            request.url,
            query.join("&")
        ));
        let signed = query.contains(&format!("sid={SESSION_MARKER}"));
        let text = if !signed {
            // MEGA's answer to an account call without a session.
            "[-15]".to_owned()
        } else if body.contains(r#""a":"f""#) {
            account_listing()
        } else if body.contains(&format!(r#""n":"{FILE}""#)) {
            FILE_ANSWER.to_owned()
        } else if body.contains(&format!(r#""n":"{CHILD}""#)) {
            CHILD_ANSWER.to_owned()
        } else {
            "[-9]".to_owned()
        };
        Ok(HostHttpResponse {
            status: 200,
            final_url: request.url.clone(),
            headers: Vec::new(),
            body: text.into_bytes(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        !self.signed_out
    }

    /// The vault side: the master key exists here and nowhere else.
    async fn derive_from_secret(
        &self,
        _client: &ClientIdentity,
        reference: &str,
        steps: &[DerivationStep],
    ) -> Result<Vec<u8>, Failure> {
        assert_eq!(reference, "mega_session", "the plugin named another secret");
        keyderive::validate(steps, SecretOrigin::SignIn)?;
        if self.signed_out {
            return Err(Failure::coded(
                FailureKind::AuthRequired,
                "plugin.provider_secret_missing",
                "Provider secret is missing".to_owned(),
            ));
        }
        let answer = keyderive::run(&MASTER_KEY, steps)?;
        self.derivations.lock().expect("derivations").push((
            reference.to_owned(),
            steps.to_vec(),
            answer.clone(),
        ));
        Ok(answer)
    }
}

/// Every bundled provider row, as startup registers them: the reach check the host runs before
/// any derivation compares the plugin's domains with the `mega_session` slot's.
fn register_bundled_providers() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .canonicalize()
        .expect("plugins directory");
    let rows: Vec<_> = std::fs::read_dir(root)
        .expect("read plugins")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("manifest.toml").is_file())
        .map(|path| {
            let text = std::fs::read_to_string(path.join("manifest.toml")).expect("manifest");
            toml::from_str::<PluginManifest>(&text).expect("parse manifest")
        })
        .filter_map(|manifest| rd_plugin_host::provider_spec_from_manifest(&manifest))
        .collect();
    let rejected = rd_provider_registry::replace_dynamic(rows);
    assert!(rejected.is_empty(), "rejected rows: {rejected:?}");
}

fn manifest(text: &str, replace: &[(&str, &str)]) -> PluginManifest {
    let mut text = text.to_owned();
    for (from, to) in replace {
        assert!(text.contains(from), "the manifest no longer says {from:?}");
        text = text.replace(from, to);
    }
    toml::from_str(&text).expect("the manifest")
}

fn transform_with(
    host: Arc<MockMega>,
    replace: &[(&str, &str)],
) -> anyhow::Result<StreamTransformProvider> {
    register_bundled_providers();
    let bytes = rd_plugin_host::artifact::component("rd-plugin-mega");
    StreamTransformProvider::new(manifest(TRANSFORM_MANIFEST, replace), &bytes, Some(host))
}

fn transform(host: Arc<MockMega>) -> StreamTransformProvider {
    transform_with(host, &[]).expect("compile the plugin")
}

fn crawler_with(host: Arc<MockMega>, replace: &[(&str, &str)]) -> FolderCrawler {
    register_bundled_providers();
    let bytes = rd_plugin_host::artifact::component("rd-plugin-mega-crawler");
    FolderCrawler::new(manifest(CRAWLER_MANIFEST, replace), &bytes, Some(host))
        .expect("compile the crawler")
}

fn account_file(handle: &str) -> String {
    format!("https://mega.nz/fm/file/{handle}")
}

fn request(url: &str) -> ResolveRequest {
    ResolveRequest {
        url: url.parse().expect("an address"),
        client: ClientIdentity {
            account_id: Some(AccountId::new()),
            proxy_profile_id: None,
            tls_revision: 0,
        },
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex"))
        .collect()
}

#[tokio::test]
async fn each_plugin_claims_its_own_account_form_and_neither_claims_the_others() {
    let host = MockMega::new();
    let transform = transform(host.clone());
    let crawler = crawler_with(host.clone(), &[]);
    let folder = format!("https://mega.nz/fm/{FOLDER}");
    assert!(transform.claims(&account_file(FILE)).await.expect("asked"));
    assert!(!transform.claims(&folder).await.expect("asked"));
    assert!(crawler.claims(&folder).await.expect("asked"));
    assert!(!crawler.claims(&account_file(FILE)).await.expect("asked"));
    assert!(host.requests().is_empty(), "claiming reached the network");
}

/// Acceptance criterion 5 of RD-120-30, and the account half of RD-120-11's criterion 1:
/// **a file in the account loads.** Resolved through the shipped component, handed to the
/// engine the scheduler hands it to, and its bytes decrypted.
#[tokio::test]
async fn a_file_in_the_account_resolves_and_its_bytes_decrypt() {
    let host = MockMega::new();
    let answer = transform(host.clone())
        .resolve(&request(&account_file(FILE)))
        .await
        .expect("the call finished")
        .expect("a description");

    // The measured file: the same key schedule its public link produced (RD-120-11), now
    // reached through the account instead of through a fragment.
    assert_eq!(hex(answer.key.expose()), "0c4c44e128eaee7a40bcbd4ffec19617");
    assert_eq!(hex(&answer.transform.cipher.nonce), "801b72fd9641ccfa");
    assert_eq!(answer.download.file_name.as_deref(), Some("10MB.bin"));
    assert_eq!(
        answer.download.size.map(rd_core::ByteCount::get),
        Some(10_000_000)
    );
    assert!(
        answer
            .download
            .url
            .as_str()
            .contains(".userstorage.mega.co.nz/")
    );
    let integrity = answer
        .transform
        .integrity
        .clone()
        .expect("an expected value");
    assert_eq!(hex(&integrity.expected), "95ef644bf6dbda07");

    // Two requests, both with the session as the host's marker: the node list, then `a=g`.
    let requests = host.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(requests[0].contains(r#""a":"f""#), "{}", requests[0]);
    assert!(
        requests[1].contains(&format!(r#""n":"{FILE}""#)),
        "{}",
        requests[1]
    );

    // Into the engine, exactly as the scheduler does it: key to the vault, reference into the
    // description.
    let directory = tempfile::tempdir().expect("tempdir");
    let vault = rd_secrets::SecretStore::open(directory.path().join("secrets"))
        .await
        .expect("a vault");
    let reference = vault
        .put_bytes(answer.key.expose())
        .await
        .expect("put the key away");
    let stream = rd_http::StreamTransform::new(
        answer.transform.clone().with_key_reference(reference),
        &answer.key,
    )
    .expect("a usable transform");
    assert_eq!(stream.expected_size(), Some(10_000_000));
    assert!(stream.mac_walker(0).is_some());

    // And the bytes. This ciphertext and its plaintext are the vector `rd-http`'s
    // `transform_vectors.rs` holds for these measured parameters, computed outside this
    // repository with an AES-128 checked against FIPS-197.
    let mut buffer = from_hex(concat!(
        "e5fea8e44a188a5f96110b0e3dd0d7059a9c64a992e2975f1715fcb396af5d57",
        "add203a79746f7f92665822bf2059a183b56d7768ded17af55027c2d68b758e4",
        "ee3daf0883433e880081e92028479f222d69f8c2e695942b78e6c8a16afb335e",
        "10da935d411276188853eeac32fc77254fa00620408fcc035b53734f881111f1",
        "f26a91aa2968ab354a69142e3e263db5191f914368f0bd0d2efd9ad5a24865e3",
        "8daf63298bcdca1c26fceb4f0aa51a1acbde76f5da9603b64410a0c186b6a422",
        "67b8839431d6a2e371a66b8e4c31215d2efd4f66348495a27eaffcc1bd969ca2",
        "d3c072c636d42b95903995f449ac45c0616f809cf096d8cf1353ebbc0e1baa44",
        "31f29ddd195c0a245059ad6e352380f759b9e436ed42b192e9f4dd4230dffa00",
        "80586472af7bbf1ea9617b78",
    ));
    stream.apply(0, &mut buffer);
    let plaintext: Vec<u8> = (0..300_u32)
        .map(|index| u8::try_from((index * 7 + 3) % 251).expect("below 251"))
        .collect();
    assert_eq!(buffer, plaintext, "the account file did not decrypt");
}

/// Acceptance criterion 7, over the real component: the session identifier, the master key,
/// the file's node key and the file key appear in no request, no address and no failure.
#[tokio::test]
async fn neither_session_nor_master_key_reaches_a_request_an_address_or_the_guest() {
    let host = MockMega::new();
    let answer = transform(host.clone())
        .resolve(&request(&account_file(FILE)))
        .await
        .expect("the call finished")
        .expect("a description");

    let node_key = b64(FILE_KEY);
    let canaries = [
        SESSION_ID.to_owned(),
        hex(&MASTER_KEY),
        crypto::b64_encode(&MASTER_KEY),
        String::from_utf8_lossy(&MASTER_KEY).into_owned(),
        hex(&node_key),
        FILE_KEY.to_owned(),
        hex(answer.key.expose()),
        crypto::b64_encode(answer.key.expose()),
    ];
    let surfaces: Vec<String> = host
        .requests()
        .into_iter()
        .chain([
            answer.download.url.to_string(),
            account_file(FILE),
            format!("{:?}", answer.transform),
        ])
        .collect();
    for surface in &surfaces {
        for canary in &canaries {
            assert!(
                !surface.contains(canary.as_str()),
                "{canary:?} surfaced in {surface}"
            );
        }
    }
    // Every account call carried the session, and only as the host's marker.
    for line in host.requests() {
        assert!(line.contains(SESSION_MARKER), "{line}");
    }

    // The host was asked exactly once, over the session and never the password, with one AES
    // step over the wrapped node key -- and what went back into the sandbox was the node key,
    // never the master key it was unwrapped under.
    let derivations = host.derivations();
    assert_eq!(derivations.len(), 1, "{derivations:?}");
    let (reference, steps, answered) = &derivations[0];
    assert_eq!(reference, "mega_session");
    assert!(matches!(
        steps.as_slice(),
        [DerivationStep::Aes128EcbDecrypt(wrapped)] if wrapped.len() == 32
    ));
    assert_eq!(answered, &node_key);
    assert!(!answered.windows(16).any(|window| window == MASTER_KEY));
}

#[tokio::test]
async fn an_account_folder_is_listed_with_names_sizes_and_keyless_addresses() {
    let host = MockMega::new();
    let mut links = crawler_with(host.clone(), &[])
        .crawl(
            &format!("https://mega.nz/fm/{FOLDER}"),
            Some(AccountId::new()),
        )
        .await
        .expect("the call finished")
        .expect("a listing");
    links.sort_by(|left, right| left.url.cmp(&right.url));
    let summary: Vec<LinkSummary> = links
        .into_iter()
        .map(|link| (link.url, link.file_name, link.size, link.package_hint))
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                account_file(CHILD),
                Some("SharedFile.jpg".to_owned()),
                Some(523_265),
                Some("SharedFolder".to_owned()),
            ),
            (
                account_file(FILE),
                Some("10MB.bin".to_owned()),
                Some(10_000_000),
                Some("SharedFolder".to_owned()),
            ),
        ]
    );
    // One listing, one host unwrap per node under the folder, all over the session.
    assert_eq!(host.requests().len(), 1);
    assert_eq!(host.derivations().len(), 3);
}

/// The file the crawler handed on resolves through the other plugin: the two halves meet.
#[tokio::test]
async fn what_the_crawler_hands_on_the_stream_plugin_resolves() {
    let host = MockMega::new();
    let links = crawler_with(host.clone(), &[])
        .crawl(
            &format!("https://mega.nz/fm/{FOLDER}"),
            Some(AccountId::new()),
        )
        .await
        .expect("the call finished")
        .expect("a listing");
    let child = links
        .iter()
        .find(|link| link.url.ends_with(CHILD))
        .expect("the child");
    let answer = transform(host)
        .resolve(&request(&child.url))
        .await
        .expect("the call finished")
        .expect("a description");
    assert_eq!(answer.download.file_name.as_deref(), Some("SharedFile.jpg"));
    assert_eq!(
        answer.download.size.map(rd_core::ByteCount::get),
        Some(523_265)
    );
}

#[tokio::test]
async fn an_account_without_a_session_is_refused_with_the_hosts_own_code() {
    let host = MockMega::signed_out();
    let failure = transform(host)
        .resolve(&request(&account_file(FILE)))
        .await
        .expect("the call finished")
        .expect_err("no session, no key");
    assert_eq!(
        failure.code.as_deref(),
        Some("plugin.provider_secret_missing"),
        "{failure:?}"
    );
}

#[tokio::test]
async fn a_plugin_that_does_not_declare_the_grant_never_gets_the_interface() {
    let error = transform_with(
        MockMega::new(),
        &[("key_derivation = true", "key_derivation = false")],
    )
    .err()
    .expect("a component importing key-derivation without the grant must not instantiate");
    assert!(
        format!("{error:#}").contains("key-derivation"),
        "the refusal does not name the interface: {error:#}"
    );
}

/// ADR 0020 point 7, over the session slot: a crawler that could reach one host the session
/// may not be sent to is refused before the host computes anything for it.
#[tokio::test]
async fn a_reach_beyond_where_the_session_may_go_is_refused() {
    let host = MockMega::new();
    let refusal = crawler_with(
        host.clone(),
        &[(
            r#"domains = ["g.api.mega.co.nz", "mega.nz"]"#,
            r#"domains = ["g.api.mega.co.nz", "mega.nz", "example.test"]"#,
        )],
    )
    .crawl(
        &format!("https://mega.nz/fm/{FOLDER}"),
        Some(AccountId::new()),
    )
    .await
    .expect("the call finished")
    .expect_err("a wider reach must be refused");
    assert_eq!(
        refusal.code.as_deref(),
        Some("plugin.key_derivation_reach_too_wide"),
        "{refusal:?}"
    );
    assert!(host.derivations().is_empty(), "a derivation ran anyway");
}
