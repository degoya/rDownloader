//! The component: verify every `.md5` sidecar the package carries.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

checksum_postprocess_common::checksum_plugin!(crate::ALGORITHM, crate::Md5Checksum);
