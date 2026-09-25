//! Image-gallery downloads fetched through external `gallery-dl`: which hosts route to it
//! and the tool settings (part of the `service.settings` blob, keys prefixed `gallery_`).

use serde::{Deserialize, Serialize};

/// Provider name stored on gallery candidates.
pub const GALLERY_PROVIDER: &str = "gallery";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct GallerySettings {
    /// Absolute path of gallery-dl; `None` = vendor folders and `PATH`.
    pub gallery_executable: Option<String>,
    /// Hosts (without `www.`) handled by gallery-dl.
    pub gallery_hosts: Vec<String>,
    /// Concurrent gallery downloads (1–8).
    pub gallery_max_parallel: u32,
    /// Shared with the other tools; searched before `PATH`.
    pub vendor_directory: Option<String>,
}

impl GallerySettings {
    /// Popular gallery-dl sites that are not already routed to the media provider. The full
    /// extractor list is much longer; the host list is editable in the settings.
    #[must_use]
    pub fn default_hosts() -> Vec<String> {
        [
            "pixiv.net",
            "deviantart.com",
            "artstation.com",
            "flickr.com",
            "danbooru.donmai.us",
            "gelbooru.com",
            "e621.net",
            "rule34.xxx",
            "kemono.cr",
            "fanbox.cc",
            "redgifs.com",
            "imgbox.com",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    /// Whether `host` (any casing, optional `www.`) belongs to the gallery provider.
    #[must_use]
    pub fn handles_host(&self, host: &str) -> bool {
        let host = host.trim_start_matches("www.").to_ascii_lowercase();
        self.gallery_hosts
            .iter()
            .any(|entry| host == *entry || host.ends_with(&format!(".{entry}")))
    }
}

impl Default for GallerySettings {
    fn default() -> Self {
        Self {
            gallery_executable: None,
            gallery_hosts: Self::default_hosts(),
            gallery_max_parallel: 2,
            vendor_directory: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GallerySettings;

    #[test]
    fn host_matching_ignores_www_case_and_subdomains() {
        let settings = GallerySettings::default();
        assert!(settings.handles_host("www.Pixiv.net"));
        assert!(!settings.handles_host("safebooru.donmai.us"));
        assert!(settings.handles_host("danbooru.donmai.us"));
        assert!(!settings.handles_host("example.com"));
    }
}
