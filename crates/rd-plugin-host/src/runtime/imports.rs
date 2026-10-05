//! Which WIT interfaces a component may import: the base every plugin gets, the interface its
//! type owns, and one per declared capability. Everything else -- WASI included -- is refused
//! when the component is compiled.
//!
//! Split out of `runtime.rs` (PLUG-21).

/// Interfaces every plugin may import, whatever its manifest says. Neither reaches the
/// network, a credential value or the user.
const BASE_IMPORTS: [&str; 2] = ["rdownloader:plugin/host", "rdownloader:plugin/types"];

/// The interfaces one manifest permits: the always-available base, the interface its plugin
/// type owns, and one entry per declared capability. WASI is in none of them, so it stays
/// denied by construction.
pub(super) fn allowed_imports(manifest: &crate::PluginManifest) -> Vec<&'static str> {
    let mut allowed = BASE_IMPORTS.to_vec();
    // The destination of a transfer is not a capability a plugin asks for; it is what a
    // transfer backend *is*. A resolver importing it would be reaching for a file to write.
    if manifest.plugin_type == crate::PluginType::Transfer {
        allowed.push("rdownloader:plugin/sink");
    }
    // Reading the files of a package is likewise not a grant but a definition: it is what a
    // post-processing step and a storage destination do. Neither can name a file — they are
    // handed a package-scoped handle — so the import is safe to give unconditionally to
    // exactly these two types and to nobody else.
    if matches!(
        manifest.plugin_type,
        crate::PluginType::Postprocess | crate::PluginType::Storage
    ) {
        allowed.push("rdownloader:plugin/source");
    }
    // Writing back what a flow produced is what an authentication plugin is; no other type
    // may even name the interface, and the plugin still names no reference of its own.
    if matches!(
        manifest.plugin_type,
        crate::PluginType::Auth | crate::PluginType::OAuth
    ) {
        allowed.push("rdownloader:plugin/credentials");
    }
    // What the host knows about the job being submitted (2026-09-27). Reaches nothing and
    // reads one label the host chose, so it is what a remote-job plugin is, not a grant.
    if manifest.plugin_type == crate::PluginType::RemoteJob {
        allowed.push("rdownloader:plugin/job-context");
    }
    // The settings of the target a notification is delivered to (RD-170-09): values the host
    // already checked against the manifest, reaching nothing.
    if manifest.plugin_type == crate::PluginType::Notifier {
        allowed.push("rdownloader:plugin/destination-settings");
    }
    let capabilities = &manifest.capabilities;
    if capabilities.net_http.is_some() {
        allowed.push("rdownloader:plugin/http");
    }
    if capabilities.cookies {
        allowed.push("rdownloader:plugin/cookies");
    }
    if capabilities.captcha {
        allowed.push("rdownloader:plugin/captcha");
    }
    if capabilities.net_stream.is_some() {
        allowed.push("rdownloader:plugin/net");
    }
    // Computing over a credential the guest never sees (RD-120-20). A grant like any other:
    // a component that imports the interface without declaring it is refused here, at
    // install and packaging time, rather than mid sign-in.
    if capabilities.key_derivation {
        allowed.push("rdownloader:plugin/key-derivation");
    }
    allowed
}

pub(super) fn import_matches(name: &str, allowed: &str) -> bool {
    name == allowed
        || name
            .strip_prefix(allowed)
            .is_some_and(|rest| rest.starts_with('@'))
}
