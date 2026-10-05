//! The component: verify every `.sha256` sidecar the package carries.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

checksum_postprocess_common::checksum_plugin!(crate::ALGORITHM, crate::Sha256Checksum);
