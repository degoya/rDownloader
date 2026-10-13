//! The component: verify every `.sha256` sidecar the package carries.

checksum_postprocess_common::checksum_plugin!(crate::ALGORITHM, crate::Sha256Checksum);
