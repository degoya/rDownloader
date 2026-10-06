//! Writing one decoded article into its place in the file being assembled, and the byte
//! ranges the articles have filled.

use std::path::Path;

use anyhow::{Result, bail};
use rd_core::{NzbFileStatus, NzbSegmentStatus};
use rd_db::Database;
use tokio::{
    fs::File,
    io::{AsyncSeekExt, AsyncWriteExt},
};

use crate::segments::NameDeviations;

/// What the assembly of one file has gathered so far.
pub(super) struct Assembly {
    pub(super) output: File,
    pub(super) name: Option<String>,
    pub(super) declared_size: Option<u64>,
    pub(super) written: Covered,
    pub(super) deviations: NameDeviations,
    /// Articles no server had; their bytes are holes.
    pub(super) missing: usize,
    pub(super) single_segment: bool,
}

impl Assembly {
    /// Takes up what the `.part` file already holds, proven by checksum: the assembly so far
    /// and the segments still to fetch.
    pub(super) async fn resume(
        database: &Database,
        nzb_file: &NzbFileStatus,
        part_path: &Path,
    ) -> Result<(Self, Vec<NzbSegmentStatus>)> {
        let resume = crate::assembly_resume::prepare(database, nzb_file, part_path).await?;
        let single_segment = nzb_file.segments.len() == 1;
        let assembly = Self {
            output: resume.output,
            name: resume.name,
            declared_size: resume.declared_size,
            written: Covered::resumed(resume.written)?,
            deviations: NameDeviations::default(),
            missing: 0,
            single_segment,
        };
        Ok((assembly, resume.remaining))
    }

    /// Writes one decoded article where its byte range says it belongs, once it is shown to
    /// fit the file; the range it took.
    pub(super) async fn write(
        &mut self,
        decoded: &crate::DecodedArticle,
        nzb_file: &NzbFileStatus,
        limits: &rd_scheduler::RunLimits,
    ) -> Result<(u64, u64)> {
        // Paces the assembly, which back-pressures the bounded fetch stream behind it —
        // so the NNTP transport honours the same limits as every other transport.
        limits.bandwidth.acquire(decoded.data.len()).await?;
        self.deviations
            .observe(self.name.as_deref(), &decoded.metadata.name);
        validate_metadata(decoded, &mut self.name, &mut self.declared_size, nzb_file)?;
        let (part_begin, part_end) = decoded_part_range(decoded, self.single_segment)?;
        self.written
            .claim(part_begin, part_end, decoded, self.declared_size)?;
        self.output
            .seek(std::io::SeekFrom::Start(part_begin - 1))
            .await?;
        self.output.write_all(&decoded.data).await?;
        // No `sync_data()` here, on purpose (RD-108-25). It looked like a durability
        // guarantee and was a throughput ceiling: one fdatasync per article, in this
        // serial loop, 3.9 ms each on the NVMe it was measured on - a hard cap near
        // 200 MB/s, far lower on NTFS or a spinning disk. The resume never needed it.
        // `assembly_resume::prepare` trusts no checkpoint: on restart it CRC-checks every
        // range the database calls complete against the bytes actually on disk, stops at
        // the first short file or mismatch, truncates to the last proven byte and queues
        // the rest again. Bytes the kernel had not flushed when the power went cost the
        // segments they belonged to, never the file. The one sync that matters, before
        // the rename, stays in `finish`. SABnzbd's assembler forces no write either.
        rd_core::failpoint!("usenet.after_article_write", || {
            anyhow::anyhow!("crash point: usenet.after_article_write")
        });
        Ok((part_begin, part_end))
    }
}

/// The byte ranges articles have filled, 1-based and inclusive.
///
/// What `written: u64` used to say when assembly was strictly front to back (RD-108-26).
/// It answers the two questions that took its place: may this article be written here, and
/// what is left over at the end.
#[derive(Default)]
pub(super) struct Covered(std::collections::BTreeMap<u64, u64>);

impl Covered {
    /// The ranges a resumed `.part` file already holds, proven by checksum.
    fn resumed(ranges: Vec<(u64, u64)>) -> Result<Self> {
        let mut covered = Self::default();
        for (begin, end) in ranges {
            covered.insert(begin, end)?;
        }
        Ok(covered)
    }

    /// Books an article's range, if the range is one this file can hold and nothing has
    /// written there yet.
    fn claim(
        &mut self,
        begin: u64,
        end: u64,
        decoded: &crate::DecodedArticle,
        declared_size: Option<u64>,
    ) -> Result<()> {
        if begin == 0 || end < begin {
            bail!("invalid yEnc part range {begin}-{end}");
        }
        if end - begin + 1 != decoded.data.len() as u64 {
            bail!("yEnc part range does not match decoded length");
        }
        if declared_size.is_some_and(|size| end > size) {
            bail!("yEnc part range {begin}-{end} lies outside the announced file size");
        }
        self.insert(begin, end)
    }

    fn insert(&mut self, begin: u64, end: u64) -> Result<()> {
        // Two articles claiming the same bytes means one of them is not the article it says
        // it is; writing both would leave whichever landed last, silently.
        if let Some((_, earlier_end)) = self.0.range(..=begin).next_back()
            && *earlier_end >= begin
        {
            bail!("overlapping yEnc part range {begin}-{end}");
        }
        if let Some((later_begin, _)) = self.0.range(begin..).next()
            && *later_begin <= end
        {
            bail!("overlapping yEnc part range {begin}-{end}");
        }
        self.0.insert(begin, end);
        Ok(())
    }

    /// The ranges of a file of `size` bytes that no article covered.
    pub(super) fn holes(&self, size: u64) -> Vec<(u64, u64)> {
        let mut holes = Vec::new();
        let mut next = 1_u64;
        for (begin, end) in &self.0 {
            if *begin > next {
                holes.push((next, begin - 1));
            }
            next = end.saturating_add(1);
        }
        if next <= size {
            holes.push((next, size));
        }
        holes
    }
}

/// Where this article belongs in the file.
///
/// A post without `=ypart` names no place, so it can only be a file of one article - which
/// is what the old contiguity check said in its own way.
fn decoded_part_range(decoded: &crate::DecodedArticle, single_segment: bool) -> Result<(u64, u64)> {
    match (decoded.metadata.part_begin, decoded.metadata.part_end) {
        (Some(begin), Some(end)) => Ok((begin, end)),
        (None, None) if single_segment => Ok((1, u64::try_from(decoded.data.len())?)),
        (None, None) => bail!("multiple yEnc segments require explicit part ranges"),
        _ => bail!("incomplete yEnc part range"),
    }
}

fn validate_metadata(
    decoded: &crate::DecodedArticle,
    name: &mut Option<String>,
    declared_size: &mut Option<u64>,
    file: &NzbFileStatus,
) -> Result<()> {
    // Before the first byte is written, so a header that announces more than the NZB lists
    // never sizes the file at all (RD-1101-16).
    crate::bounds::check_declared_size(decoded.metadata.declared_size, file)?;
    if declared_size.is_some_and(|value| value != decoded.metadata.declared_size) {
        bail!("yEnc segments disagree on the declared file size");
    }
    name.get_or_insert_with(|| decoded.metadata.name.clone());
    declared_size.get_or_insert(decoded.metadata.declared_size);
    Ok(())
}
