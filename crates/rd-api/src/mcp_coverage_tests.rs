use std::collections::BTreeSet;

use super::{COVERAGE, Decision, Method, OWNER_LINE, capability_for, tools_for};

/// Every `(path, method)` the OpenAPI document carries.
///
/// Read out of the serialised document for the same reason `scope_policy` reads it there:
/// the JSON is the contract, and it does not move when utoipa reshapes its types.
pub(super) fn documented() -> BTreeSet<(String, Method)> {
    let document = serde_json::to_value(crate::openapi_document()).expect("serialise");
    let paths = document
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .expect("the document has paths");
    let mut operations = BTreeSet::new();
    for (path, item) in paths {
        for method in item.as_object().expect("path item").keys() {
            let Ok(method) = Method::from_bytes(method.to_uppercase().as_bytes()) else {
                continue;
            };
            operations.insert((path.clone(), method));
        }
    }
    assert!(
        operations.len() > 200,
        "only {} operations were found; the document did not serialise as expected",
        operations.len()
    );
    operations
}

/// No gap stays uncommented, and the build is where that is enforced.
///
/// This is the whole point of the table. A coverage answer written once decays the moment
/// somebody adds a route; a route that falls into no capability fails here instead, so
/// adding one without saying whether the toolbox should have it is not possible.
#[test]
fn every_documented_operation_belongs_to_a_capability() {
    let orphans: Vec<String> = documented()
        .into_iter()
        .filter(|(path, method)| capability_for(path, method).is_none())
        .map(|(path, method)| format!("{method} {path}"))
        .collect();
    assert!(
        orphans.is_empty(),
        "these operations belong to no capability, so nothing says whether MCP should \
         cover them:\n  {}",
        orphans.join("\n  ")
    );
}

/// And a capability that claims nothing real is a decision about something that is gone.
#[test]
fn every_capability_claims_at_least_one_operation() {
    let documented = documented();
    let empty: Vec<&str> = COVERAGE
        .iter()
        .filter(|capability| {
            !documented.iter().any(|(path, method)| {
                capability_for(path, method).is_some_and(|found| std::ptr::eq(found, *capability))
            })
        })
        .map(|capability| capability.name)
        .collect();
    assert!(
        empty.is_empty(),
        "these capabilities match no operation the API has: {empty:?}"
    );
}

/// `Covered` means a tool exists, and `Omitted` means none does — checked, not asserted.
///
/// Written this way round on purpose: the decision is the thing a person reads, and it is
/// held against `TOOL_POLICY`, which is held against `scope_policy`, which is held against
/// the document. A tool quietly added to an omitted capability fails here rather than
/// leaving the reason standing as a lie.
#[test]
fn the_decision_and_the_tools_agree() {
    for capability in COVERAGE {
        let tools = tools_for(capability);
        match capability.decision {
            Decision::Covered => assert!(
                !tools.is_empty(),
                "{} is marked covered but no tool reaches it",
                capability.name
            ),
            Decision::Omitted(why) => {
                assert!(
                    tools.is_empty(),
                    "{} is marked deliberately out but these tools reach it: {tools:?}",
                    capability.name
                );
                assert!(
                    why.len() > 40,
                    "{} is left out without a reason worth reading",
                    capability.name
                );
            }
        }
    }
}

/// Two capabilities claiming one operation equally would make the winner arbitrary.
#[test]
fn no_two_capabilities_claim_the_same_route_the_same_way() {
    let mut seen: Vec<(&str, Option<&str>)> = Vec::new();
    for capability in COVERAGE {
        for claim in capability.claims {
            let key = (claim.prefix, claim.method);
            assert!(
                !seen.contains(&key),
                "{} claims {} {:?}, which another capability already claims",
                capability.name,
                claim.prefix,
                claim.method
            );
            seen.push(key);
        }
    }
}

/// The findings the job reports, pinned so a later change has to face them.
#[test]
fn the_capabilities_rd_120_29_decided_stay_decided() {
    let by_name = |name: &str| {
        COVERAGE
            .iter()
            .find(|capability| capability.name == name)
            .unwrap_or_else(|| panic!("no capability called {name}"))
    };
    assert_eq!(by_name("Remote jobs").decision, Decision::Covered);
    assert_eq!(by_name("Transfer statistics").decision, Decision::Covered);
    assert_eq!(by_name("The log store").decision, Decision::Covered);
    assert_eq!(by_name("The audit log").decision, Decision::Covered);
    assert_eq!(
        by_name("Clearing logs, audit records and statistics").decision,
        Decision::Covered
    );
    assert_eq!(
        by_name("Site rules: read and switch").decision,
        Decision::Covered
    );
    assert!(matches!(
        by_name("Deleting a remote job at the provider").decision,
        Decision::Omitted(_)
    ));
    // Out under RD-120-29, in since RD-120-31 gave the import routes a JSON body.
    assert_eq!(
        by_name("Handing in a container file").decision,
        Decision::Covered
    );
}

/// RD-120-32's three sorts, pinned: group 1 and 2 in, the owner's nine out on his line.
#[test]
fn the_capabilities_rd_120_32_decided_stay_decided() {
    let by_name = |name: &str| {
        COVERAGE
            .iter()
            .find(|capability| capability.name == name)
            .unwrap_or_else(|| panic!("no capability called {name}"))
    };
    for name in [
        "LinkGrabber: candidate-level handling",
        "Mirror groups",
        "Reviewing an NZB before it is queued",
        "NZB import files and enqueue",
        "LinkGrabber: package editing and ordering",
        "Ordering the queue by hand",
        "Renaming and retargeting queued work",
        "Clearing finished work in one sweep",
        "Unpacking on demand",
        "Torrent detail and seeding",
        "Post-processing inventory and queue",
        "Managed external tools",
        "Storage capacity",
        "Writing a site rule",
    ] {
        assert_eq!(by_name(name).decision, Decision::Covered, "{name}");
    }
    let owner: Vec<&str> = COVERAGE
        .iter()
        .filter(|capability| capability.decision == Decision::Omitted(OWNER_LINE))
        .map(|capability| capability.name)
        .collect();
    assert_eq!(
        owner,
        [
            "Deleting a remote job at the provider",
            "Signing in, sessions, second factor and API tokens",
            "Signing in at a provider",
            "Trying a stored credential or destination",
            "Remote logins and trusted host keys",
            // RD-150-04: a profile takes the access key and secret in.
            "Object storage profiles",
            "Solving captchas",
            "Consent to replay a paid link",
            // RD-160-01: the passphrase is a secret taken in.
            "Full backup passphrase",
            // RD-160-03: every restore step takes the passphrase in.
            "Restoring a full backup",
            "Import and export of a whole area",
            "Plugin trust and installation",
            // RD-120-55: the parts of three of the thirteen that meet one of the marks.
            "Probing an indexer's capabilities",
            // RD-180-19: an indexer takes its API key in.
            "Defining and testing indexers",
            "Approving and fetching a diagnostic bundle",
            "Reconnecting on demand",
        ],
        "the owner decided nine capabilities on 2026-09-23, and RD-120-55 applied the same \
         line to three more and RD-150-04, RD-160-01, RD-160-03 and RD-180-19 to one each, \
         with one reason for all of them"
    );
    // RD-180-19: what uses the key without showing it is in.
    assert_eq!(
        by_name("Searching Newznab and Torznab indexers and taking hits into the LinkGrabber")
            .decision,
        Decision::Covered
    );
}

/// RD-120-55's verdicts, pinned: the thirteen are in, except the parts that meet a mark.
#[test]
fn the_capabilities_rd_120_55_decided_stay_decided() {
    let by_name = |name: &str| {
        COVERAGE
            .iter()
            .find(|capability| capability.name == name)
            .unwrap_or_else(|| panic!("no capability called {name}"))
    };
    for name in [
        "Which providers can take a remote job",
        "Power actions",
        "Plugin execution history",
        "Plugin message catalogues",
        "Automation history, vocabulary and dry run",
        "Notification history and the destination catalogue",
        "Subscription items, runs and forced polls",
        "Stream schedules, runs and recording now",
        "The diagnostic bundle: preview",
        "Metrics",
        "Reconnect status",
        "The hosters one account covers",
        "Trying a routing regular expression",
    ] {
        assert_eq!(by_name(name).decision, Decision::Covered, "{name}");
    }
    for name in [
        "Probing an indexer's capabilities",
        "Approving and fetching a diagnostic bundle",
        "Reconnecting on demand",
    ] {
        assert_eq!(
            by_name(name).decision,
            Decision::Omitted(OWNER_LINE),
            "{name}"
        );
    }
    // Not a mark but a route no tool can be priced by; the reason says which tool answers.
    assert!(matches!(
        by_name("The health probe").decision,
        Decision::Omitted(why) if why != OWNER_LINE
    ));
}
