//! The rules and overrides in force for this process, and the functions that judge against
//! them.

use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, LazyLock, RwLock},
};

use super::{Assessment, CompatRules, RULED_TOOLS};
use crate::{
    manifest::ToolManifest,
    version::{self, DetectedVersion},
};

/// The rules and overrides in force for this process.
#[derive(Clone, Debug)]
struct Active {
    rules: CompatRules,
    overrides: BTreeSet<String>,
}

static ACTIVE: LazyLock<RwLock<Arc<Active>>> = LazyLock::new(|| {
    RwLock::new(Arc::new(Active {
        rules: CompatRules::base(),
        overrides: BTreeSet::new(),
    }))
});

/// The rules and overrides currently in force. Base rules and no overrides until something
/// says otherwise, which is what makes a process that never starts the tool service behave
/// exactly like one that did and found nothing.
fn active() -> Arc<Active> {
    ACTIVE
        .read()
        .map(|active| Arc::clone(&active))
        .unwrap_or_else(|_| {
            Arc::new(Active {
                rules: CompatRules::base(),
                overrides: BTreeSet::new(),
            })
        })
}

fn replace(build: impl FnOnce(&Active) -> Active) {
    if let Ok(mut current) = ACTIVE.write() {
        *current = Arc::new(build(&current));
    }
}

/// Puts a rule set in force.
pub fn set_rules(rules: CompatRules) {
    replace(|current| Active {
        rules,
        overrides: current.overrides.clone(),
    });
}

/// Puts the explicit overrides in force, as tool names.
///
/// Unknown names are dropped rather than refused: the setting is a list of tools, and a tool
/// this build does not gate on simply has no rule to override.
pub fn set_overrides(tools: &[String]) {
    let overrides: BTreeSet<String> = tools
        .iter()
        .map(|tool| tool.trim().to_lowercase())
        .filter(|tool| RULED_TOOLS.contains(&tool.as_str()))
        .collect();
    replace(|current| Active {
        rules: current.rules.clone(),
        overrides,
    });
}

/// Adopts the rules a verified manifest carries, degrading to [`CompatRules::base`] on any
/// failure.
///
/// The manifest reaching here has already passed signature, schema and replay verification;
/// this is the second half of the same rule — a document that verified but whose rules this
/// build cannot read leaves the compiled-in floor in force, and says so.
pub fn adopt_manifest(manifest: &ToolManifest) {
    match CompatRules::from_manifest(manifest) {
        Ok(rules) => set_rules(rules),
        Err(error) => {
            tracing::warn!(
                %error,
                "the tool manifest's compatibility rules are unusable; \
                 the compiled-in base rules stay in force"
            );
            set_rules(CompatRules::base());
        }
    }
}

/// The rules in force.
#[must_use]
pub fn rules() -> CompatRules {
    active().rules.clone()
}

/// Judges an already-detected version against the rules in force.
#[must_use]
pub fn assess_detected(tool: &str, detected: &DetectedVersion) -> Assessment {
    let active = active();
    let assessment = active
        .rules
        .assess(tool, detected, active.overrides.contains(tool));
    audit(&assessment);
    assessment
}

/// Reads the version at `path` — from the cache when the binary has not changed — and judges
/// it against the rules in force.
pub async fn assess(tool: &str, path: &Path) -> Assessment {
    let detected = version::detect(tool, path).await;
    assess_detected(tool, &detected)
}

/// Reads an archive tool's version and judges it against its security floor alone.
///
/// Unlike [`assess`], neither the rules a manifest delivered nor an override enter: the floor
/// is [`ARCHIVE_TOOL_FLOORS`](super::ARCHIVE_TOOL_FLOORS), compiled in. An unreadable version is
/// [`Verdict::Unknown`](super::Verdict::Unknown) as everywhere else, and the caller decides what
/// that means for its tool.
pub async fn assess_archive_tool(tool: &str, path: &Path) -> Assessment {
    let detected = version::detect(tool, path).await;
    CompatRules::base().assess(tool, &detected, false)
}

/// Records an override that actually suppressed a block.
///
/// This is the auditable half of "override only explicit and auditable": the setting is the
/// explicit half, and nothing skips a block without leaving this line behind.
fn audit(assessment: &Assessment) {
    if assessment.overridden && assessment.verdict.is_incompatible() {
        tracing::warn!(
            tool = %assessment.tool,
            verdict = %assessment.verdict,
            version = assessment.version.as_deref().unwrap_or("unknown"),
            min_version = assessment.min_version.as_deref().unwrap_or("none"),
            affects = %assessment
                .affects
                .iter()
                .map(|capability| capability.as_str())
                .collect::<Vec<_>>()
                .join(","),
            "a tool compatibility rule is overridden by an explicit setting; \
             the affected capabilities are not blocked"
        );
    }
}
