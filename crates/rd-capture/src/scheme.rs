//! Parsing an `rdownloader://` address into something safe to act on (RD-090-09).
//!
//! A URL scheme handler is reachable from any web page: anything can put an
//! `rdownloader://…` link on a site and have a browser hand it to this binary. So the parser
//! is an allowlist, not a translator. It decides what may be asked for at all, and anything
//! it does not recognise is refused rather than passed on and interpreted later.

use anyhow::{Result, bail, ensure};
use url::Url;

/// The scheme this handler is registered for.
pub const SCHEME: &str = "rdownloader";

/// Longest address accepted, so a page cannot hand over a megabyte of query string.
const MAX_URL_CHARS: usize = 8192;
/// Most links accepted in one hand-over.
const MAX_LINKS: usize = 50;

/// Schemes a handed-over link may itself use.
///
/// `file:` is absent on purpose: a page that could make the agent read a local path and
/// send it somewhere would turn the handler into a file-exfiltration primitive.
const ALLOWED_LINK_SCHEMES: &[&str] = &["http", "https", "ftp", "ftps", "sftp", "magnet"];

/// File types an `open` address may point at.
///
/// `.nzb` only. `.torrent` used to be listed here and in the two doc comments below, but
/// `Action::OpenFile` ends in `open()`, which uploads the content as `application/x-nzb` to the
/// agent's NZB endpoint -- so a `.torrent` parsed cleanly and then broke at the endpoint. An
/// honest refusal is better than a promise that does not hold; a real torrent import path is its
/// own piece of work with its own acceptance, not a side effect of this one (RD-109-03).
const ALLOWED_FILE_EXTENSIONS: &[&str] = &["nzb"];

/// What an `rdownloader://` address asks for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Hand links to the LinkGrabber.
    Links(Vec<String>),
    /// Import a local `.nzb` file.
    OpenFile(std::path::PathBuf),
}

/// Parses an address, refusing anything outside the allowlist.
///
/// Accepted forms:
/// - `rdownloader://add?url=<link>` (repeatable, or `urls=` with newline-separated links)
/// - `rdownloader://add?url=magnet:?xt=…`
/// - `rdownloader://open?path=<absolute local path to a .nzb>`
pub fn parse(input: &str) -> Result<Action> {
    ensure!(
        input.chars().count() <= MAX_URL_CHARS,
        "address is too long"
    );
    let parsed = Url::parse(input).map_err(|error| anyhow::anyhow!("not a URL: {error}"))?;
    ensure!(parsed.scheme() == SCHEME, "not an {SCHEME} address");
    // The host carries the verb: browsers normalise `rdownloader://add?…` so that `add` is
    // the host, not the path. Both spellings are accepted because a hand-written link may
    // use either, and neither is the user's mistake to debug.
    let action = parsed
        .host_str()
        .map(str::to_owned)
        .or_else(|| {
            parsed
                .path()
                .trim_matches('/')
                .split('/')
                .next()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    match action.as_str() {
        "add" => links(&parsed),
        "open" => open(&parsed),
        "" => bail!("address names no action"),
        other => bail!("unknown action: {other}"),
    }
}

fn links(parsed: &Url) -> Result<Action> {
    let mut collected = Vec::new();
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "url" => collected.push(value.into_owned()),
            "urls" => collected.extend(
                value
                    .split(['\n', '\r'])
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned),
            ),
            _ => {}
        }
    }
    ensure!(!collected.is_empty(), "address carries no link");
    ensure!(
        collected.len() <= MAX_LINKS,
        "address carries more than {MAX_LINKS} links"
    );
    let mut accepted = Vec::new();
    for link in collected {
        let link_url = Url::parse(&link)
            .map_err(|error| anyhow::anyhow!("handed-over link is not a URL: {error}"))?;
        ensure!(
            ALLOWED_LINK_SCHEMES.contains(&link_url.scheme()),
            "handed-over link uses the refused scheme {}",
            link_url.scheme()
        );
        accepted.push(link);
    }
    Ok(Action::Links(accepted))
}

/// Whether the path names anything other than a plain local file.
///
/// Decided on the text rather than on [`std::path::Component::Prefix`], because the answer has
/// to be the same on every host: a prefix component is only ever produced by the Windows path
/// parser, so on Linux -- where this is built and tested -- `//evil.test/s/x.nzb` looks like an
/// ordinary absolute path and would pass. Both leading separators are checked in both spellings,
/// which covers the UNC forms (`\\host\share`, `//host/share`), the verbatim ones
/// (`\\?\...`, `\\?\UNC\...`) and the device namespace (`\\.\...`).
///
/// The prefix check is kept alongside it: on Windows it is the authoritative reading of the same
/// question, and a drive letter is the one prefix kind that is allowed through.
fn names_a_remote_or_device_location(path: &std::path::Path) -> bool {
    let text = path.to_string_lossy();
    let mut leading = text.chars();
    let first_two = (leading.next(), leading.next());
    let separator = |value: Option<char>| matches!(value, Some('/' | '\\'));
    if separator(first_two.0) && separator(first_two.1) {
        return true;
    }
    path.components().any(|component| {
        matches!(component, std::path::Component::Prefix(prefix)
            if !matches!(prefix.kind(), std::path::Prefix::Disk(_)))
    })
}

fn open(parsed: &Url) -> Result<Action> {
    let path = parsed
        .query_pairs()
        .find(|(key, _)| key == "path")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default();
    ensure!(!path.trim().is_empty(), "address carries no path");
    let path = std::path::PathBuf::from(path);
    // Absolute only. A relative path would resolve against whatever directory the browser
    // happened to start the handler in, which is neither predictable nor the user's intent.
    // Absolute is not the same as local. `//evil.test/share/x.nzb` and `\\evil.test\share\x.nzb`
    // are absolute on Windows too, and opening one makes Windows build an SMB connection to a
    // host a web page chose: a forced network fetch and an NTLM credential leak, from a handler
    // any page can reach (RD-109-03). Checked before `is_absolute`, so the refusal reads the same
    // on every host: the backslash spelling is not absolute on Unix at all.
    ensure!(
        !names_a_remote_or_device_location(&path),
        "path must name a local file, not a network share or a device namespace"
    );
    ensure!(path.is_absolute(), "path must be absolute");
    ensure!(
        !path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir)),
        "path must not contain .."
    );
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    ensure!(
        ALLOWED_FILE_EXTENSIONS.contains(&extension.as_str()),
        "only .nzb files can be opened"
    );
    Ok(Action::OpenFile(path))
}

#[cfg(test)]
mod tests {
    use super::{Action, MAX_LINKS, parse};

    #[test]
    fn a_link_is_handed_over() {
        assert_eq!(
            parse("rdownloader://add?url=https://example.com/file.bin").expect("parse"),
            Action::Links(vec!["https://example.com/file.bin".to_owned()])
        );
    }

    #[test]
    fn several_links_arrive_in_order() {
        let parsed = parse("rdownloader://add?url=https://example.com/a&url=https://example.com/b")
            .expect("parse");
        assert_eq!(
            parsed,
            Action::Links(vec![
                "https://example.com/a".to_owned(),
                "https://example.com/b".to_owned()
            ])
        );
    }

    #[test]
    fn a_magnet_is_accepted() {
        let magnet = "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01";
        let encoded = format!(
            "rdownloader://add?url={}",
            magnet
                .replace(':', "%3A")
                .replace('?', "%3F")
                .replace('&', "%26")
        );
        assert_eq!(
            parse(&encoded).expect("parse"),
            Action::Links(vec![magnet.to_owned()])
        );
    }

    #[test]
    fn a_local_file_scheme_is_refused() {
        // The handler is reachable from any web page. A `file:` link would let a page make
        // the agent read a local path and send it to the service.
        for link in [
            "rdownloader://add?url=file%3A%2F%2F%2Fetc%2Fpasswd",
            "rdownloader://add?url=javascript%3Aalert(1)",
            "rdownloader://add?url=data%3Atext%2Fhtml%2Chi",
        ] {
            assert!(parse(link).is_err(), "{link} was accepted");
        }
    }

    #[test]
    fn an_unknown_action_is_refused_rather_than_guessed_at() {
        for input in [
            "rdownloader://",
            "rdownloader://delete?id=1",
            "rdownloader://settings?admin=true",
            "https://example.com/",
            "not a url",
        ] {
            assert!(parse(input).is_err(), "{input} was accepted");
        }
    }

    #[test]
    fn an_over_long_address_is_refused_before_it_is_parsed() {
        let long = format!(
            "rdownloader://add?url=https://example.com/{}",
            "a".repeat(9000)
        );
        assert!(parse(&long).is_err());
    }

    #[test]
    fn more_links_than_the_cap_are_refused() {
        let links: Vec<String> = (0..=MAX_LINKS)
            .map(|index| format!("url=https://example.com/{index}"))
            .collect();
        let input = format!("rdownloader://add?{}", links.join("&"));
        assert!(parse(&input).is_err());
    }

    #[test]
    fn only_nzb_files_can_be_opened() {
        assert_eq!(
            parse("rdownloader://open?path=%2Ftmp%2Frelease.nzb").expect("parse"),
            Action::OpenFile(std::path::PathBuf::from("/tmp/release.nzb"))
        );
        assert!(parse("rdownloader://open?path=%2Fetc%2Fpasswd").is_err());
        assert!(parse("rdownloader://open?path=%2Ftmp%2Fx.sh").is_err());
        // `.torrent` parsed cleanly and then broke at the NZB endpoint it was handed to. It is
        // refused here instead, until there is a path that really imports one (RD-109-03).
        assert!(parse("rdownloader://open?path=%2Ftmp%2Frelease.torrent").is_err());
    }

    /// A web page may hand this handler an address. `//host/share/x.nzb` is absolute on
    /// Windows, so `is_absolute()` alone let it through -- and opening it makes Windows dial
    /// SMB to a host the page named, which forces a network fetch and leaks NTLM credentials.
    #[test]
    fn a_network_share_or_device_path_is_refused() {
        for address in [
            // `//host/share/x.nzb`
            "rdownloader://open?path=%2F%2Fevil.test%2Fshare%2Fx.nzb",
            // `\\host\share\x.nzb`
            "rdownloader://open?path=%5C%5Cevil.test%5Cshare%5Cx.nzb",
            // The mixed spellings Windows also accepts.
            "rdownloader://open?path=%2F%5Cevil.test%2Fshare%2Fx.nzb",
            "rdownloader://open?path=%5C%2Fevil.test%5Cshare%5Cx.nzb",
            // `\\?\UNC\host\share\x.nzb` and the device namespace.
            "rdownloader://open?path=%5C%5C%3F%5CUNC%5Cevil.test%5Cshare%5Cx.nzb",
            "rdownloader://open?path=%5C%5C.%5CGLOBALROOT%5Cx.nzb",
        ] {
            let error = parse(address).expect_err("{address} was accepted");
            assert!(
                error.to_string().contains("local file"),
                "{address}: {error}"
            );
        }
        // An ordinary absolute path is untouched by the rule.
        assert!(parse("rdownloader://open?path=%2Ftmp%2Frelease.nzb").is_ok());
    }

    #[test]
    fn a_relative_or_traversing_path_is_refused() {
        // A relative path resolves against whatever directory the browser started the
        // handler in; `..` is the same problem written differently.
        assert!(parse("rdownloader://open?path=release.nzb").is_err());
        assert!(parse("rdownloader://open?path=%2Ftmp%2F..%2F..%2Fetc%2Fx.nzb").is_err());
    }

    #[test]
    fn an_address_without_a_payload_is_refused() {
        assert!(parse("rdownloader://add").is_err());
        assert!(parse("rdownloader://add?url=").is_err());
        assert!(parse("rdownloader://open").is_err());
        assert!(parse("rdownloader://open?path=%20").is_err());
    }
}
