//! TLS configuration for FTPS.
//!
//! The trust decision itself lives in `rd_http::tls_client_config`, so FTPS, ordinary HTTP
//! and the plugin transfer host all augment the platform store with the operator's custom CA
//! in exactly the same way. Only the connector type is specific to suppaftp.

use std::sync::Arc;

use anyhow::Result;
use suppaftp::tokio::AsyncRustlsConnector;

/// Builds the connector used for both explicit `AUTH TLS` and implicit FTPS.
///
/// `custom_ca_pem` holds the PEM bundles already parsed by the settings layer, in the same
/// `Vec<Vec<u8>>` shape `rd_http::NetworkDefaults` carries.
pub fn connector(custom_ca_pem: &[Vec<u8>]) -> Result<AsyncRustlsConnector> {
    let config = rd_http::tls_client_config(custom_ca_pem)?;
    Ok(AsyncRustlsConnector::from(
        tokio_rustls::TlsConnector::from(Arc::new(config)),
    ))
}
