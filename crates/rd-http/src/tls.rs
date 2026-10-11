//! The one place TLS trust is decided for the service's own socket clients.
//!
//! The HTTP client pool augments the platform trust store with the operator's custom CA
//! rather than replacing it; anything else that opens a TLS socket has to make the same
//! decision, or "custom CA" would mean something different depending on which protocol
//! happened to be carrying the bytes. FTPS and the plugin transfer host both build their
//! connector from here.

use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::{
    ClientConfig,
    client::WantsClientCert,
    pki_types::{CertificateDer, pem::PemObject},
};
use rustls_platform_verifier::{ConfigVerifierExt, Verifier};

/// Builds a client configuration trusting the platform store plus `custom_ca_pem`.
///
/// `custom_ca_pem` holds the PEM bundles the settings layer already parsed, in the same
/// `Vec<Vec<u8>>` shape [`crate::NetworkDefaults`] carries.
pub fn client_config(custom_ca_pem: &[Vec<u8>]) -> Result<ClientConfig> {
    if custom_ca_pem.is_empty() {
        return ClientConfig::with_platform_verifier()
            .context("platform certificate verifier unavailable");
    }
    // PEM parsing comes from `rustls-pki-types` rather than `rustls-pemfile`, which is
    // unmaintained and is itself only a thin wrapper around this (RUSTSEC-2025-0134).
    let mut extra_roots: Vec<CertificateDer<'static>> = Vec::new();
    for pem in custom_ca_pem {
        for certificate in CertificateDer::pem_slice_iter(pem) {
            extra_roots.push(certificate.context("custom CA bundle is not valid PEM")?);
        }
    }
    // A bundle that parses to nothing is a misconfiguration. Continuing with the platform
    // roots alone would look like it worked while trusting a different set of issuers than
    // the operator asked for.
    anyhow::ensure!(
        !extra_roots.is_empty(),
        "custom CA bundle contains no certificate"
    );
    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::aws_lc_rs::default_provider()));
    let verifier = Verifier::new_with_extra_roots(extra_roots, provider.clone())
        .context("build certificate verifier with the custom CA")?;
    let builder: rustls::ConfigBuilder<ClientConfig, WantsClientCert> =
        ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .context("no usable TLS protocol version")?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier));
    Ok(builder.with_no_client_auth())
}

/// The trust a download tool (yt-dlp, gallery-dl, streamlink) is handed as one PEM bundle: the
/// platform roots plus `custom_ca_pem` (RD-1240-08).
///
/// Those tools read a CA *file* (`SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE`, gallery-dl's `verify`)
/// and replace their own roots with it, so a file holding the custom CA alone would make every
/// public site fail. `None` without a custom CA, and when the platform roots cannot be read:
/// the tool then keeps its own roots, which is what it did before.
pub fn tool_trust_bundle(custom_ca_pem: &[Vec<u8>]) -> Result<Option<String>> {
    use base64::Engine as _;

    if custom_ca_pem.is_empty() {
        return Ok(None);
    }
    let mut custom = Vec::new();
    for pem in custom_ca_pem {
        for certificate in CertificateDer::pem_slice_iter(pem) {
            custom.push(certificate.context("custom CA bundle is not valid PEM")?);
        }
    }
    anyhow::ensure!(
        !custom.is_empty(),
        "custom CA bundle contains no certificate"
    );
    let platform = rustls_native_certs::load_native_certs();
    if platform.certs.is_empty() {
        tracing::warn!(
            errors = platform.errors.len(),
            "the platform roots could not be read; download tools keep their own and do not \
             trust the custom CA"
        );
        return Ok(None);
    }
    let mut bundle = String::new();
    for certificate in platform.certs.iter().chain(&custom) {
        let encoded = base64::engine::general_purpose::STANDARD.encode(certificate.as_ref());
        bundle.push_str("-----BEGIN CERTIFICATE-----\n");
        for line in encoded.as_bytes().chunks(64) {
            bundle.push_str(&String::from_utf8_lossy(line));
            bundle.push('\n');
        }
        bundle.push_str("-----END CERTIFICATE-----\n");
    }
    Ok(Some(bundle))
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_unusable_custom_ca_is_refused_instead_of_silently_ignored() {
        // Falling back to the platform roots here would turn a misconfigured pin into a
        // connection that looks fine but trusts a different set of issuers.
        assert!(super::client_config(&[b"not a certificate".to_vec()]).is_err());
        assert!(super::client_config(&[Vec::new()]).is_err());
    }

    #[test]
    fn the_default_configuration_uses_the_platform_store() {
        assert!(super::client_config(&[]).is_ok());
    }

    /// RD-1240-08: no custom CA, no bundle — the tools keep their own roots.
    #[test]
    fn a_tool_gets_no_bundle_without_a_custom_ca() {
        assert_eq!(super::tool_trust_bundle(&[]).expect("bundle"), None);
        assert!(super::tool_trust_bundle(&[b"not a certificate".to_vec()]).is_err());
    }

    /// The bundle carries the custom CA beside the platform roots, never instead of them.
    #[test]
    fn a_tool_bundle_holds_the_custom_ca_beside_the_platform_roots() {
        let key = rcgen::KeyPair::generate().expect("key");
        let mut params =
            rcgen::CertificateParams::new(vec!["ca.example".to_owned()]).expect("params");
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let pem = params.self_signed(&key).expect("certificate").pem();
        let Some(bundle) = super::tool_trust_bundle(&[pem.as_bytes().to_vec()]).expect("bundle")
        else {
            // A machine without readable platform roots keeps the tools' own trust.
            return;
        };
        let certificates = bundle.matches("-----BEGIN CERTIFICATE-----").count();
        assert!(certificates > 1, "{certificates}");
        let body: String = pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        assert!(bundle.replace('\n', "").contains(&body));
    }
}
