//! The settings fields an `api:config` caller may not change: they decide who may talk to the
//! service, what it runs and where its data goes. Split from `settings_handlers.rs` into named
//! groups (RD-1280-08); the order of the groups is the order the fields are checked in.

use crate::dto::SettingsResponse;

/// The first privileged field `next` changes, if any, in the order of the groups below.
///
/// The settings blob is priced `api:config`, but a handful of its fields do not configure
/// downloading at all — they decide who may talk to the service and what it executes.
/// `admin_login_disabled` is the sharpest: setting it makes `granted_scopes` hand every
/// caller, authenticated or not, the full `Scope::API`, so an `api:config` token could mint
/// itself `api:secrets` and `api:admin`. The executable paths and the completion script are
/// the same problem one step removed: they name a program this service runs. The outbound
/// endpoints that receive what the service holds -- the trace export, the DLC decryption
/// service -- are the third kind: pointing one elsewhere hands somebody else the data.
pub(super) fn privileged_change(
    current: &SettingsResponse,
    next: &SettingsResponse,
) -> Option<&'static str> {
    access_fields(current, next)
        .into_iter()
        .chain(program_fields(current, next))
        .chain(outbound_fields(current, next))
        .chain(self_management_fields(current, next))
        .find_map(|(name, changed)| changed.then_some(name))
}

/// Who may talk to the service, from where, and for how long a session lasts.
fn access_fields(current: &SettingsResponse, next: &SettingsResponse) -> [(&'static str, bool); 7] {
    [
        (
            "admin_login_disabled",
            current.admin_login_disabled != next.admin_login_disabled,
        ),
        (
            "trusted_proxies",
            current.trusted_proxies != next.trusted_proxies,
        ),
        ("external_url", current.external_url != next.external_url),
        // Which names a browser may reach the service by: the rebinding guard (security
        // review 2026-09-28, finding 3), so as privileged as the proxy contract it sits in.
        ("allowed_hosts", current.allowed_hosts != next.allowed_hosts),
        (
            "cookie_security",
            current.cookie_security != next.cookie_security,
        ),
        // How long a stolen cookie stays worth something: lengthening either limit widens
        // what a credential reaches in time the way the fields above widen it in scope
        // (RD-130-09).
        (
            "session_idle_hours",
            current.session_idle_hours != next.session_idle_hours,
        ),
        (
            "session_max_hours",
            current.session_max_hours != next.session_max_hours,
        ),
    ]
}

/// Programs and scripts the service runs, and the trust it extends to hosts and certificates.
fn program_fields(
    current: &SettingsResponse,
    next: &SettingsResponse,
) -> [(&'static str, bool); 8] {
    [
        (
            "completion_action",
            current.completion_action != next.completion_action,
        ),
        (
            "completion_script",
            current.completion_script != next.completion_script,
        ),
        (
            "scripts_directory",
            current.scripts_directory != next.scripts_directory,
        ),
        ("custom_ca_pem", current.custom_ca_pem != next.custom_ca_pem),
        (
            "vendor_directory",
            current.vendor_directory != next.vendor_directory,
        ),
        (
            "rar_executable",
            current.rar_executable != next.rar_executable,
        ),
        // The upload tool is a program the service runs like the unpacker, and on Windows the
        // only one of the four without a fixed name to fall back to (security audit
        // 2026-09-30, finding 3).
        (
            "rclone_executable",
            current.rclone_executable != next.rclone_executable,
        ),
        (
            "remote_ssh_auto_trust",
            current.remote_ssh_auto_trust != next.remote_ssh_auto_trust,
        ),
    ]
}

/// Recipients of what the service holds, the programs and files around them, and the addresses it
/// fetches from on its own.
fn outbound_fields(
    current: &SettingsResponse,
    next: &SettingsResponse,
) -> [(&'static str, bool); 11] {
    [
        // Pointing the trace export somewhere is telling the service to post to a URL of
        // somebody's choosing on a timer. That is the same family of decision as naming a
        // program it runs, whatever the payload is (RD-110-03).
        (
            "otlp_endpoint",
            current.otlp_endpoint != next.otlp_endpoint
                || current.otlp_enabled != next.otlp_enabled,
        ),
        // Every imported DLC's key blob goes to this address, and whatever it answers is queued
        // as the container's links: a service of somebody's choosing would read the person's
        // imports and choose what they download.
        (
            "dlc_service_endpoint",
            current.dlc_service_endpoint != next.dlc_service_endpoint,
        ),
        // Every byte of every finished package is streamed there once the malware scan is on
        // (RD-190-14): the same kind of recipient as the DLC service.
        ("clamd_address", current.clamd_address != next.clamd_address),
        (
            "media_ytdlp_executable",
            current.media_ytdlp_executable != next.media_ytdlp_executable
                || current.media_ffmpeg_executable != next.media_ffmpeg_executable
                || current.gallery_executable != next.gallery_executable
                || current.record_streamlink_executable != next.record_streamlink_executable,
        ),
        // The reconnect runs a script and then asks addresses of somebody's choosing what the
        // public address is (audit 2026-10-05, S8): a program and a recipient at once.
        (
            "reconnect_enabled",
            current.reconnect_enabled != next.reconnect_enabled,
        ),
        (
            "reconnect_script",
            current.reconnect_script != next.reconnect_script,
        ),
        (
            "reconnect_ip_check_urls",
            current.reconnect_ip_check_urls != next.reconnect_ip_check_urls,
        ),
        // Files the service reads wherever they are (audit 2026-10-05, S9): every line of the
        // password list becomes an argument of the unpacker, which the process list shows.
        (
            "passwords_file",
            current.passwords_file != next.passwords_file,
        ),
        (
            "excluded_domains_file",
            current.excluded_domains_file != next.excluded_domains_file,
        ),
        // Addresses the service fetches from on its own (audit 2026-10-08, API-01), the same
        // family as the reconnect's address check. The tool manifest is signed and its
        // sequence only rises, so a foreign host cannot swap the tools; it still learns this
        // installation exists and decides whether it ever sees an update. The blocklist is
        // unsigned: whoever serves it chooses which peers the torrent engine refuses.
        (
            "managed_tools_manifest_url",
            current.managed_tools_manifest_url != next.managed_tools_manifest_url,
        ),
        (
            "torrent_ip_blocklist_url",
            current.torrent_ip_blocklist_url != next.torrent_ip_blocklist_url,
        ),
    ]
}

/// What the service does to itself -- installing, restarting, keeping and removing copies -- and
/// what an MCP client may make it run.
fn self_management_fields(
    current: &SettingsResponse,
    next: &SettingsResponse,
) -> [(&'static str, bool); 5] {
    [
        // Installing a new version of the program by itself (RD-1240-27): what the service runs
        // next, the same decision the install route keeps to the interface.
        (
            "update_auto_install",
            current.update_auto_install != next.update_auto_install
                || current.update_auto_install_window != next.update_auto_install_window,
        ),
        // Restarting the service by itself when a restart is pending (RD-1240-32): when the
        // service stops and starts, as privileged as the automatic install beside it.
        (
            "restart_when_needed",
            current.restart_when_needed != next.restart_when_needed,
        ),
        // How long the copies a rollback reaches for stay (RD-1240-34): as much the
        // administrator's as removing them, which `POST /api/v1/system/cleanup` keeps to them.
        (
            "update_backup_retention_days",
            current.update_backup_retention_days != next.update_backup_retention_days,
        ),
        // The same for the subscription archive's full rows (RD-1240-35): the same clean-up
        // compacts them.
        (
            "subscription_item_retention_days",
            current.subscription_item_retention_days != next.subscription_item_retention_days,
        ),
        // Whether MCP tools may name a script (RD-1190-21): what an agent may make this service
        // run, so as privileged as the script fields it opens. MCP refuses it outright.
        (
            "mcp_scripts_allowed",
            current.mcp_scripts_allowed != next.mcp_scripts_allowed,
        ),
    ]
}
