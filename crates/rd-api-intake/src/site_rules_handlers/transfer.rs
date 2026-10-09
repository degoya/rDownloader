//! Export and import of the user's own site rules.

use super::*;

#[utoipa::path(
    get,
    path = "/api/v1/site-rules/export",
    tag = "configuration",
    responses((status = 200, body = SiteRuleDocument))
)]
pub async fn export_site_rules(
    State(state): State<AppState>,
) -> Result<Json<SiteRuleDocument>, ApiError> {
    Ok(Json(SiteRuleDocument {
        format_version: DOCUMENT_VERSION,
        rules: state
            .database
            .list_site_rules()
            .await?
            .into_iter()
            .map(|stored| stored.rule)
            .collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/site-rules/import",
    tag = "configuration",
    request_body(content = SiteRuleImportRequest, content_type = "application/json"),
    responses((status = 200, body = ImportSiteRulesResponse))
)]
pub async fn import_site_rules(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Result<Json<ImportSiteRulesResponse>, ApiError> {
    rd_api_core::input_checks::require_media_type(&headers, "application/json")?;
    let (bodies, signature) = import_bodies(&body)?;
    let origin = match &signature {
        Some(signature) => admit(&state, signature).await?,
        None => SiteRuleOrigin::unsigned(SiteRuleOriginKind::Import),
    };
    let mut existing: Vec<String> = state
        .database
        .list_site_rules()
        .await?
        .into_iter()
        .map(|stored| stored.id)
        .collect();
    let mut results = Vec::with_capacity(bodies.len());
    let mut stored_count = 0usize;
    for body in &bodies {
        let refused = |id: String, name: String, code: &str| ImportedSiteRuleResponse {
            id,
            name,
            status: "refused".to_owned(),
            code: Some(code.to_owned()),
        };
        let rule = match parse_rule(body) {
            Ok(rule) => rule,
            Err(error) => {
                let id = body
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let name = body
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                results.push(refused(id, name, error.code()));
                continue;
            }
        };
        // A second import of the release file meets the rules the first one stored, perhaps
        // edited since: the stored rule stays and the file's is reported, never written over it.
        if existing.iter().any(|id| id == &rule.id) {
            results.push(refused(
                rule.id.clone(),
                rule.name.clone(),
                "site_rules.duplicate_id",
            ));
            continue;
        }
        // Switched off, without asking and without an option to ask otherwise: nothing a file
        // brings starts active here, signed or not.
        store(&state, &rule, false, origin.clone()).await?;
        existing.push(rule.id.clone());
        stored_count += 1;
        results.push(ImportedSiteRuleResponse {
            id: rule.id.clone(),
            name: rule.name.clone(),
            status: "stored".to_owned(),
            code: None,
        });
    }
    Ok(Json(ImportSiteRulesResponse {
        rules: results,
        stored: stored_count,
        signed: signature.is_some(),
    }))
}

/// Who signed an imported file and its sequence (RD-1200-05).
pub(super) struct Signature {
    signer: String,
    sequence: u64,
}

/// Measures a signed file's sequence against the highest one accepted from its signer and
/// records it, or refuses an older file whole with `site_rules.sequence_older` -- before a
/// single rule of it is stored. The same sequence again passes: a second import of one file
/// restores a rule deleted since, and its rules already stored are reported as duplicates.
async fn admit(state: &AppState, signature: &Signature) -> Result<SiteRuleOrigin, ApiError> {
    let known = state
        .database
        .record_site_rule_pack(&signature.signer, signature.sequence)
        .await?;
    rd_siterules::admit_sequence(signature.sequence, known).map_err(|error| {
        ApiError::conflict(error.code(), error.to_string())
            .with_param("signer", &signature.signer)
            .with_param("sequence", signature.sequence)
            .with_param("known", known.unwrap_or_default())
    })?;
    Ok(SiteRuleOrigin::signed(
        &signature.signer,
        signature.sequence,
    ))
}

/// The rule bodies an imported file carries, and who signed them when they came under a
/// signature that held (RD-130-07, RD-1200-05).
///
/// A file with `signatures` is the signed release file and is verified as one, against the
/// compiled-in site-rules root and from the bytes exactly as they arrived -- the signature
/// covers those bytes, not a parsed copy of them. One that does not verify is refused whole,
/// with the pack's own code, and is never read a second time as an unsigned export: a release
/// file that fails its signature is a damaged or altered one, not somebody's own rules.
pub(super) fn import_bodies(
    bytes: &[u8],
) -> Result<(Vec<serde_json::Value>, Option<Signature>), ApiError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| ApiError::bad_request("site_rules.malformed", error.to_string()))?;
    if value.get("signatures").is_some() {
        let rd_siterules::SignedPack { pack, signer } =
            rd_siterules::verify_signed(bytes, chrono::Utc::now())
                .map_err(|error| ApiError::bad_request(error.code(), error.to_string()))?;
        let bodies = pack
            .rules
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ApiError::bad_request("site_rules.invalid_rule", error.to_string()))?;
        return Ok((
            bodies,
            Some(Signature {
                signer,
                sequence: pack.sequence,
            }),
        ));
    }
    let document: SiteRuleDocument = serde_json::from_value(value)
        .map_err(|error| ApiError::bad_request("site_rules.malformed", error.to_string()))?;
    if document.format_version != DOCUMENT_VERSION {
        return Err(ApiError::bad_request(
            "site_rules.format_version_unsupported",
            "This build does not read that rule format",
        )
        .with_param("format_version", document.format_version));
    }
    Ok((document.rules, None))
}
