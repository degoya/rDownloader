//! Contracts shared by the FTP/FTPS, SFTP and WebDAV transfer sources (milestone 0.6):
//! stored logins, the SSH host-key trust store, and the directory listing that is reviewed
//! in the LinkGrabber before anything is queued.

mod credential;
mod listing;
mod settings;

pub use credential::{
    MAX_REMOTE_HOST, MAX_REMOTE_KEY, MAX_REMOTE_SECRET, RemoteAuthMode, RemoteCredential,
    RemoteFamily, RemoteProtocol, RemoteTarget, SshHostKey,
};
pub use listing::{
    ListingLimit, MAX_REMOTE_DEPTH, MAX_REMOTE_ENTRIES, MAX_REMOTE_PATH, REMOTE_CONTRACT_VERSION,
    RemoteCandidateState, RemoteEntry, RemoteListing, RemoteListingPlan, RemoteListingSummary,
    ResolvedRemoteEntry, ResolvedRemoteListing, is_safe_relative_path, resolve_listing,
};
pub use settings::{FTP_PROVIDER, RemoteSettings, SFTP_PROVIDER, WEBDAV_PROVIDER};
