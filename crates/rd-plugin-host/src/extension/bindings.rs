//! Generated bindings for the ten extension worlds.
//!
//! They live together because what they have in common is the part that must not diverge:
//! every world reuses the resolver world's generated types, so a `failure` or a
//! `client-identity` is one type across all of them rather than seven identical ones the
//! compiler considers unrelated.

macro_rules! extension_world {
    ($module:ident, $world:literal) => {
        pub mod $module {
            wasmtime::component::bindgen!({
                path: "../rd-plugin-api/wit",
                world: $world,
                imports: { default: async },
                exports: { default: async },
                with: {
                    "rdownloader:plugin/types@0.9.0": crate::component::rdownloader::plugin::types,
                    "rdownloader:plugin/host@0.9.0": crate::component::rdownloader::plugin::host,
                    "rdownloader:plugin/http@0.9.0": crate::component::rdownloader::plugin::http,
                },
            });
        }
    };
}

extension_world!(intake, "intake-plugin");
extension_world!(auth, "auth-plugin");
extension_world!(oauth, "oauth-plugin");

/// The crawler world imports two interfaces the others do not, so it cannot go through the
/// macro: `cookies` and `captcha` must map onto the resolver world's generated types, or the
/// host traits already implemented for the store would not satisfy them.
pub mod crawler {
    wasmtime::component::bindgen!({
        path: "../rd-plugin-api/wit",
        world: "crawler-plugin",
        imports: { default: async },
        exports: { default: async },
        with: {
            "rdownloader:plugin/types@0.9.0": crate::component::rdownloader::plugin::types,
            "rdownloader:plugin/host@0.9.0": crate::component::rdownloader::plugin::host,
            "rdownloader:plugin/http@0.9.0": crate::component::rdownloader::plugin::http,
            "rdownloader:plugin/cookies@0.9.0": crate::component::rdownloader::plugin::cookies,
            "rdownloader:plugin/captcha@0.9.0": crate::component::rdownloader::plugin::captcha,
        },
    });
}
extension_world!(remote_job, "remote-job-plugin");
extension_world!(enricher, "enricher-plugin");
extension_world!(notifier, "notifier-plugin");
extension_world!(stream_transform, "stream-transform-plugin");

pub mod postprocess {
    wasmtime::component::bindgen!({
        path: "../rd-plugin-api/wit",
        world: "postprocess-plugin",
        imports: { default: async },
        exports: { default: async },
        with: {
            "rdownloader:plugin/types@0.9.0": crate::component::rdownloader::plugin::types,
            "rdownloader:plugin/host@0.9.0": crate::component::rdownloader::plugin::host,
        },
    });
}

pub mod storage {
    wasmtime::component::bindgen!({
        path: "../rd-plugin-api/wit",
        world: "storage-plugin",
        imports: { default: async },
        exports: { default: async },
        with: {
            "rdownloader:plugin/types@0.9.0": crate::component::rdownloader::plugin::types,
            "rdownloader:plugin/host@0.9.0": crate::component::rdownloader::plugin::host,
            "rdownloader:plugin/http@0.9.0": crate::component::rdownloader::plugin::http,
            "rdownloader:plugin/net@0.9.0": crate::transfer::rdownloader::plugin::net,
            "rdownloader:plugin/source@0.9.0": crate::extension::bindings::postprocess::rdownloader::plugin::source,
        },
    });
}
