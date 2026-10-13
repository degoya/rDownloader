//! One behavioural question per plugin type beyond resolver and crawler (RD-1280-19).
//!
//! Instantiating a component proves its exports line up with its world; it says nothing about
//! what the plugin answers. Each question here asks the built component one thing a plugin of
//! its type must get right without a network -- the conformance run has none, every request
//! meets [`super::RefusingHost`] -- and that no real input can stand in for later: a notifier
//! that reports a delivery it never made, a sign-in that authorizes nobody, a post-processing
//! step that fails a package with nothing in it.
//!
//! Every question splits in two: the call into the component ([`ask`]) and a judge over its
//! answer (`judge_*`). The judges are what the negative cases in `questions_tests.rs` feed a
//! wrong answer, the one a deliberately broken component would give. A trap is never a
//! refusal: a plugin that crashes on an input is wrong about it, whatever it meant to say.
//!
//! The `id` of each question is its stable code, the one a CI job acts on; the wiki's
//! conformance page lists them.

use std::sync::Arc;

use rd_core::Failure;
use rd_plugin_api::ResolverHost;

use crate::{
    PluginManifest, PluginType, RemoteFile, TransferBackend, TransferState, TransferTarget,
    extension::{
        AuthProgress, AuthProvider, Delivery, IntakeParser, IntakeProposal, MetadataEnricher,
        NotifierPlugin, PostprocessPlugin, RemoteJobPlugin, RemoteJobSource, SourceState,
        StepOutcome, StoragePlugin, StreamTransformProvider,
    },
};

/// Text in no format any intake parser reads: no address, no markup, no key.
pub(super) const UNKNOWN_INPUT: &str =
    "rDownloader conformance check: this text is no file any intake plugin reads.\n";

/// A scheme no transfer backend declares.
pub(super) const UNDECLARED_SCHEME: &str = "conformance-undeclared";

/// An address on a reserved name (RFC 6761) the storage destination never uploaded to.
pub(super) const UNREACHED_OBJECT: &str = "https://conformance.invalid/conformance.bin";

/// A minimal `.torrent`: one bencoded info dictionary.
pub(super) const TORRENT: &[u8] = b"d4:infod6:lengthi1e4:name15:conformance.bin12:piece lengthi16384e6:pieces20:aaaaaaaaaaaaaaaaaaaaee";

/// A minimal `.nzb` document.
pub(super) const NZB: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<nzb xmlns=\"http://www.newzbin.com/DTD/2003/nzb\"><file poster=\"conformance\" date=\"0\" subject=\"conformance.bin\"><groups><group>alt.binaries.test</group></groups><segments><segment bytes=\"1\" number=\"1\">conformance@example.invalid</segment></segments></file></nzb>\n";

/// The container formats [`judge_containers`] can hand a plugin a sample of. `dlc` and `rsdf`
/// are encrypted lists with no shape a sample could have.
pub(super) const CONTAINER_SAMPLES: [(&str, &[u8]); 2] = [("torrent", TORRENT), ("nzb", NZB)];

/// The failure codes the host gives a trap (`component::describe_component_error`).
const TRAP_CODES: [&str; 4] = [
    "plugin.trapped",
    "plugin.fuel_exhausted",
    "plugin.timeout",
    "plugin.execution_failed",
];

/// One answered question.
pub(super) struct Answer {
    pub(super) id: &'static str,
    pub(super) about: &'static str,
    pub(super) result: Result<(), String>,
}

/// A built plugin of a type that has a question.
pub(super) enum Typed {
    Transfer(Box<TransferBackend>),
    Notifier(Box<NotifierPlugin>),
    Auth(Box<AuthProvider>),
    Intake(Box<IntakeParser>),
    Enricher(Box<MetadataEnricher>),
    Postprocess(Box<PostprocessPlugin>),
    Storage(Box<StoragePlugin>),
    RemoteJob(Box<RemoteJobPlugin>),
    StreamTransform(Box<StreamTransformProvider>),
}

/// Builds the package as the core would, for every type with a question; `None` for the
/// others (resolver and crawler have theirs in `conformance.rs`, OAuth has none yet).
pub(super) fn build(
    manifest: &PluginManifest,
    component: &[u8],
    host: Arc<dyn ResolverHost>,
) -> Option<anyhow::Result<Typed>> {
    let manifest = manifest.clone();
    let host = Some(host);
    let plugin_type = manifest.plugin_type.clone();
    Some(match plugin_type {
        PluginType::Transfer => TransferBackend::new(manifest, component)
            .map(|plugin| Typed::Transfer(Box::new(plugin))),
        PluginType::Notifier => NotifierPlugin::new(manifest, component, host)
            .map(|plugin| Typed::Notifier(Box::new(plugin))),
        PluginType::Auth => {
            AuthProvider::new(manifest, component, host).map(|plugin| Typed::Auth(Box::new(plugin)))
        }
        PluginType::Intake => IntakeParser::new(manifest, component, host)
            .map(|plugin| Typed::Intake(Box::new(plugin))),
        PluginType::Enricher => MetadataEnricher::new(manifest, component, host)
            .map(|plugin| Typed::Enricher(Box::new(plugin))),
        PluginType::Postprocess => PostprocessPlugin::new(manifest, component, host)
            .map(|plugin| Typed::Postprocess(Box::new(plugin))),
        PluginType::Storage => StoragePlugin::new(manifest, component, host)
            .map(|plugin| Typed::Storage(Box::new(plugin))),
        PluginType::RemoteJob => RemoteJobPlugin::new(manifest, component, host)
            .map(|plugin| Typed::RemoteJob(Box::new(plugin))),
        PluginType::StreamTransform => StreamTransformProvider::new(manifest, component, host)
            .map(|plugin| Typed::StreamTransform(Box::new(plugin))),
        _ => return None,
    })
}

/// Asks the built plugin its type's question.
pub(super) async fn ask(typed: &Typed, manifest: &PluginManifest) -> Answer {
    match typed {
        Typed::Transfer(plugin) => Answer {
            id: "transfer_undeclared_scheme",
            about: "The transfer backend refuses an address in a scheme its manifest does not \
                    declare",
            result: match probe_undeclared(plugin, manifest).await {
                Ok(outcome) => judge_undeclared_scheme(outcome),
                Err(error) => Err(format!("the probe could not be set up: {error:#}")),
            },
        },
        Typed::Notifier(plugin) => Answer {
            id: "notifier_empty_destination",
            about: "The notifier refuses a delivery to an empty destination instead of \
                    reporting it delivered",
            result: judge_refusal(plugin.deliver(empty_delivery()).await),
        },
        Typed::Auth(plugin) => Answer {
            id: "auth_no_credentials",
            about: "A sign-in begun without credentials authorizes no account",
            result: judge_no_credentials(plugin.begin(rd_core::AccountId::new(), None).await),
        },
        Typed::Intake(plugin) => Answer {
            id: "intake_unknown_input",
            about: "The intake parser proposes nothing for input in no format it reads",
            result: judge_unknown_input(plugin.parse(UNKNOWN_INPUT).await),
        },
        Typed::Enricher(plugin) => Answer {
            id: "enricher_empty_input",
            about: "The enricher answers an empty link with no fields rather than a failure",
            result: judge_no_fields(plugin.enrich("", None, None).await),
        },
        Typed::Postprocess(plugin) => Answer {
            id: "postprocess_empty_input",
            about: "The post-processing step finishes a package without files, neither \
                    failing nor warning",
            result: judge_empty_package(
                plugin
                    .run(
                        SourceState::new(
                            "conformance".to_owned(),
                            std::env::temp_dir(),
                            Vec::new(),
                        ),
                        Vec::new(),
                        None,
                    )
                    .await,
            ),
        },
        Typed::Storage(plugin) => Answer {
            id: "storage_unreached_verify",
            about: "The storage destination confirms no upload it could not reach",
            result: judge_unreached_verify(
                plugin.verify("conformance", UNREACHED_OBJECT, None).await,
            ),
        },
        Typed::RemoteJob(plugin) => {
            let mut answers = Vec::new();
            for (kind, sample) in CONTAINER_SAMPLES {
                let answer = plugin
                    .claims(&RemoteJobSource::Container(sample.to_vec()))
                    .await;
                answers.push((kind, answer));
            }
            Answer {
                id: "remote_job_containers",
                about: "The remote-job plugin takes exactly the container formats its manifest \
                        declares, of torrent and nzb",
                result: judge_containers(declared_containers(manifest), answers),
            }
        }
        Typed::StreamTransform(plugin) => Answer {
            id: "stream_transform_empty_input",
            about: "The stream transform claims no empty address, and answers without a failure",
            result: judge_unclaimed(plugin.claims("").await),
        },
    }
}

/// What a trap says, or `None` for an error that is a refusal.
fn trapped(error: &anyhow::Error) -> Option<String> {
    error
        .downcast_ref::<wasmtime::Trap>()
        .map(|_| format!("the plugin trapped: {error:#}"))
}

/// A refusal is what is asked for; a success or a trap is not.
pub(super) fn judge_refusal(outcome: anyhow::Result<()>) -> Result<(), String> {
    match outcome {
        Ok(()) => Err("it reported a delivery to an empty destination".to_owned()),
        Err(error) => trapped(&error).map_or(Ok(()), Err),
    }
}

/// Anything but an authorized account: a failure, a prompt, a wait.
pub(super) fn judge_no_credentials(outcome: anyhow::Result<AuthProgress>) -> Result<(), String> {
    match outcome {
        Ok(AuthProgress::Authorized) => {
            Err("it reported the account authorized without any credentials".to_owned())
        }
        Ok(_) => Ok(()),
        Err(error) => trapped(&error).map_or(Ok(()), Err),
    }
}

/// No candidate; a refusal is as good as an empty list.
pub(super) fn judge_unknown_input(
    outcome: anyhow::Result<Vec<IntakeProposal>>,
) -> Result<(), String> {
    match outcome {
        Ok(proposals) if proposals.is_empty() => Ok(()),
        Ok(proposals) => Err(format!(
            "it proposed {} candidate(s), the first {}, for text with no link",
            proposals.len(),
            proposals[0].url
        )),
        Err(error) => trapped(&error).map_or(Ok(()), Err),
    }
}

/// An empty list, and only that: a failure costs a candidate its state.
pub(super) fn judge_no_fields(
    outcome: anyhow::Result<Vec<(String, String)>>,
) -> Result<(), String> {
    match outcome {
        Ok(fields) if fields.is_empty() => Ok(()),
        Ok(fields) => Err(format!(
            "it answered an empty link with {} field(s), the first `{}`",
            fields.len(),
            fields[0].0
        )),
        Err(error) => Err(format!(
            "it failed instead of answering with nothing: {error:#}"
        )),
    }
}

/// Complete without a warning, or skipped.
pub(super) fn judge_empty_package(outcome: anyhow::Result<StepOutcome>) -> Result<(), String> {
    match outcome {
        Ok(StepOutcome::Skipped) => Ok(()),
        Ok(StepOutcome::Complete { warnings, .. }) if warnings.is_empty() => Ok(()),
        Ok(StepOutcome::Complete { warnings, .. }) => Err(format!(
            "it warned about a package without files: {}",
            warnings[0].code
        )),
        Ok(StepOutcome::Stopped { .. }) => Err("it stopped on a package without files".to_owned()),
        Ok(StepOutcome::Failed { message }) => {
            Err(format!("it failed a package without files: {message}"))
        }
        Err(error) => Err(format!("it failed a package without files: {error:#}")),
    }
}

/// Never `true`: the host deletes the local copy on that answer.
pub(super) fn judge_unreached_verify(outcome: anyhow::Result<bool>) -> Result<(), String> {
    match outcome {
        Ok(true) => Err(format!(
            "it confirmed {UNREACHED_OBJECT}, which no conformance run can reach"
        )),
        Ok(false) => Ok(()),
        Err(error) => trapped(&error).map_or(Ok(()), Err),
    }
}

/// A refusal that is not a trap.
pub(super) fn judge_undeclared_scheme(outcome: Result<RemoteFile, Failure>) -> Result<(), String> {
    match outcome {
        Ok(_) => Err(format!(
            "it described a file at a `{UNDECLARED_SCHEME}://` address its manifest does not \
             declare"
        )),
        Err(failure)
            if failure
                .code
                .as_deref()
                .is_some_and(|code| TRAP_CODES.contains(&code)) =>
        {
            Err(format!("the plugin trapped: {}", failure.message))
        }
        Err(_) => Ok(()),
    }
}

/// Each sample claimed exactly when its format is declared.
pub(super) fn judge_containers(
    declared: &[String],
    answers: Vec<(&str, anyhow::Result<bool>)>,
) -> Result<(), String> {
    let mut wrong = Vec::new();
    for (kind, answer) in answers {
        let declares = declared.iter().any(|format| format == kind);
        match answer {
            Ok(claimed) if claimed == declares => {}
            Ok(true) => wrong.push(format!("it claims a {kind} container it does not declare")),
            Ok(false) => wrong.push(format!("it declares {kind} but claims no {kind} container")),
            Err(error) => wrong.push(format!("a {kind} container: {error:#}")),
        }
    }
    if wrong.is_empty() {
        Ok(())
    } else {
        Err(wrong.join("; "))
    }
}

/// `false`, not a failure.
pub(super) fn judge_unclaimed(outcome: anyhow::Result<bool>) -> Result<(), String> {
    match outcome {
        Ok(false) => Ok(()),
        Ok(true) => Err("it claims the empty address".to_owned()),
        Err(error) => Err(format!("it failed on the empty address: {error:#}")),
    }
}

fn declared_containers(manifest: &PluginManifest) -> &[String] {
    manifest
        .extension
        .as_ref()
        .map(|extension| extension.containers.as_slice())
        .unwrap_or_default()
}

fn empty_delivery() -> Delivery<'static> {
    Delivery {
        title: "rDownloader conformance check",
        body: "A delivery to an empty destination.",
        event: "conformance",
        severity: "info",
        idempotency_key: "conformance",
        destination: "",
        secret_ref: None,
        settings: &[],
        package_path: None,
    }
}

/// Probes an address in [`UNDECLARED_SCHEME`], with a part file that is removed afterwards.
async fn probe_undeclared(
    plugin: &TransferBackend,
    manifest: &PluginManifest,
) -> anyhow::Result<Result<RemoteFile, Failure>> {
    let path = std::env::temp_dir().join(format!("rd-conformance-{}.part", uuid::Uuid::now_v7()));
    let part = rd_files::PartFile::open(path.clone(), None).await?;
    let state = TransferState::new(
        TransferTarget {
            part,
            committed: 0,
            total: None,
        },
        tokio_util::sync::CancellationToken::new(),
        rd_limits::ScopedLimiter::unlimited(),
        manifest.capabilities.net_stream.clone(),
        Arc::new(rd_http::tls_client_config(&[])?),
        Arc::new(|_, _| {}),
    );
    let outcome = plugin
        .probe(
            state,
            format!("{UNDECLARED_SCHEME}://conformance.invalid:1/conformance.bin"),
            None,
        )
        .await;
    // Best effort: a part file left in the temporary directory is harmless.
    let _ = tokio::fs::remove_file(&path).await;
    Ok(outcome)
}

#[cfg(test)]
#[path = "questions_tests.rs"]
mod tests;
