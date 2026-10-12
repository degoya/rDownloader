//! The files the desktop hands the agent besides an NZB (RD-1260-09, `crate::hand_over`): a link
//! container and a torrent.

use anyhow::Result;
use reqwest::multipart;

use super::{CaptureClient, ensure_success};

impl CaptureClient {
    /// A link container or a torrent, to the route the browser extension hands its files to
    /// (RD-130-16): it takes a `.dlc`, `.ccf`, `.rsdf`, `.rdlinks` or `.crawljob` by its name,
    /// through the LinkGrabber's container import, and a torrent by its content.
    pub(crate) async fn upload_file(&self, name: String, content: Vec<u8>) -> Result<()> {
        let form = multipart::Form::new().part(
            "file",
            multipart::Part::bytes(content)
                .file_name(name)
                .mime_str("application/octet-stream")?,
        );
        let endpoint = self.service.join("api/v1/capture/file")?;
        let response = self.send(self.http.post(endpoint).multipart(form)).await?;
        ensure_success(response, "file import").await?;
        Ok(())
    }
}
