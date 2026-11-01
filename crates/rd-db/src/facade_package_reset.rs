//! Database facade for a full package reset (RD-1390-07): what the package produced, and the
//! reset of its rows.

use anyhow::Result;
use rd_core::{DownloadFile, PackageId};

use crate::{Database, PackageResetPlan, commands::PackagesCommand, writer};

impl Database {
    /// Records entries an unpack created in the package's folder, absolute paths; a path
    /// already recorded stays. Refused for a package that no longer exists.
    pub async fn record_package_outputs(&self, id: PackageId, paths: Vec<String>) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        writer::request(&self.writer, |reply| {
            PackagesCommand::RecordPackageOutputs { id, paths, reply }
        })
        .await
    }

    /// What a full reset of the package may delete beyond its downloads' own files: the
    /// entries its unpacks created, their folders, and its Usenet files' assembled outputs.
    pub async fn package_reset_plan(&self, id: PackageId) -> Result<PackageResetPlan> {
        crate::package_outputs_store::plan(&self.readers, id).await
    }

    /// Puts every file of the package back to `queued` and the package as the LinkGrabber
    /// handed it over, in one transaction; answers the reset rows.
    ///
    /// Refused as a whole - nothing written - when a file is still at work or is a Usenet
    /// file whose NZB was dropped. `forget_outputs` drops the record of what its unpacks
    /// created, for a reset that deleted those entries. The data on disk is the caller's to
    /// delete, before this.
    pub async fn reset_package(
        &self,
        id: PackageId,
        forget_outputs: bool,
    ) -> Result<Vec<DownloadFile>> {
        writer::request(&self.writer, |reply| PackagesCommand::ResetPackage {
            id,
            forget_outputs,
            reply,
        })
        .await
    }
}
