//! MCP tools for handing in a container file (RD-120-31).
//!
//! RD-120-29 measured why these were missing: every import route took `multipart/form-data`,
//! and a tool call carries JSON and no file. The routes now take a JSON body as well, with the
//! file as base64, and each tool here calls its route's handler with that body built by hand —
//! the same handler, the same checks, the same codes, and through `TOOL_POLICY` the same
//! price. One tool per route rather than one tool guessing the route from a file name: the
//! three answer with three different things (LinkGrabber packages, a torrent's LinkGrabber
//! batch, an NZB import waiting for review), and a dispatcher would be a second place deciding
//! which intake a file belongs to.

use axum::extract::State;
use rmcp::{handler::server::wrapper::Parameters, schemars, tool, tool_router};
use serde::Deserialize;

use super::{
    RdMcpServer,
    error::{McpToolResult, respond},
};
use crate::container_upload::{ContainerUpload, UploadBody};

/// A container file, as the JSON body of an import route spells it.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ContainerFileParams {
    /// The file's bytes as base64 (standard alphabet; padding optional, line breaks ignored).
    /// At most 48 MiB once decoded.
    pub content: String,
    /// The file's name. import_container reads the format from its extension; a
    /// `{{password}}` marker in it names the archive password.
    #[serde(default)]
    pub file_name: Option<String>,
    /// The package name to use instead of the one the file suggests.
    #[serde(default)]
    pub name: Option<String>,
    /// The category to file the result under.
    #[serde(default)]
    pub category_id: Option<String>,
    /// `low`, `normal` or `high`.
    #[serde(default)]
    pub priority: Option<String>,
}

/// The same, plus the format override only the generic container route reads.
#[derive(Deserialize, schemars::JsonSchema)]
pub(crate) struct ImportContainerParams {
    #[serde(flatten)]
    pub file: ContainerFileParams,
    /// `dlc`, `ccf`, `rsdf` or `txt`, overriding the file name's extension.
    #[serde(default)]
    pub format: Option<String>,
}

impl ContainerFileParams {
    fn into_body(self, format: Option<String>) -> UploadBody {
        UploadBody::Json(ContainerUpload {
            content: self.content,
            file_name: self.file_name,
            name: self.name,
            category_id: self.category_id,
            priority: self.priority,
            format,
        })
    }
}

#[tool_router(router = containers_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Hand a link container to the LinkGrabber: a .dlc, .ccf, .rsdf or a plain .txt link list, as base64. The format comes from file_name's extension or from `format`. A DLC or CCF is opened by the online decryption service, which has to be switched on in the settings. Answers with the LinkGrabber packages and links it produced; enqueue them with enqueue_collector."
    )]
    pub async fn import_container(
        &self,
        Parameters(params): Parameters<ImportContainerParams>,
    ) -> McpToolResult {
        let body = params.file.into_body(params.format);
        respond(
            crate::container_handlers::import_container(State(self.state.clone()), body)
                .await
                .map(|(_, answer)| answer.0),
        )
    }

    #[tool(
        description = "Hand a .torrent file to the LinkGrabber, as base64 (at most 16 MiB). Answers with the LinkGrabber batch, package and candidate it produced; enqueue it with enqueue_collector."
    )]
    pub async fn import_torrent(
        &self,
        Parameters(params): Parameters<ContainerFileParams>,
    ) -> McpToolResult {
        respond(
            crate::torrent_handlers::import_torrent(
                State(self.state.clone()),
                params.into_body(None),
            )
            .await
            .map(|(_, answer)| answer.0),
        )
    }

    #[tool(
        description = "Hand an .nzb file in, as base64. It lands as an NZB import waiting for review, exactly as a browser upload does; queueing it is done in the NZB import screen. Answers with the import row."
    )]
    pub async fn import_nzb(
        &self,
        Parameters(params): Parameters<ContainerFileParams>,
    ) -> McpToolResult {
        respond(
            crate::handlers::import_nzb(State(self.state.clone()), params.into_body(None))
                .await
                .map(|(_, answer)| answer.0),
        )
    }
}
