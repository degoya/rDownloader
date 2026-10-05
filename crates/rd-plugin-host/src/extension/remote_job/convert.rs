//! Turning a remote-job guest's answers into the host's types, and refusing on the way what
//! the host cannot store or must not follow: an unnamed job, an unusable content key, a path
//! that would leave the job, a source that carries a marker.
//!
//! Split out of `remote_job.rs` (PLUG-21).

use rd_core::AccountId;

use super::{
    CacheAnswer, CacheKind, CacheState, MAX_JOB_ARTIFACTS, MAX_JOB_ENTRIES, RemoteJobArtifact,
    RemoteJobEntry, RemoteJobHandle, RemoteJobProgress, RemoteJobRefusal, RemoteJobSource,
    RemoteJobWork, bindings,
};

/// Longest content key the host will store. A key is a digest or an identifier; anything
/// longer is a payload wearing a key's name.
const MAX_CONTENT_KEY: usize = 256;

pub(super) fn is_usable_key(key: &str) -> bool {
    !key.trim().is_empty() && key.len() <= MAX_CONTENT_KEY && !key.contains(char::is_control)
}

/// Longest remote identifier the host will store, on the same reasoning.
const MAX_REMOTE_ID: usize = 256;

/// Accepts a handle only when the provider actually named the job.
///
/// The identifier is the whole point of the handle: without it there is nothing to poll,
/// nothing to choose against and nothing to delete, and a row carrying an empty one would be
/// a job the person can see and nobody can reach.
pub(super) fn handle_from(
    handle: bindings::exports::rdownloader::plugin::remote_job::RemoteHandle,
    account: AccountId,
) -> Result<RemoteJobHandle, RemoteJobRefusal> {
    let remote_id = handle.remote_id.trim().to_owned();
    if remote_id.is_empty() || remote_id.len() > MAX_REMOTE_ID {
        return Err(RemoteJobRefusal {
            code: Some("remote_job.missing_remote_id".to_owned()),
            message: "the provider did not name the job it created".to_owned(),
            category: rd_core::FailureKind::Permanent,
        });
    }
    Ok(RemoteJobHandle {
        remote_id,
        // The account is the host's own, never the guest's answer: a plugin that named
        // another account here would be asking for a credential it was not started for.
        account_id: account.to_string(),
        job_state: handle.job_state,
    })
}

pub(super) fn to_wit_source(
    source: &RemoteJobSource,
) -> bindings::exports::rdownloader::plugin::remote_job::JobSource {
    use bindings::exports::rdownloader::plugin::remote_job::JobSource as Wit;
    match source {
        RemoteJobSource::Magnet(address) => Wit::Magnet(address.clone()),
        RemoteJobSource::Container(bytes) => Wit::Container(bytes.clone()),
        RemoteJobSource::Address(address) => Wit::Address(address.clone()),
    }
}

pub(super) fn to_wit_handle(
    handle: &RemoteJobHandle,
) -> bindings::exports::rdownloader::plugin::remote_job::RemoteHandle {
    bindings::exports::rdownloader::plugin::remote_job::RemoteHandle {
        remote_id: handle.remote_id.clone(),
        account_id: handle.account_id.clone(),
        job_state: handle.job_state.clone(),
    }
}

pub(super) fn kind_from(
    kind: bindings::exports::rdownloader::plugin::remote_job::CacheKind,
) -> CacheKind {
    use bindings::exports::rdownloader::plugin::remote_job::CacheKind as Wit;
    match kind {
        Wit::Torrent => CacheKind::Torrent,
        Wit::Usenet => CacheKind::Usenet,
        Wit::Hoster => CacheKind::Hoster,
    }
}

pub(super) fn to_wit_kind(
    kind: CacheKind,
) -> bindings::exports::rdownloader::plugin::remote_job::CacheKind {
    use bindings::exports::rdownloader::plugin::remote_job::CacheKind as Wit;
    match kind {
        CacheKind::Torrent => Wit::Torrent,
        CacheKind::Usenet => Wit::Usenet,
        CacheKind::Hoster => Wit::Hoster,
    }
}

pub(super) fn answer_from(
    answer: bindings::exports::rdownloader::plugin::remote_job::CacheAnswer,
) -> CacheAnswer {
    use bindings::exports::rdownloader::plugin::remote_job::CacheState as Wit;
    CacheAnswer {
        state: match answer.state {
            Wit::Cached => CacheState::Cached,
            Wit::Known => CacheState::Known,
            Wit::Unknown => CacheState::Unknown,
        },
        file_name: answer
            .file_name
            .as_deref()
            .map(safe_name)
            .filter(|name| !name.is_empty()),
        size: answer.size,
    }
}

pub(super) fn progress_from(
    progress: bindings::exports::rdownloader::plugin::remote_job::RemoteProgress,
) -> RemoteJobProgress {
    use bindings::exports::rdownloader::plugin::remote_job::RemoteProgress as Wit;
    match progress {
        Wit::Preparing(seconds) => RemoteJobProgress::Preparing {
            retry_after_seconds: seconds,
        },
        Wit::AwaitingChoice(entries) => RemoteJobProgress::AwaitingChoice {
            entries: entries
                .into_iter()
                .take(MAX_JOB_ENTRIES)
                .map(|entry| RemoteJobEntry {
                    id: entry.id,
                    path: safe_path(&entry.path),
                    size: entry.size,
                    selected: entry.selected,
                })
                .collect(),
        },
        Wit::Working(work) => RemoteJobProgress::Working(RemoteJobWork {
            progress_permille: work.progress_permille.map(|value| value.min(1_000)),
            speed_bytes_per_second: work.speed_bytes_per_second,
            seconds_remaining: work.seconds_remaining,
        }),
        Wit::Ready(artifacts) => RemoteJobProgress::Ready {
            artifacts: artifacts
                .into_iter()
                .take(MAX_JOB_ARTIFACTS)
                .map(|artifact| RemoteJobArtifact {
                    url: artifact.url,
                    file_name: artifact.file_name.as_deref().map(safe_name),
                    size: artifact.size,
                    package_hint: artifact
                        .package_hint
                        .as_deref()
                        .map(safe_path)
                        .filter(|hint| !hint.is_empty()),
                })
                .collect(),
        },
        Wit::Failed(failure) => RemoteJobProgress::Failed(refusal(failure)),
    }
}

/// Whether an address source carries a marker (RD-120-66). A container is bytes, not an
/// address; the host does not expand markers inside an upload, which is where it goes.
pub(super) fn marked(source: &RemoteJobSource) -> bool {
    match source {
        RemoteJobSource::Magnet(address) | RemoteJobSource::Address(address) => {
            crate::foreign_address::carries_marker(address)
        }
        RemoteJobSource::Container(_) => false,
    }
}

pub(super) fn marked_refusal() -> RemoteJobRefusal {
    let failure = crate::foreign_address::refused();
    RemoteJobRefusal {
        code: failure.code,
        message: failure.message,
        category: failure.category,
    }
}

pub(super) fn refusal(
    failure: crate::component::rdownloader::plugin::types::Failure,
) -> RemoteJobRefusal {
    let failure = crate::component::from_wit_failure(failure);
    RemoteJobRefusal {
        code: failure.code,
        message: failure.message,
        category: failure.category,
    }
}

/// Reduces a path from a stranger's data structure to something that can only mean a place
/// inside this job.
///
/// The same rule the crawler's `package-hint` is held to, stated once here. A remote entry's
/// path becomes a file name and a folder under somebody's download directory, so a `..`, an
/// absolute path or a drive letter in it is not a path — it is an attempt to leave.
fn safe_path(path: &str) -> String {
    path.split(['/', '\\'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .map(safe_name)
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// One path segment, reduced to something that can stand as a name.
fn safe_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_control() && !matches!(character, '/' | '\\' | ':'))
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .trim()
        .chars()
        .take(200)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{is_usable_key, safe_name, safe_path};

    /// A path out of a torrent is a stranger's data structure, and the one thing it may never
    /// do is name a place outside the job it came from.
    #[test]
    fn a_remote_path_cannot_leave_the_job_it_came_from() {
        assert_eq!(safe_path("Show/Season 1/ep.mkv"), "Show/Season 1/ep.mkv");
        assert_eq!(safe_path("../../etc/passwd"), "etc/passwd");
        assert_eq!(safe_path("/absolute/file.bin"), "absolute/file.bin");
        // A backslash is a separator here too, so a drive letter loses its colon and becomes
        // an ordinary relative folder rather than a root.
        assert_eq!(
            safe_path("C:\\Windows\\system32\\x"),
            "C/Windows/system32/x"
        );
        assert_eq!(safe_path("./a/./b"), "a/b");
        assert_eq!(safe_path("   "), "");
    }

    /// Control characters in a name reach a log line and a file system; both are reasons to
    /// drop them here rather than to hope.
    #[test]
    fn a_remote_name_keeps_nothing_that_is_not_a_name() {
        assert_eq!(safe_name("ep\u{0}01.mkv"), "ep01.mkv");
        assert_eq!(safe_name("  spaced.mkv  "), "spaced.mkv");
        assert_eq!(safe_name("...hidden..."), "hidden");
        assert_eq!(safe_name(&"x".repeat(400)).len(), 200);
    }

    /// The content key is what the duplicate guard is built on, so a key the host could not
    /// store is refused rather than written down in a shortened form nobody can match again.
    #[test]
    fn a_content_key_the_host_cannot_store_is_refused() {
        assert!(is_usable_key("c8f1a0b2"));
        assert!(!is_usable_key(""));
        assert!(!is_usable_key("   "));
        assert!(!is_usable_key(&"a".repeat(257)));
        assert!(!is_usable_key("has\na newline"));
    }
}
