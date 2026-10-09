//! The NZBs an `.rdlinks` file carries, taken in like a dropped NZB (RD-1220-02).
//!
//! An exported indexer hit or Usenet download comes back as its NZB document, never as the
//! indexer's address: it becomes an NZB import exactly as a file dropped on the LinkGrabber
//! does — reviewed in the LinkGrabber, or queued at once when the import asked for that — with
//! the package's name, password and category from the file. No indexer, no API key, no hoster
//! candidate, so nothing is left "not resolvable", on this installation or another.

use rd_collector::{LinksDocument, NzbDocument};
use rd_db::{NewNzbFile, NewNzbImport, NewNzbSegment};

use crate::{ApiError, dlc_import::DlcImportOptions};

/// Where the NZBs of a link file land.
#[derive(Clone, Copy)]
pub enum NzbLanding<'a> {
    /// In the LinkGrabber's NZB list, for review: what a dropped NZB does.
    Review,
    /// Straight into the download list, the queue's destination found by the scheduler.
    Enqueue(&'a rd_scheduler::SchedulerHandle),
}

/// One NZB of the file, parsed and ready to store.
pub(crate) struct EmbeddedNzb {
    name: String,
    password: Option<String>,
    category_id: Option<rd_core::CategoryId>,
    sha256: String,
    document: NzbDocument,
}

/// Parses every NZB of `document`, so a broken one refuses the whole file before anything is
/// created. `category_of` finds a package's category by the name the file gives it.
pub(crate) fn parse_embedded(
    document: &LinksDocument,
    category_of: impl Fn(Option<&str>) -> Option<rd_core::CategoryId>,
) -> Result<Vec<EmbeddedNzb>, ApiError> {
    let mut parsed = Vec::new();
    for package in &document.packages {
        for nzb in &package.nzbs {
            let content = nzb.content.as_bytes();
            let document = rd_collector::parse_nzb(content).map_err(|error| {
                ApiError::bad_request(
                    "rdlinks.nzb_invalid",
                    "An NZB in the link file is not valid",
                )
                .with_param("name", nzb.name.chars().take(120).collect::<String>())
                .with_param(
                    "detail",
                    error.to_string().chars().take(200).collect::<String>(),
                )
            })?;
            // The NZB's own password is the one its package would have taken (the export put it
            // there); the package's comes next, as for a dropped NZB with a marker.
            let password = document
                .password
                .clone()
                .or_else(|| package.password.clone())
                .filter(|password| !password.is_empty());
            parsed.push(EmbeddedNzb {
                name: rd_files::sanitize_file_name(nzb.name.trim()),
                password,
                category_id: category_of(package.category.as_deref()),
                sha256: crate::input_checks::sha256_hex(content),
                document,
            });
        }
    }
    Ok(parsed)
}

/// Stores the parsed NZBs as imports and, for [`NzbLanding::Enqueue`], queues each new one.
///
/// A file whose NZB is already here answers with the import it has, as a second drop of the
/// same NZB does, and queues nothing a second time.
pub(crate) async fn store_embedded(
    database: &rd_db::Database,
    nzbs: Vec<EmbeddedNzb>,
    options: &DlcImportOptions,
    landing: NzbLanding<'_>,
) -> Result<Vec<rd_core::NzbImport>, ApiError> {
    let mut imports = Vec::with_capacity(nzbs.len());
    for nzb in nzbs {
        let import = database
            .add_nzb_import(NewNzbImport {
                name: nzb.name,
                sha256: nzb.sha256,
                category_id: nzb.category_id,
                source: options.source,
                priority: options.priority,
                import_mode: match landing {
                    NzbLanding::Review => rd_core::ImportMode::Review,
                    NzbLanding::Enqueue(_) => rd_core::ImportMode::Enqueue,
                },
                source_path: None,
                password: nzb.password,
                // Somebody handed the file over, as with a dropped NZB.
                announce_arrival: true,
                files: files_of(nzb.document),
            })
            .await?;
        let import = match landing {
            NzbLanding::Enqueue(scheduler) if !import.duplicate => {
                let destination =
                    crate::destination::intake_destination(database, scheduler, import.category_id)
                        .await?;
                database
                    .enqueue_nzb_import(
                        import.id,
                        destination,
                        options
                            .priority
                            .unwrap_or(rd_core::DownloadPriority::Normal),
                        false,
                    )
                    .await?;
                database.get_nzb_import(import.id).await?.unwrap_or(import)
            }
            _ => import,
        };
        imports.push(import);
    }
    Ok(imports)
}

fn files_of(document: NzbDocument) -> Vec<NewNzbFile> {
    document
        .files
        .into_iter()
        .map(|file| NewNzbFile {
            subject: file.subject,
            poster: file.poster,
            groups: file.groups,
            segments: file
                .segments
                .into_iter()
                .map(|segment| NewNzbSegment {
                    number: segment.number,
                    bytes: segment.bytes,
                    message_id: segment.message_id,
                })
                .collect(),
        })
        .collect()
}
