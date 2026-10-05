use super::{BEGIN, COVERAGE, Decision, END, markdown};

const DOC_FILE: &str = "crates/rd-api/mcp-coverage.md";
const DOC: &str = include_str!("../mcp-coverage.md");

fn generated() -> String {
    markdown(&super::tests::documented().into_iter().collect::<Vec<_>>())
}

fn block(text: &str) -> &str {
    let start = text.find(BEGIN).expect("the page has a BEGIN marker");
    let end = text.find(END).expect("the page has an END marker") + END.len();
    &text[start..end]
}

#[test]
fn the_doc_carries_the_generated_table() {
    let generated = generated();
    let normalised = DOC.replace("\r\n", "\n");
    assert_eq!(
        block(&normalised),
        generated.trim_end(),
        "{DOC_FILE} is out of date; run scripts/mcp-coverage.sh"
    );
}

/// Every decision is readable on the page, not only in this source.
///
/// The deliverable the owner asked for is the decision per gap, and the place it is read is
/// the page. A reason that exists only as a Rust doc comment is not delivered.
#[test]
fn every_reason_reaches_the_doc() {
    for capability in COVERAGE {
        let Decision::Omitted(why) = capability.decision else {
            continue;
        };
        let one_line: String = why.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            DOC.contains(&one_line),
            "the reason for leaving out {} is not in {DOC_FILE}",
            capability.name
        );
    }
}

/// Writes the table into the page. Run through `scripts/mcp-coverage.sh`, never in CI.
#[test]
#[ignore = "rewrites crates/rd-api/mcp-coverage.md; scripts/mcp-coverage.sh runs it"]
fn write_the_doc_table() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(DOC_FILE);
    let current = std::fs::read_to_string(&path).expect("the page is readable");
    let updated = current.replace(block(&current), generated().trim_end());
    std::fs::write(&path, updated).expect("the page is writable");
    println!("wrote the comparison into {DOC_FILE}");
}
