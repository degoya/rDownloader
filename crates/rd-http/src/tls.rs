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
}
