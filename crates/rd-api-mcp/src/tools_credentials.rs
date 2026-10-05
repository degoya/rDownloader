//! MCP tools for the credential-bearing configuration: accounts, proxies and NNTP servers.
//!
//! ## No secret passes through here
//!
//! None of these tools takes or returns a password, an API key, a cookie jar or a vault
//! reference. A row is created, renamed, re-pointed, switched off and deleted from here; the
//! credential itself is typed into the web UI, which is the one surface where it is entered
//! deliberately and never travels through a model's context. Update tools merge onto the
//! stored row, so an edit that does not mention the credential keeps it.

use axum::{
    Json,
    extract::{Path as AxumPath, State},
};
use rmcp::{handler::server::wrapper::Parameters, tool, tool_router};

use super::{
    RdMcpServer,
    error::{McpToolResult, parse_id, respond},
    params_config::{
        CreateAccountParams, CreateProxyProfileParams, CreateUsenetServerParams, IdParams,
        SetUsenetQuotaParams, UpdateAccountParams, UpdateProxyProfileParams,
        UpdateUsenetServerParams, clearing, merged,
    },
};
use crate::{
    ApiError,
    dto::{
        CreateAccountRequest, CreateProxyProfileRequest, CreateUsenetServerRequest,
        SetUsenetQuotaRequest, UpdateAccountRequest, UpdateUsenetServerRequest,
    },
};

#[tool_router(router = credentials_router, vis = "pub(crate)")]
impl RdMcpServer {
    #[tool(
        description = "Create a provider account. Metadata only: the password or API key is entered in the web UI afterwards, and no tool accepts one."
    )]
    pub async fn create_account(
        &self,
        Parameters(params): Parameters<CreateAccountParams>,
    ) -> McpToolResult {
        let result = async {
            let request = CreateAccountRequest {
                provider: params.provider,
                label: params.label,
                username: params.username,
                credential_mode: params.credential_mode.map(Into::into),
                secret: None,
                cookies: None,
                proxy_profile_id: params
                    .proxy_profile_id
                    .as_deref()
                    .map(parse_id)
                    .transpose()?,
                enabled: params.enabled.unwrap_or(true),
            };
            Ok(
                crate::config_handlers::create_account(State(self.state.clone()), Json(request))
                    .await?
                    .1
                    .0,
            )
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change a provider account's metadata. The stored credential is kept untouched; only the fields you pass are changed."
    )]
    pub async fn update_account(
        &self,
        Parameters(params): Parameters<UpdateAccountParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::AccountId = parse_id(&params.id)?;
            let current = self
                .state
                .database
                .list_accounts()
                .await?
                .into_iter()
                .find(|account| account.id == id)
                .ok_or_else(crate::error_codes::account_not_found)?;
            let cleared = clearing(
                params.clear.as_ref(),
                &["username", "credential_mode", "proxy_profile_id"],
            )?;
            let request = UpdateAccountRequest {
                provider: params.provider.unwrap_or(current.provider),
                label: params.label.unwrap_or(current.label),
                username: merged(&cleared, "username", params.username, current.username),
                credential_mode: merged(
                    &cleared,
                    "credential_mode",
                    params.credential_mode.map(Into::into),
                    current.credential_mode,
                ),
                secret: None,
                cookies: None,
                clear_secret: false,
                clear_cookies: false,
                proxy_profile_id: merged(
                    &cleared,
                    "proxy_profile_id",
                    params
                        .proxy_profile_id
                        .as_deref()
                        .map(parse_id)
                        .transpose()?,
                    current.proxy_profile_id,
                ),
                enabled: params.enabled.unwrap_or(current.enabled),
            };
            Ok(crate::config_handlers::update_account(
                State(self.state.clone()),
                AxumPath(id),
                Json(request),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Delete one provider account and its stored credential. Destructive and refused while unfinished downloads still use it."
    )]
    pub async fn delete_account(&self, Parameters(params): Parameters<IdParams>) -> McpToolResult {
        let result = async {
            let id: rd_core::AccountId = parse_id(&params.id)?;
            Ok(
                crate::config_handlers::delete_account(State(self.state.clone()), AxumPath(id))
                    .await?
                    .0,
            )
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Create a proxy profile from a credential-free URL such as socks5://10.0.0.2:1080. A proxy that needs a login is set up in the web UI."
    )]
    pub async fn create_proxy_profile(
        &self,
        Parameters(params): Parameters<CreateProxyProfileParams>,
    ) -> McpToolResult {
        let request = CreateProxyProfileRequest {
            name: params.name,
            kind: params.kind.into(),
            endpoint: params.endpoint,
            username: None,
            password: None,
        };
        respond(
            crate::config_handlers::create_proxy_profile(State(self.state.clone()), Json(request))
                .await
                .map(|created| created.1.0),
        )
    }

    #[tool(
        description = "Change one proxy profile's name, type or address. A stored login is kept; only the fields you pass are changed."
    )]
    pub async fn update_proxy_profile(
        &self,
        Parameters(params): Parameters<UpdateProxyProfileParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::ProxyProfileId = parse_id(&params.id)?;
            let current = self
                .state
                .database
                .proxy_profile(id)
                .await?
                .ok_or_else(|| ApiError::not_found("proxy.not_found", "Proxy profile not found"))?;
            let request = CreateProxyProfileRequest {
                name: params.name.unwrap_or(current.name),
                kind: params.kind.map_or(current.kind, Into::into),
                endpoint: params
                    .endpoint
                    .unwrap_or_else(|| current.endpoint.to_string()),
                // Carried over so the stored password keeps a username to pair with; the
                // password itself never leaves the vault.
                username: current.username,
                password: None,
            };
            Ok(crate::config_handlers::update_proxy_profile(
                State(self.state.clone()),
                AxumPath(id),
                Json(request),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Delete one proxy profile. Destructive and refused while an account, an NNTP server, an unfinished download or the global proxy setting still points at it."
    )]
    pub async fn delete_proxy_profile(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::ProxyProfileId = parse_id(&params.id)?;
            Ok(crate::config_handlers::delete_proxy_profile(
                State(self.state.clone()),
                AxumPath(id),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "List the configured NNTP servers with their priority, connection limits and traffic quota (limit, action, used bytes, whether it is used up). Passwords are never included."
    )]
    pub async fn list_usenet_servers(&self) -> McpToolResult {
        respond(
            crate::usenet_handlers::list_usenet_servers(State(self.state.clone()))
                .await
                .map(|servers| servers.0),
        )
    }

    #[tool(
        description = "Create an NNTP server entry. Metadata only: a server that needs a login is completed in the web UI, because no tool accepts a password. Names are unique: a name another server has is refused with usenet.server_name_taken."
    )]
    pub async fn create_usenet_server(
        &self,
        Parameters(params): Parameters<CreateUsenetServerParams>,
    ) -> McpToolResult {
        let result = async {
            let request = CreateUsenetServerRequest {
                name: params.name,
                host: params.host,
                port: params.port,
                tls: params.tls.unwrap_or(true),
                username: params.username,
                password: None,
                proxy_profile_id: params
                    .proxy_profile_id
                    .as_deref()
                    .map(parse_id)
                    .transpose()?,
                priority: params.priority.unwrap_or(0),
                max_connections: params.max_connections.unwrap_or(8),
                enabled: params.enabled.unwrap_or(true),
            };
            Ok(crate::usenet_handlers::create_usenet_server(
                State(self.state.clone()),
                Json(request),
            )
            .await?
            .1
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Change one NNTP server. The stored password is kept; only the fields you pass are changed. A name another server has is refused with usenet.server_name_taken."
    )]
    pub async fn update_usenet_server(
        &self,
        Parameters(params): Parameters<UpdateUsenetServerParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::UsenetServerId = parse_id(&params.id)?;
            let current = self
                .state
                .database
                .list_usenet_servers()
                .await?
                .into_iter()
                .find(|server| server.id == id)
                .ok_or_else(crate::error_codes::usenet_server_not_found)?;
            let cleared = clearing(params.clear.as_ref(), &["username", "proxy_profile_id"])?;
            let drops_username = cleared.iter().any(|name| name == "username");
            let request = UpdateUsenetServerRequest {
                name: params.name.unwrap_or(current.name),
                host: params.host.unwrap_or(current.host),
                port: params.port.unwrap_or(current.port),
                tls: params.tls.unwrap_or(current.tls),
                username: merged(&cleared, "username", params.username, current.username),
                password: None,
                // Dropping the username without dropping the password would leave a pairing the
                // REST handler refuses, so the two go together.
                clear_password: drops_username,
                proxy_profile_id: merged(
                    &cleared,
                    "proxy_profile_id",
                    params
                        .proxy_profile_id
                        .as_deref()
                        .map(parse_id)
                        .transpose()?,
                    current.proxy_profile_id,
                ),
                priority: params.priority.unwrap_or(current.priority),
                max_connections: params.max_connections.unwrap_or(current.max_connections),
                enabled: params.enabled.unwrap_or(current.enabled),
            };
            Ok(crate::usenet_handlers::update_usenet_server(
                State(self.state.clone()),
                AxumPath(id),
                Json(request),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Delete one NNTP server and its stored password. Destructive; Usenet downloads then fall back to the remaining servers."
    )]
    pub async fn delete_usenet_server(
        &self,
        Parameters(params): Parameters<IdParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::UsenetServerId = parse_id(&params.id)?;
            Ok(crate::usenet_handlers::delete_usenet_server(
                State(self.state.clone()),
                AxumPath(id),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }

    #[tool(
        description = "Set, change or remove the traffic quota of one NNTP server, such as a block account's volume. Once the server has delivered limit_bytes, action applies from the next file: backup (asked only after every other server) or pause (not asked at all); a notification (usenet_quota_reached) goes out. Merges onto the stored quota: only the fields you pass change. reset_on (YYYY-MM-DD) starts the used figure again at zero on that day, once; reset_usage does it now; clear [\"limit_bytes\"] removes the quota."
    )]
    pub async fn set_usenet_server_quota(
        &self,
        Parameters(params): Parameters<SetUsenetQuotaParams>,
    ) -> McpToolResult {
        let result = async {
            let id: rd_core::UsenetServerId = parse_id(&params.id)?;
            let current = self
                .state
                .database
                .list_usenet_servers()
                .await?
                .into_iter()
                .find(|server| server.id == id)
                .ok_or_else(crate::error_codes::usenet_server_not_found)?
                .quota;
            let cleared = clearing(params.clear.as_ref(), &["limit_bytes", "reset_on"])?;
            let reset_on = params
                .reset_on
                .as_deref()
                .map(|day| {
                    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|_| {
                        ApiError::bad_request(
                            "usenet.quota_invalid",
                            "reset_on must be a day written YYYY-MM-DD",
                        )
                    })
                })
                .transpose()?;
            let request = SetUsenetQuotaRequest {
                limit_bytes: merged(
                    &cleared,
                    "limit_bytes",
                    params.limit_bytes,
                    current.as_ref().map(|quota| quota.limit_bytes),
                ),
                action: params.action.map_or_else(
                    || {
                        current
                            .as_ref()
                            .map(|quota| quota.action)
                            .unwrap_or_default()
                    },
                    Into::into,
                ),
                reset_on: merged(
                    &cleared,
                    "reset_on",
                    reset_on,
                    current.as_ref().and_then(|quota| quota.reset_on),
                ),
                reset_usage: params.reset_usage.unwrap_or(false),
            };
            Ok(crate::usenet_handlers::set_usenet_server_quota(
                State(self.state.clone()),
                AxumPath(id),
                Json(request),
            )
            .await?
            .0)
        }
        .await;
        respond(result)
    }
}
