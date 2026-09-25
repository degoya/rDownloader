use anyhow::{Context, Result, bail};
use async_trait::async_trait;

use crate::{DecodedArticle, NntpClient, decode_yenc};

/// One ordered article reference from an NZB file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentRequest {
    pub number: u32,
    pub message_id: String,
}

/// Fully decoded file ready for crash-safe staging persistence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssembledFile {
    pub name: String,
    pub data: Vec<u8>,
    pub segment_crc32: Vec<(u32, u32)>,
}

/// Minimal article source abstraction used by NNTP clients and deterministic fixtures.
#[async_trait]
pub trait ArticleSource: Send {
    async fn article(&mut self, message_id: &str) -> Result<Vec<u8>>;
}

#[async_trait]
impl ArticleSource for NntpClient {
    async fn article(&mut self, message_id: &str) -> Result<Vec<u8>> {
        self.body(message_id).await
    }
}

/// Downloads ordered segments, retries missing or corrupt data on backup sources, and assembles it.
pub async fn download_file(
    sources: &mut [Box<dyn ArticleSource>],
    mut requests: Vec<SegmentRequest>,
) -> Result<AssembledFile> {
    if sources.is_empty() {
        bail!("no NNTP article sources are available");
    }
    requests.sort_by_key(|segment| segment.number);
    if requests.is_empty() {
        bail!("NZB file contains no segments");
    }
    let mut name = None;
    let mut declared_size = None;
    let mut data = Vec::new();
    let mut checksums = Vec::with_capacity(requests.len());
    let mut deviations = NameDeviations::default();
    for request in requests {
        let (decoded, _) = fetch_decoded_with_attempts(sources, &request.message_id).await?;
        deviations.observe(name.as_deref(), &decoded.metadata.name);
        if declared_size.is_some_and(|value| value != decoded.metadata.declared_size) {
            bail!("yEnc segments disagree on the declared file size");
        }
        validate_part_position(&decoded, data.len() as u64)?;
        name.get_or_insert_with(|| decoded.metadata.name.clone());
        declared_size.get_or_insert(decoded.metadata.declared_size);
        checksums.push((request.number, decoded.crc32));
        data.extend_from_slice(&decoded.data);
    }
    let expected = declared_size.context("missing yEnc size")?;
    if data.len() as u64 != expected {
        bail!(
            "assembled yEnc size mismatch: expected {expected}, got {}",
            data.len()
        );
    }
    let name = name.context("missing yEnc filename")?;
    deviations.report(&name);
    Ok(AssembledFile {
        name,
        data,
        segment_crc32: checksums,
    })
}

pub(crate) async fn fetch_decoded_with_attempts(
    sources: &mut [Box<dyn ArticleSource>],
    message_id: &str,
) -> Result<(DecodedArticle, u32)> {
    let mut failures = Vec::new();
    for source in sources {
        match source
            .article(message_id)
            .await
            .and_then(|body| decode_yenc(&body))
        {
            Ok(decoded) => {
                let attempts = u32::try_from(failures.len().saturating_add(1))?;
                return Ok((decoded, attempts));
            }
            Err(error) => failures.push(error.to_string()),
        }
    }
    bail!(
        "segment {message_id} failed on every server: {}",
        failures.join("; ")
    )
}

/// Posters (and obfuscation tools) frequently vary the yEnc name per part; like SABnzbd the
/// first part decides the output name and later deviations are only logged.
///
/// Logged **once per file**, which is the whole point of counting them here. A fully obfuscated
/// set gives every article its own random name, so one warning per segment turned a single
/// download into hundreds of identical lines -- seen on a live run on 2026-09-23, where the only
/// thing that differed between them was a name nothing ever used.
#[derive(Default)]
pub(crate) struct NameDeviations {
    count: u64,
    first: Option<String>,
}

impl NameDeviations {
    pub(crate) fn observe(&mut self, current: Option<&str>, candidate: &str) {
        if current.is_some_and(|current| current != candidate) {
            self.count = self.count.saturating_add(1);
            if self.first.is_none() {
                self.first = Some(candidate.to_owned());
            }
        }
    }

    /// Whether any part disagreed -- the signal that the yEnc names are not to be trusted and
    /// the NZB subject carries the real one.
    pub(crate) fn any(&self) -> bool {
        self.first.is_some()
    }

    /// `chosen` is the name the file actually ends up with, which after a disagreement is
    /// usually none of the names the articles carried.
    pub(crate) fn report(&self, chosen: &str) {
        if let Some(first) = self.first.as_deref() {
            tracing::warn!(
                chosen,
                first_deviation = first,
                deviating_segments = self.count,
                "yEnc segments disagree on the output filename"
            );
        }
    }
}

pub(crate) fn validate_part_position(article: &DecodedArticle, assembled_bytes: u64) -> Result<()> {
    match (article.metadata.part_begin, article.metadata.part_end) {
        (Some(begin), Some(end)) => {
            let expected_begin = assembled_bytes.saturating_add(1);
            if begin != expected_begin || end < begin {
                bail!("non-contiguous yEnc part range {begin}-{end}");
            }
            if end - begin + 1 != article.data.len() as u64 {
                bail!("yEnc part range does not match decoded length");
            }
        }
        (None, None) if assembled_bytes == 0 => {}
        (None, None) => bail!("multiple yEnc segments require explicit part ranges"),
        _ => bail!("incomplete yEnc part range"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use anyhow::Result;
    use async_trait::async_trait;

    use super::{ArticleSource, NameDeviations, SegmentRequest, download_file};

    struct FixtureSource(VecDeque<Result<Vec<u8>>>);

    #[async_trait]
    impl ArticleSource for FixtureSource {
        async fn article(&mut self, _message_id: &str) -> Result<Vec<u8>> {
            self.0.pop_front().expect("fixture response")
        }
    }

    #[tokio::test]
    async fn corrupt_primary_segment_is_retried_on_backup() {
        let corrupt =
            b"=ybegin line=128 size=1 name=file.bin\r\n+\r\n=yend size=1 crc32=00000000\r\n"
                .to_vec();
        let valid = format!(
            "=ybegin line=128 size=1 name=file.bin\r\n+\r\n=yend size=1 crc32={:08x}\r\n",
            crc32fast::hash(&[1])
        )
        .into_bytes();
        let mut sources: Vec<Box<dyn ArticleSource>> = vec![
            Box::new(FixtureSource(VecDeque::from([Ok(corrupt)]))),
            Box::new(FixtureSource(VecDeque::from([Ok(valid)]))),
        ];

        let result = download_file(
            &mut sources,
            vec![SegmentRequest {
                number: 1,
                message_id: "segment-1".to_owned(),
            }],
        )
        .await
        .expect("backup result");
        assert_eq!(result.name, "file.bin");
        assert_eq!(result.data, [1]);
    }

    #[tokio::test]
    async fn differing_part_names_keep_the_first_name() {
        let part = |name: &str, begin: u64, byte: u8| {
            format!(
                "=ybegin part=1 line=128 size=2 name={name}\r\n=ypart begin={begin} end={begin}\r\n{}\r\n=yend size=1 part=1 pcrc32={:08x}\r\n",
                char::from(byte.wrapping_add(42)),
                crc32fast::hash(&[byte])
            )
            .into_bytes()
        };
        let mut sources: Vec<Box<dyn ArticleSource>> =
            vec![Box::new(FixtureSource(VecDeque::from([
                Ok(part("release.rar", 1, 1)),
                Ok(part("obfuscated.bin", 2, 2)),
            ])))];
        let result = download_file(
            &mut sources,
            vec![
                SegmentRequest {
                    number: 1,
                    message_id: "segment-1".to_owned(),
                },
                SegmentRequest {
                    number: 2,
                    message_id: "segment-2".to_owned(),
                },
            ],
        )
        .await
        .expect("assembled despite differing part names");
        assert_eq!(result.name, "release.rar");
        assert_eq!(result.data, [1, 2]);
    }

    #[tokio::test]
    async fn unavailable_on_every_server_reports_all_failures() {
        let mut sources: Vec<Box<dyn ArticleSource>> = vec![
            Box::new(FixtureSource(VecDeque::from([Err(anyhow::anyhow!(
                "missing"
            ))]))),
            Box::new(FixtureSource(VecDeque::from([Err(anyhow::anyhow!(
                "offline"
            ))]))),
        ];
        let error = download_file(
            &mut sources,
            vec![SegmentRequest {
                number: 1,
                message_id: "segment-1".to_owned(),
            }],
        )
        .await
        .expect_err("all servers fail");
        let message = error.to_string();
        assert!(message.contains("missing"), "{message}");
        assert!(message.contains("offline"), "{message}");
    }

    #[test]
    fn a_fully_obfuscated_set_reports_one_line_for_all_of_its_parts() {
        // Every article carries its own random name, which is what obfuscation does. The
        // counter has to end at "many deviations", not "many warnings".
        let mut deviations = NameDeviations::default();
        deviations.observe(None, "7ca65a76c4373f7247572df3dc554fbba");
        for candidate in [
            "764004c64ef80f1216d2b093f83a83a5",
            "G42WMMBRGY2TSNZZGNTDCMTGGFDQOBY",
        ] {
            deviations.observe(Some("7ca65a76c4373f7247572df3dc554fbba"), candidate);
        }
        assert!(deviations.any());
        assert_eq!(deviations.count, 2);
        // The first deviation is kept because it is the only one worth naming; the rest are a
        // number.
        assert_eq!(
            deviations.first.as_deref(),
            Some("764004c64ef80f1216d2b093f83a83a5")
        );
    }

    #[test]
    fn a_set_whose_parts_agree_reports_nothing() {
        let mut deviations = NameDeviations::default();
        deviations.observe(None, "release.rar");
        deviations.observe(Some("release.rar"), "release.rar");
        assert!(!deviations.any());
        assert_eq!(deviations.count, 0);
    }
}
