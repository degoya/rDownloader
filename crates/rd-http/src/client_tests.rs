use super::{ClientPurpose, client_builder, with_platform_trust};

#[test]
fn both_purposes_build_a_client() {
    for purpose in [ClientPurpose::Service, ClientPurpose::Direct] {
        assert!(client_builder(purpose).build().is_ok(), "{purpose:?}");
    }
}

/// RD-1490-13: without the platform store a client still builds, with a custom CA and without.
#[test]
fn a_client_builds_without_the_platform_store() {
    assert!(
        with_platform_trust(reqwest::Client::builder(), false)
            .build()
            .is_ok()
    );
    let key = rcgen::KeyPair::generate().expect("key");
    let mut params = rcgen::CertificateParams::new(vec!["ca.example".to_owned()]).expect("params");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let pem = params.self_signed(&key).expect("certificate").pem();
    let custom = reqwest::Certificate::from_pem(pem.as_bytes()).expect("custom CA");
    assert!(
        with_platform_trust(reqwest::Client::builder(), false)
            .add_root_certificate(custom)
            .build()
            .is_ok()
    );
}

/// Marks the child process of [`a_machine_without_ca_certificates_gets_clients_not_a_panic`].
const CHILD: &str = "RD_HTTP_EMPTY_STORE_CHILD";

/// The container of the live check of 2026-11-26, reproduced: `SSL_CERT_FILE` and
/// `SSL_CERT_DIR` point at an empty file and an empty folder, so the platform store holds no
/// certificate. Run by [`a_machine_without_ca_certificates_gets_clients_not_a_panic`] in a child
/// process of its own, because the store is read once per process.
#[test]
#[ignore = "run in a child process with an empty certificate store"]
fn child_without_ca_certificates() {
    // Only as the parent's child: a run of every ignored test has a store.
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    assert!(
        reqwest::Client::builder().build().is_err(),
        "the environment must reproduce the empty store"
    );
    assert!(crate::platform_trust_problem().is_some());
    for purpose in [ClientPurpose::Service, ClientPurpose::Direct] {
        assert!(client_builder(purpose).build().is_ok(), "{purpose:?}");
    }
    assert!(crate::tls_client_config(&[]).is_ok());
}

#[cfg(target_os = "linux")]
#[test]
fn a_machine_without_ca_certificates_gets_clients_not_a_panic() {
    let directory = tempfile::tempdir().expect("tempdir");
    let file = directory.path().join("empty.pem");
    std::fs::write(&file, b"").expect("empty bundle");
    let certificates = directory.path().join("certs");
    std::fs::create_dir(&certificates).expect("empty folder");
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "client::tests::child_without_ca_certificates",
            "--exact",
            "--ignored",
        ])
        .env(CHILD, "1")
        .env("SSL_CERT_FILE", &file)
        .env("SSL_CERT_DIR", &certificates)
        .output()
        .expect("run the child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "{stdout}\n{stderr}"
    );
}
