//! The component: verify every `.md5` sidecar the package carries.

checksum_postprocess_common::checksum_plugin!(crate::ALGORITHM, crate::Md5Checksum);
