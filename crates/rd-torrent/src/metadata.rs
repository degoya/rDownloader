//! Turning `.torrent` bytes into the domain metadata rDownloader persists.
//!
//! librqbit validates the info dictionary but keeps its own representation; this module
//! projects it onto [`TorrentMetadataInfo`], which is what the API, the file plan and the
//! UI work with. The file indices produced here are the librqbit indices, which is what
//! makes them usable as `only_files` keys.

use anyhow::{Context, Result};
use rd_core::{ByteCount, TorrentFileEntry, TorrentMetadataInfo, TorrentTracker, TrackerOrigin};

/// Summary of a parsed `.torrent` file.
///
/// Kept alongside the full [`TorrentMetadataInfo`] because the intake path only needs the
/// four scalar values to create a link candidate.
#[derive(Clone, Debug)]
pub struct ParsedTorrent {
    pub info_hash: String,
    pub name: String,
    pub total_bytes: u64,
    pub file_count: u64,
    /// The full tree, trackers and web seeds.
    pub metadata: TorrentMetadataInfo,
}

/// Projects a validated info dictionary onto the domain metadata.
///
/// Shared by the `.torrent` path and the magnet path so both produce exactly the same
/// model once the metadata is known; only the tracker and web-seed sources differ.
pub(crate) fn project(
    validated: &librqbit::ValidatedTorrentMetaV1Info<impl AsRef<[u8]>>,
    info_hash: String,
    private: bool,
    trackers: Vec<TorrentTracker>,
    web_seeds: Vec<String>,
) -> Result<TorrentMetadataInfo> {
    let name = validated
        .name()
        .map(|name| name.into_owned())
        .unwrap_or_else(|| info_hash.clone());
    let files: Vec<TorrentFileEntry> = validated
        .iter_file_details()
        .enumerate()
        .map(|(index, details)| {
            Ok(TorrentFileEntry {
                index: u32::try_from(index).context("torrent has too many files")?,
                path: details.filename.to_vec(),
                length: ByteCount::new(details.len).map_err(anyhow::Error::msg)?,
            })
        })
        .collect::<Result<_>>()?;
    let total_bytes: u64 = files.iter().map(|file| file.length.get()).sum();
    let lengths = validated.lengths();
    Ok(TorrentMetadataInfo {
        info_hash,
        name,
        total_bytes: ByteCount::new(total_bytes).map_err(anyhow::Error::msg)?,
        piece_length: u64::from(lengths.default_piece_length()),
        piece_count: lengths.total_pieces(),
        private,
        files,
        trackers,
        web_seeds,
    })
}

/// Trackers advertised by a magnet link's `tr` parameters, in order.
#[must_use]
pub(crate) fn magnet_trackers(magnet: &url::Url) -> Vec<TorrentTracker> {
    let mut trackers: Vec<TorrentTracker> = Vec::new();
    for (key, value) in magnet.query_pairs() {
        if key != "tr" || value.is_empty() {
            continue;
        }
        let candidate = TorrentTracker::new(value.into_owned(), 0, TrackerOrigin::Metadata);
        if !trackers.iter().any(|existing| existing.id == candidate.id) {
            trackers.push(candidate);
        }
    }
    trackers
}

/// Web seeds advertised by a magnet link's `ws` parameters.
#[must_use]
pub(crate) fn magnet_web_seeds(magnet: &url::Url) -> Vec<String> {
    magnet
        .query_pairs()
        .filter(|(key, value)| key == "ws" && !value.is_empty())
        .map(|(_, value)| value.into_owned())
        .collect()
}

/// Parses `.torrent` bytes without adding them to the session.
pub fn parse_torrent(bytes: &[u8]) -> Result<ParsedTorrent> {
    let meta = librqbit::torrent_from_bytes(bytes).context("parse .torrent file")?;
    let info_hash = meta.info_hash.as_string();
    // `validate` consumes the info dictionary, so anything read straight off it has to be
    // taken first.
    let private = meta.info.data.private;
    let trackers = announce_trackers(&meta);
    let validated = meta
        .info
        .data
        .clone()
        .validate()
        .context("validate .torrent metadata")?;
    let metadata = project(
        &validated,
        info_hash.clone(),
        private,
        trackers,
        web_seeds(bytes),
    )?;
    Ok(ParsedTorrent {
        info_hash,
        name: metadata.name.clone(),
        total_bytes: metadata.total_bytes.get(),
        file_count: metadata.files.len() as u64,
        metadata,
    })
}

/// Collects the announce list, preserving BEP 12 tiers.
///
/// The outer list index is the tier; a bare `announce` key without an announce list is
/// tier 0. Duplicates are dropped so a torrent that repeats its tracker in both keys does
/// not produce two entries with the same id.
fn announce_trackers(meta: &librqbit::TorrentMetaV1<impl AsRef<[u8]>>) -> Vec<TorrentTracker> {
    let mut trackers: Vec<TorrentTracker> = Vec::new();
    let mut push = |url: String, tier: u16| {
        let candidate = TorrentTracker::new(url, tier, TrackerOrigin::Metadata);
        if !trackers.iter().any(|existing| existing.id == candidate.id) {
            trackers.push(candidate);
        }
    };
    for (tier, urls) in meta.announce_list.iter().enumerate() {
        let tier = u16::try_from(tier).unwrap_or(u16::MAX);
        for url in urls {
            if let Ok(url) = std::str::from_utf8(url.as_ref()) {
                push(url.to_owned(), tier);
            }
        }
    }
    if let Some(announce) = meta.announce.as_ref()
        && let Ok(url) = std::str::from_utf8(announce.as_ref())
    {
        push(url.to_owned(), 0);
    }
    trackers
}

/// Extracts the BEP 19 `url-list` web seeds straight from the bencode.
///
/// librqbit's metainfo type drops the key entirely, and the engine cannot use web seeds
/// anyway — they are parsed purely so the UI can show that a torrent offers them and that
/// the engine is not using them.
fn web_seeds(bytes: &[u8]) -> Vec<String> {
    let Some(value) = crate::bencode::decode(bytes) else {
        return Vec::new();
    };
    let Some(list) = value.get(b"url-list") else {
        return Vec::new();
    };
    // The key is either one URL or a list of them.
    let urls: Vec<&[u8]> = match list.items() {
        Some(items) => items
            .iter()
            .filter_map(crate::bencode::Value::bytes)
            .collect(),
        None => list.bytes().into_iter().collect(),
    };
    urls.into_iter()
        .filter_map(|url| std::str::from_utf8(url).ok())
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{parse_torrent, web_seeds};

    /// Bencoded byte string.
    fn bstr(value: &str) -> Vec<u8> {
        let mut out = format!("{}:", value.len()).into_bytes();
        out.extend_from_slice(value.as_bytes());
        out
    }

    /// A minimal valid single-file torrent, optionally carrying a `url-list`.
    ///
    /// Built by hand rather than with `create_torrent` so the test needs no filesystem.
    /// Dictionary keys are emitted in lexicographic order, as bencode requires.
    fn torrent_bytes(url_list: Option<&[&str]>) -> Vec<u8> {
        let mut bytes = vec![b'd'];
        bytes.extend(bstr("announce"));
        bytes.extend(bstr("http://tracker.example/announce"));
        bytes.extend(bstr("info"));
        bytes.push(b'd');
        bytes.extend(bstr("length"));
        bytes.extend_from_slice(b"i10e");
        bytes.extend(bstr("name"));
        bytes.extend(bstr("file.txt"));
        bytes.extend(bstr("piece length"));
        bytes.extend_from_slice(b"i16384e");
        bytes.extend(bstr("pieces"));
        // Exactly one 20-byte SHA-1 piece hash.
        bytes.extend_from_slice(b"20:");
        bytes.extend_from_slice(&[0_u8; 20]);
        bytes.push(b'e');
        if let Some(seeds) = url_list {
            bytes.extend(bstr("url-list"));
            if let [single] = seeds {
                bytes.extend(bstr(single));
            } else {
                bytes.push(b'l');
                for seed in seeds {
                    bytes.extend(bstr(seed));
                }
                bytes.push(b'e');
            }
        }
        bytes.push(b'e');
        bytes
    }

    #[test]
    fn a_single_file_torrent_yields_one_indexed_entry() {
        let parsed = parse_torrent(&torrent_bytes(None)).expect("parses");
        assert_eq!(parsed.file_count, 1);
        assert_eq!(parsed.total_bytes, 10);
        assert_eq!(parsed.name, "file.txt");
        let file = &parsed.metadata.files[0];
        assert_eq!(file.index, 0);
        assert_eq!(file.display_path(), "file.txt");
        assert_eq!(parsed.metadata.piece_count, 1);
        assert!(!parsed.metadata.private);
    }

    #[test]
    fn the_announce_key_becomes_a_tier_zero_tracker() {
        let parsed = parse_torrent(&torrent_bytes(None)).expect("parses");
        assert_eq!(parsed.metadata.trackers.len(), 1);
        assert_eq!(parsed.metadata.trackers[0].tier, 0);
        assert_eq!(
            parsed.metadata.trackers[0].url,
            "http://tracker.example/announce"
        );
    }

    #[test]
    fn a_single_web_seed_is_read_from_url_list() {
        let parsed =
            parse_torrent(&torrent_bytes(Some(&["https://seed.example/files/"]))).expect("parses");
        assert_eq!(
            parsed.metadata.web_seeds,
            vec!["https://seed.example/files/"]
        );
    }

    #[test]
    fn a_web_seed_list_is_read_from_url_list() {
        let seeds = web_seeds(&torrent_bytes(Some(&[
            "https://a.example/one/",
            "https://b.example/two/",
        ])));
        assert_eq!(
            seeds,
            vec!["https://a.example/one/", "https://b.example/two/"]
        );
    }

    #[test]
    fn a_torrent_without_web_seeds_reports_none() {
        assert!(web_seeds(&torrent_bytes(None)).is_empty());
    }

    #[test]
    fn garbage_is_rejected_rather_than_guessed() {
        assert!(parse_torrent(b"not a torrent").is_err());
        assert!(web_seeds(b"not a torrent").is_empty());
    }
}
