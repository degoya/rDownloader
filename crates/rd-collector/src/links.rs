use std::{collections::HashSet, sync::LazyLock};

use regex::Regex;
use url::Url;

/// Compiled once. `parse_link_list` calls `extract_urls` per line, so rebuilding this made a
/// large pasted list pay for tens of thousands of regex compilations to run as many matches.
static URL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(https?://|magnet:\?|ftps?://|sftp://|webdavs?://|davs?://)[^\s<>\"']+"#)
        .expect("static URL regex")
});

/// Extracts unique download URLs without retaining unrelated clipboard text.
///
/// Covers HTTP(S), magnets and the milestone 0.6 transfer protocols. `ftp://user:pw@host`
/// is accepted deliberately — that is how such a link is normally pasted — and the
/// credentials are split off and vaulted at intake rather than being rejected here.
#[must_use]
pub fn extract_urls(text: &str) -> Vec<Url> {
    let mut seen = HashSet::new();
    URL_PATTERN
        .find_iter(text)
        .filter_map(|candidate| {
            let trimmed = candidate
                .as_str()
                .trim_end_matches(['.', ',', ';', ')', ']']);
            Url::parse(trimmed).ok()
        })
        .map(canonical_url)
        .filter(|url| seen.insert(url.as_str().to_owned()))
        .collect()
}

/// Rewrites alias hosts (e.g. `ddl.to`) to the canonical hoster domain and short video
/// links (`youtu.be/<id>`) to their watch page.
#[must_use]
pub fn canonical_url(mut url: Url) -> Url {
    // Hoster aliasing rewrites the host *and* forces https, which would turn an `ftp://`
    // link into an HTTP one. The remote transfer schemes address a specific server and are
    // never aliases of a filehoster, so they are left exactly as typed.
    if rd_core::RemoteProtocol::from_url_scheme(url.scheme()).is_some() {
        return url;
    }
    let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
        return url;
    };
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if host == "youtu.be" {
        let id = url.path().trim_matches('/').to_owned();
        if !id.is_empty()
            && let Ok(mut watch) = Url::parse("https://www.youtube.com/watch")
        {
            watch.query_pairs_mut().append_pair("v", &id);
            if let Some(query) = url.query() {
                for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
                    if key == "t" || key == "list" {
                        watch.query_pairs_mut().append_pair(&key, &value);
                    }
                }
            }
            return watch;
        }
    }
    if let Some((_, canonical)) = rd_provider_registry::host_aliases()
        .into_iter()
        .find(|(alias, _)| *alias == host)
        && url.set_host(Some(&canonical)).is_ok()
    {
        let _ = url.set_scheme("https");
    }
    url
}

#[cfg(test)]
pub(crate) mod tests {
    use url::Url;

    use super::{canonical_url, extract_urls};

    /// Registers one provider row carrying an alias, the way an installed plugin's manifest does.
    ///
    /// These assertions used to lean on the eleven hoster rows compiled into the binary. Since
    /// RD-101-13 a provider exists only while its plugin does, so a test about alias rewriting
    /// has to supply the alias — which is honest anyway: what is under test is that an alias is
    /// rewritten, not which hoster happens to ship one.
    pub(crate) fn register_alias() {
        rd_provider_registry::replace_dynamic(vec![rd_provider_registry::DynamicProvider {
            plugin_id: "plugin-fixture".to_owned(),
            spec: rd_provider_registry::ProviderSpec {
                slug: "fixture".to_owned(),
                display_name: "Fixture".to_owned(),
                kind: rd_provider_registry::ProviderKind::Hoster,
                credentials: rd_provider_registry::CredentialKind::ApiKey,
                username_required: false,
                transfer_auth: rd_provider_registry::TransferAuth::None,
                secrets: Vec::new(),
                request_domains: vec!["fixture.test".to_owned()],
                cookie_scope: None,
                match_hosts: vec!["fixture.test".to_owned()],
                host_aliases: vec![("alias.test".to_owned(), "fixture.test".to_owned())],
                source: rd_provider_registry::ProviderSource::Plugin,
                plugin_id: Some("plugin-fixture".to_owned()),
                plugin_version: Some("1.0.0".to_owned()),
            },
        }]);
    }

    #[test]
    fn extracts_only_unique_download_urls() {
        let input = "secret text https://example.com/a, https://example.com/a mailto:a@b.test";
        let urls = extract_urls(input);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].as_str(), "https://example.com/a");
    }

    #[test]
    fn transfer_protocol_links_are_picked_up() {
        // Until milestone 0.6 an `ftp://` link was dropped at intake; these four schemes
        // are the ones the new transports serve.
        let urls = extract_urls(
            "ftp://files.example.com/pub/a.bin sftp://box.example/srv/b.bin \
             davs://cloud.example/dav/c.bin ftps://files.example.com/pub/d.bin",
        );
        let schemes: Vec<&str> = urls.iter().map(Url::scheme).collect();
        assert_eq!(schemes, ["ftp", "sftp", "davs", "ftps"]);
    }

    #[test]
    fn a_remote_link_is_never_rewritten_to_https() {
        // Hoster aliasing rewrites the host *and* forces https; applying it to a transfer
        // URL would silently change the protocol the user asked for.
        register_alias();
        let ftp: Url = "ftp://alias.test/pub/file.rar".parse().expect("url");
        assert_eq!(canonical_url(ftp.clone()), ftp);
    }

    #[test]
    fn rewrites_alias_hosts_to_the_canonical_hoster() {
        register_alias();
        let urls = extract_urls(
            "https://alias.test/ga54c0dlen1e/file.rar http://www.alias.test/x1y2z3a4b5c6",
        );
        assert_eq!(
            urls[0].as_str(),
            "https://fixture.test/ga54c0dlen1e/file.rar"
        );
        assert_eq!(urls[1].as_str(), "https://fixture.test/x1y2z3a4b5c6");
        let plain = "https://example.com/a".parse().expect("url");
        assert_eq!(
            canonical_url(plain),
            "https://example.com/a".parse().expect("url")
        );
    }

    #[test]
    fn expands_short_youtube_links_to_watch_pages() {
        let short: Url = "https://youtu.be/dQw4w9WgXcQ?t=42".parse().expect("url");
        assert_eq!(
            canonical_url(short).as_str(),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42"
        );
    }
}
