use std::path::Path;

use anyhow::Result;
use rd_core::{NzbFileStatus, NzbSegmentState, NzbSegmentStatus};
use rd_db::Database;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
};

pub(crate) struct ResumeState {
    pub output: File,
    /// The ranges the database calls complete and the bytes on disk agree with, 1-based and
    /// inclusive. Not necessarily a prefix: articles are written where they belong, so a
    /// file may hold part seven while part three is still missing (RD-108-26).
    pub written: Vec<(u64, u64)>,
    pub name: Option<String>,
    pub declared_size: Option<u64>,
    pub remaining: Vec<NzbSegmentStatus>,
}

/// What of a half-finished `.part` file may be kept, and what has to be fetched again.
///
/// Every segment the database calls complete is checked on its own: its range has to lie
/// inside the file and inside the announced size, and the bytes there have to hash to the
/// checksum the checkpoint recorded. What passes is kept where it is, what does not goes back
/// into the queue. Nothing is truncated - a range that failed will be written again at the
/// same offset, and a range whose article turns out to be gone is zero-filled when the file
/// is finished.
///
/// That per-range check is what lets RD-108-25 leave out the `fdatasync` per article: bytes
/// the kernel had not flushed when the power went fail their checksum here and cost their own
/// segment, never the file.
pub(crate) async fn prepare(
    database: &Database,
    file: &NzbFileStatus,
    part_path: &Path,
) -> Result<ResumeState> {
    let mut output = tokio::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(part_path)
        .await?;
    let disk_length = output.metadata().await?.len();
    let mut segments = file.segments.clone();
    segments.sort_by_key(|segment| segment.number);
    let declared_size = file.declared_size.map(rd_core::ByteCount::get);
    let metadata_complete = file.assembly_name.is_some() && declared_size.is_some();
    let mut written = Vec::new();
    let mut remaining = Vec::new();
    for segment in segments {
        let complete = segment.state == NzbSegmentState::Completed;
        let proven = if metadata_complete && complete {
            proven_range(&mut output, &segment, disk_length, declared_size).await?
        } else {
            None
        };
        if let Some(range) = proven {
            written.push(range);
            continue;
        }
        if complete {
            database
                .set_nzb_segment_state(segment.id, NzbSegmentState::Queued, None)
                .await?;
        }
        remaining.push(segment);
    }
    let resumed = !written.is_empty();
    Ok(ResumeState {
        output,
        written,
        name: resumed.then(|| file.assembly_name.clone()).flatten(),
        declared_size: resumed.then_some(declared_size).flatten(),
        remaining,
    })
}

/// The segment's range, if the checkpoint describes one the file on disk actually holds.
async fn proven_range(
    output: &mut File,
    segment: &NzbSegmentStatus,
    disk_length: u64,
    declared_size: Option<u64>,
) -> Result<Option<(u64, u64)>> {
    let Some((begin, end, expected_crc)) = checkpoint_range(segment) else {
        return Ok(None);
    };
    if begin == 0
        || end < begin
        || end > disk_length
        || declared_size.is_some_and(|size| end > size)
    {
        return Ok(None);
    }
    let crc = crc_range(output, begin - 1, end - begin + 1).await?;
    Ok((crc == expected_crc).then_some((begin, end)))
}

pub(crate) async fn validates_completed_file(file: &NzbFileStatus, path: &Path) -> Result<bool> {
    let (Some(_), Some(declared_size)) = (&file.assembly_name, file.declared_size) else {
        return Ok(false);
    };
    let mut input = match File::open(path).await {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if input.metadata().await?.len() != declared_size.get() {
        return Ok(false);
    }
    let mut segments = file.segments.iter().collect::<Vec<_>>();
    segments.sort_by_key(|segment| segment.number);
    let mut written = 0_u64;
    for segment in segments {
        if segment.state != NzbSegmentState::Completed {
            return Ok(false);
        }
        let Some((begin, end, expected_crc)) = checkpoint_range(segment) else {
            return Ok(false);
        };
        if begin != written.saturating_add(1) || end < begin || end > declared_size.get() {
            return Ok(false);
        }
        if crc_range(&mut input, begin - 1, end - begin + 1).await? != expected_crc {
            return Ok(false);
        }
        written = end;
    }
    Ok(written == declared_size.get())
}

fn checkpoint_range(segment: &NzbSegmentStatus) -> Option<(u64, u64, u32)> {
    Some((
        segment.part_begin?.get(),
        segment.part_end?.get(),
        u32::from_str_radix(segment.crc32.as_deref()?, 16).ok()?,
    ))
}

async fn crc_range(file: &mut File, offset: u64, length: u64) -> Result<u32> {
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut remaining = length;
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut hasher = crc32fast::Hasher::new();
    while remaining > 0 {
        let wanted = usize::try_from(remaining.min(buffer.len() as u64))?;
        file.read_exact(&mut buffer[..wanted]).await?;
        hasher.update(&buffer[..wanted]);
        remaining -= wanted as u64;
    }
    Ok(hasher.finalize())
}
