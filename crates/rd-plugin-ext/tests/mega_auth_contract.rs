//! A MEGA account sign-in, through the real component, with the password on the host side of
//! the boundary the whole time (RD-120-20, closing RD-120-11's last open box).
//!
//! **The provider is a mock and the account is invented.** Nothing here opens a socket and
//! `mega.nz` is not contacted. The three encrypted blocks below were generated on 2026-09-22
//! for this file alone, from the throwaway RSA-2048 key `plugins/mega-login-probe` already
//! carries and from a password written down five lines under this one. No credential of any
//! person, and no real MEGA account, is in this tree.
//!
//! What the tests establish, in order:
//!
//! 1. the sign-in completes and stores a session the account can be used with;
//! 2. **the password appears in nothing the guest touched** -- not in a request body, not in
//!    the stored value, not in what any derivation answered;
//! 3. a plugin that does not declare `key_derivation` never gets the interface;
//! 4. the derivation is charged against the caller's fuel budget, in that a manifest whose
//!    budget is a hair under the measured price cannot complete a sign-in that the same
//!    manifest with a hair over completes.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rd_core::{AccountId, Failure};
use rd_plugin_api::{
    ClientIdentity, DerivationStep, HostHttpRequest, HostHttpResponse, ResolverHost,
};
use rd_plugin_host::{
    PluginManifest,
    extension::{AuthProgress, AuthProvider},
    keyderive,
};

const MANIFEST: &str = include_str!("../../../plugins/mega-auth/manifest.toml");

/// The invented account's password. Not anybody's, and the point of the whole file is that
/// no part of the component ever sees it.
const PASSWORD: &str = "not-a-real-mega-password";
/// The salt `us0` answers with, base64 in MEGA's alphabet.
const SALT: &str = "bWVnYS1jb250cmFjdC1zYWx0LTAwMDAwMDAwMDAwMDA";
/// The master key wrapped under the first half of the derivation.
const WRAPPED_MASTER_KEY: &str = "bJQSgj05bkJzK02b3NwAGA";
/// The `uh` MEGA expects for this password and salt: the derivation's second half.
const EXPECTED_USER_HASH: &str = "0u6zFTd1ZN-9PCqGvP98ZA";
/// The master key itself, which is what the stored session has to carry.
const MASTER_KEY: &str = "DExE4Sjq7npAvL1P_sGWFw";
/// The session identifier the encrypted `csid` below stands for.
const EXPECTED_SESSION_ID: &str = "UkRURVNUU0VTU0lPTklERU5USUZJRVIwMTIzNDU2Nzg5YWJjZGVmZ2hpag";
/// The RSA private key block, wrapped under the master key, as `us` sends it.
const WRAPPED_PRIVATE_KEY: &str = "yUHeYKaxA5TuW05L0tSTYNjvDoJSrzuOWtM4rLphfT6U29ekiFlECKDF8sC5jEyyFdDjkbeIkQ9jiG_xDm8AmqjzjsTE6WSt_fDmYcGM0hQfmU2kNRQR1IYhbaHq2TaXomUSeQQe-LJGOKpBN-6FYoHH0x8n4pkFHOK6ajTBWh_orO6xYDiI8zD0aaTAOlveu1SodQF4pbNHzp8UHF4PJX24mD7AK2oiXQhcagAeDgR1aYtXmq8SmXGw7GoBj56Li7svnqUEBXS-26S1crDIcFehk0hWG1LmnLcLcQx4fst8x0p_qEwM6F_Idy0Sa0Gv_tQwyvirC5LcdHuET3XUnTf_z0u7o8IPeMUfI7i2GQdqRnxON_eaMg6SU7RuTL0-i8oUpv_Z5AJTGmn6RF2zk4v3deEH7mtCZ4cDHcGA43LOnku9bmM2n3NCgCv7IGKrzm8kNTIWoY4gtt-mbvV9JxdkXVmIwNcpvag2tfm_QWNFtz7qNRgx2TXbI-Nz5UeYIeD-I_6qneAQhnQJlNDt8jp9aWJHer_YYHROlfplIVefqHIV8tOb-ChUsgSqR62rjexFKh2JNIAyGO2LGo4QL5bbN939jEA-MouMFbW5m4ckoUM-zYoafANd2ydwLFQDPuRnmfDOeIS1kgaMzwdhdtYhN9VggpJI10YkVxZw5jXpmNDRrHBWgJwpC4XJS6xbXsNVD-HFMP_F9XklT7XTK_aS0zhi2FVsyHijEZXHSqdfOqr_EEaTxUQ3QxOfLWuLaJ_Cu7Ry3NNSwryYkmS-D8dkwIHh61XKxbRDOnRijnGoVt5otDgErK757R--noqdOvfot26HEVHKeg2f6iBhvVqY1n1C1KwPp_R_aDb0IeI";
/// The session identifier encrypted to the invented account's public key.
const ENCRYPTED_SESSION_ID: &str = "01AjImGG8A-K76tJAprnjIPn0nJSSce2I3Rw8KjuTVtTctgrmnoynW4JkU30G6WuNPrrmWTCbgbLvrZoC6TcO0AbbQXmutGazltH-P4GhZgJAbJceDFvTaIpGA7XhhIZS9_TZKjrBK6e2eoPFhYU52zuepeVcn_bR9Hg97sPXXV0Wza2m2-ifpuL0LOVLsFPSfgYNZMdXDhjqvzNsQE2XJLzOwBiHYXOKJ-Twc0wiiRNTowHJDwhMeJSmn6xMag5CoVtoU1DOJl9meDGPcNIuwTYXDphwxZ636LwftB5BT8uI_rqD_STH1xRzkwhkvIyTL6y_yBDQOnCpI1-YM9NgA";

/// The provider, at the host boundary. Also the vault: `derive_from_secret` is the one place
/// the password exists at all, and it is `rd_plugin_host::keyderive` that runs the chain.
struct MockMega {
    /// Every request, as `<method> <url> <body>`.
    requests: Mutex<Vec<String>>,
    /// What `store-token` was handed, if anything.
    stored: Mutex<Option<String>>,
    /// Every chain the guest asked for, and what it was answered with.
    derivations: Mutex<Vec<(Vec<DerivationStep>, Vec<u8>)>>,
}

impl MockMega {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            stored: Mutex::new(None),
            derivations: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }

    fn stored(&self) -> Option<String> {
        self.stored.lock().expect("stored").clone()
    }

    fn derivations(&self) -> Vec<(Vec<DerivationStep>, Vec<u8>)> {
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
        self.requests
            .lock()
            .expect("requests")
            .push(format!("{} {} {body}", request.method, request.url));
        let answer = |text: String| {
            Ok(HostHttpResponse {
                status: 200,
                final_url: request.url.clone(),
                headers: Vec::new(),
                body: text.into_bytes(),
            })
        };
        if body.contains(r#""a":"us0""#) {
            return answer(format!(r#"[{{"v":2,"s":"{SALT}"}}]"#));
        }
        if body.contains(r#""a":"us""#) {
            // The provider checks `uh`; a wrong one is `-9`, exactly as a wrong password is.
            if !body.contains(EXPECTED_USER_HASH) {
                return answer("[-9]".to_owned());
            }
            return answer(format!(
                r#"[{{"k":"{WRAPPED_MASTER_KEY}","privk":"{WRAPPED_PRIVATE_KEY}","csid":"{ENCRYPTED_SESSION_ID}"}}]"#
            ));
        }
        answer("[-9]".to_owned())
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        true
    }

    async fn store_token(&self, _account_id: AccountId, value: &str) -> Result<(), Failure> {
        *self.stored.lock().expect("stored") = Some(value.to_owned());
        Ok(())
    }

    /// The vault side. The credential exists here and nowhere else, and what goes back is
    /// whatever the shared engine made of it -- the same code the real host runs.
    async fn derive_from_secret(
        &self,
        _client: &ClientIdentity,
        reference: &str,
        steps: &[DerivationStep],
    ) -> Result<Vec<u8>, Failure> {
        assert_eq!(
            reference, "mega_password",
            "the plugin named another secret"
        );
        // The password is a typed credential: RD-120-20's rule, unchanged by RD-120-30.
        keyderive::validate(steps, keyderive::SecretOrigin::Person)?;
        let derived = keyderive::run(PASSWORD.as_bytes(), steps)?;
        self.derivations
            .lock()
            .expect("derivations")
            .push((steps.to_vec(), derived.clone()));
        Ok(derived)
    }
}

fn manifest_from(text: &str) -> PluginManifest {
    toml::from_str(text).expect("the manifest")
}

/// Registers every bundled provider row, the way startup does from the installed manifests.
///
/// Since RD-101-13 a provider exists exactly while its plugin is installed; nothing is
/// compiled in. A bare test process therefore knows no provider at all, and the host's reach
/// check -- may this plugin compute over a credential it could send everywhere it reaches? --
/// has nothing to check against. MEGA's own row arrived with RD-120-20, on
/// `plugins/mega/manifest.toml`.
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
    assert!(
        rd_provider_registry::by_slug("mega").is_some(),
        "MEGA has no provider row, so no account could be configured for it"
    );
}

fn provider(host: Arc<MockMega>) -> AuthProvider {
    register_bundled_providers();
    let bytes = rd_plugin_host::artifact::component("rd-plugin-mega-auth");
    AuthProvider::new(manifest_from(MANIFEST), &bytes, Some(host)).expect("compile the plugin")
}

/// The same component under a manifest a test narrowed. Only the limits differ, which is
/// exactly what a budget has to be tested against.
fn provider_with(host: Arc<MockMega>, from: &str, to: &str) -> Result<AuthProvider, String> {
    register_bundled_providers();
    assert!(
        MANIFEST.contains(from),
        "the manifest no longer says {from:?}"
    );
    let text = MANIFEST.replace(from, to);
    let bytes = rd_plugin_host::artifact::component("rd-plugin-mega-auth");
    AuthProvider::new(manifest_from(&text), &bytes, Some(host))
        .map_err(|error| format!("{error:#}"))
}

fn account() -> AccountId {
    AccountId::new()
}

#[tokio::test]
async fn a_mega_account_signs_in_and_the_session_carries_the_master_key() {
    let host = MockMega::new();
    let state = provider(Arc::clone(&host))
        .begin(account(), None)
        .await
        .expect("the sign-in runs");
    assert_eq!(state, AuthProgress::Authorized, "{state:?}");

    // Two requests, both to the command endpoint, in the order MEGA prescribes.
    let requests = host.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(requests[0].contains(r#""a":"us0""#), "{:?}", requests[0]);
    assert!(requests[1].contains(r#""a":"us""#), "{:?}", requests[1]);
    // The address is the host's marker, never a value the plugin held.
    assert!(requests[0].contains("{{username}}"), "{:?}", requests[0]);
    assert!(
        requests[1].contains(EXPECTED_USER_HASH),
        "the request did not carry the derived user hash: {:?}",
        requests[1]
    );

    let stored = host.stored().expect("a session was stored");
    assert!(
        stored.contains(EXPECTED_SESSION_ID),
        "the stored session has no identifier: {stored}"
    );
    assert!(
        stored.contains(MASTER_KEY),
        "the stored session has no master key, so no account file could be opened: {stored}"
    );
    // In the shape the host splits (RD-120-30): the identifier as the token a marker sends,
    // the master key as key material nothing but a derivation reads. Any other object is
    // refused at `store-token` rather than stored as a token and sent whole.
    let stored: serde_json::Value = serde_json::from_str(&stored).expect("a JSON object");
    assert_eq!(stored["token"], EXPECTED_SESSION_ID);
    assert_eq!(stored["key"], MASTER_KEY);
    assert_eq!(stored.as_object().map(serde_json::Map::len), Some(2));
}

#[tokio::test]
async fn the_password_reaches_nothing_the_guest_touched() {
    // The acceptance criterion, stated as a test. Three surfaces the plaintext could have
    // escaped through, and it is absent from all three.
    let host = MockMega::new();
    provider(Arc::clone(&host))
        .begin(account(), None)
        .await
        .expect("the sign-in runs");

    for request in host.requests() {
        assert!(
            !request.contains(PASSWORD),
            "a request carried the password: {request}"
        );
    }
    let stored = host.stored().expect("a session was stored");
    assert!(!stored.contains(PASSWORD), "the stored session carried it");
    for (steps, answer) in host.derivations() {
        assert!(
            !answer
                .windows(PASSWORD.len())
                .any(|window| window == PASSWORD.as_bytes()),
            "a derivation answered with the password itself: {steps:?}"
        );
    }
}

#[tokio::test]
async fn the_password_key_never_leaves_the_host() {
    // The reason the plugin makes three calls instead of one. The first sixteen derived
    // bytes unwrap the master key and are the one value that is a direct function of the
    // password; no chain this plugin asks for ends on them.
    let host = MockMega::new();
    provider(Arc::clone(&host))
        .begin(account(), None)
        .await
        .expect("the sign-in runs");

    let derivations = host.derivations();
    assert_eq!(derivations.len(), 3, "{derivations:?}");
    let whole = keyderive::run(
        PASSWORD.as_bytes(),
        &[DerivationStep::Pbkdf2HmacSha512 {
            salt: salt_bytes(),
            rounds: 100_000,
            length: 32,
        }],
    )
    .expect("the whole derivation");
    let password_key = &whole[..16];
    for (steps, answer) in &derivations {
        assert!(
            !answer
                .windows(password_key.len())
                .any(|window| window == password_key),
            "a derivation handed the password key over: {steps:?}"
        );
    }
}

#[tokio::test]
async fn a_plugin_that_does_not_declare_the_capability_never_gets_the_interface() {
    // The component is the shipped one; only the manifest's grant is taken away. It then
    // fails to instantiate, because the interface it imports is not linked.
    let text = provider_with(
        MockMega::new(),
        "key_derivation = true",
        "key_derivation = false",
    )
    .err()
    .expect("a plugin without the grant must not instantiate");
    assert!(
        text.contains("key-derivation"),
        "the refusal does not name the interface: {text}"
    );
}

#[tokio::test]
async fn the_derivation_is_charged_against_the_callers_fuel_budget() {
    // The price of one chain, at the measured guest rate: 100 000 rounds of
    // PBKDF2-HMAC-SHA512 over one hash block (RD-120-11).
    let one_chain = keyderive::fuel_cost(&[DerivationStep::Pbkdf2HmacSha512 {
        salt: salt_bytes(),
        rounds: 100_000,
        length: 32,
    }]);
    assert!(one_chain > 3_000_000_000, "the rate collapsed: {one_chain}");

    // A budget that holds less than the very first chain cannot buy it, and the sign-in
    // stops at the derivation rather than at a request.
    let host = MockMega::new();
    let budget = one_chain - 1;
    let provider = provider_with(
        Arc::clone(&host),
        "fuel = 12000000000",
        &format!("fuel = {budget}"),
    )
    .expect("the manifest is still valid");
    let error = provider
        .begin(account(), None)
        .await
        .expect_err("the sign-in cannot be paid for");
    assert!(host.derivations().is_empty(), "work was done anyway");
    // One request was made -- `us0` -- and none after it.
    assert_eq!(host.requests().len(), 1, "{:?}", host.requests());
    let text = format!("{error:?}");
    assert!(
        text.contains("fuel") || text.contains("key_derivation_budget"),
        "the refusal does not say what ran out: {text}"
    );

    // And the same manifest with room for all three chains completes.
    let host = MockMega::new();
    let state = provider_with(
        Arc::clone(&host),
        "fuel = 12000000000",
        "fuel = 12000000000",
    )
    .expect("the manifest is still valid")
    .begin(account(), None)
    .await
    .expect("the sign-in runs");
    assert_eq!(state, AuthProgress::Authorized, "{state:?}");
}

/// The salt the mock answers with, decoded the way the plugin decodes it.
fn salt_bytes() -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bits = 0_u32;
    let mut held = 0_u32;
    let mut out = Vec::new();
    for character in SALT.bytes() {
        let index = ALPHABET
            .iter()
            .position(|entry| *entry == character)
            .expect("the fixture is MEGA base64") as u32;
        held = (held << 6) | index;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((held >> bits) & 0xff) as u8);
        }
    }
    out
}

#[tokio::test]
async fn a_legacy_account_is_refused_by_name_rather_than_attempted() {
    // Version 1 derives its key with 65 536 AES rounds over the password instead, which the
    // contract does not carry. What matters is that the person is told which it is.
    struct Legacy;
    #[async_trait]
    impl ResolverHost for Legacy {
        async fn http_request(
            &self,
            _client: &ClientIdentity,
            request: HostHttpRequest,
        ) -> Result<HostHttpResponse, Failure> {
            Ok(HostHttpResponse {
                status: 200,
                final_url: request.url,
                headers: Vec::new(),
                body: br#"[{"v":1}]"#.to_vec(),
            })
        }
        async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
            true
        }
        async fn derive_from_secret(
            &self,
            _client: &ClientIdentity,
            _reference: &str,
            _steps: &[DerivationStep],
        ) -> Result<Vec<u8>, Failure> {
            panic!("a legacy account must be refused before anything is derived");
        }
    }
    register_bundled_providers();
    let bytes = rd_plugin_host::artifact::component("rd-plugin-mega-auth");
    let provider = AuthProvider::new(manifest_from(MANIFEST), &bytes, Some(Arc::new(Legacy)))
        .expect("compile the plugin");
    let error = provider
        .begin(account(), None)
        .await
        .expect_err("a legacy account is refused");
    assert!(
        format!("{error:?}").contains("account_version_unsupported"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_host_without_the_primitive_refuses_rather_than_inventing_an_answer() {
    struct NoVault;
    #[async_trait]
    impl ResolverHost for NoVault {
        async fn http_request(
            &self,
            _client: &ClientIdentity,
            request: HostHttpRequest,
        ) -> Result<HostHttpResponse, Failure> {
            Ok(HostHttpResponse {
                status: 200,
                final_url: request.url,
                headers: Vec::new(),
                body: format!(r#"[{{"v":2,"s":"{SALT}"}}]"#).into_bytes(),
            })
        }
        async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
            true
        }
    }
    register_bundled_providers();
    let bytes = rd_plugin_host::artifact::component("rd-plugin-mega-auth");
    let provider = AuthProvider::new(manifest_from(MANIFEST), &bytes, Some(Arc::new(NoVault)))
        .expect("compile the plugin");
    let error = provider
        .begin(account(), None)
        .await
        .expect_err("a host with no vault cannot derive");
    assert!(
        format!("{error:?}").contains("key_derivation_unsupported"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_plugin_that_reaches_further_than_the_credential_may_be_sent_is_refused() {
    // The rule that keeps the primitive from widening anything (ADR 0020, point 7). A
    // credential may be *sent* only to the domains its slot names; derived bytes are not
    // gated that way, so a plugin may compute over a credential exactly when everything it
    // can reach is somewhere that credential could itself have gone. The component is the
    // shipped one; only the declared reach is widened.
    let host = MockMega::new();
    let provider = provider_with(
        Arc::clone(&host),
        r#"domains = ["g.api.mega.co.nz"]"#,
        r#"domains = ["g.api.mega.co.nz", "mega.nz"]"#,
    )
    .expect("the manifest is still valid");
    let error = provider
        .begin(account(), None)
        .await
        .expect_err("a wider reach must be refused");
    assert!(
        format!("{error:?}").contains("key_derivation_reach_too_wide"),
        "{error:?}"
    );
    assert!(host.derivations().is_empty(), "a derivation ran anyway");
}
