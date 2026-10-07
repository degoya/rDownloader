use super::{RegexNode, RegexNodeKind, STRUCTURE_LIMITS_CODE, pattern_structure};

/// The structure in one line, so a table row reads like the pattern it describes.
fn sketch(node: &RegexNode) -> String {
    let children = |separator: &str| {
        node.children
            .iter()
            .map(sketch)
            .collect::<Vec<_>>()
            .join(separator)
    };
    let text = node.text.clone().unwrap_or_default();
    let not = if node.negated { "not " } else { "" };
    match node.kind {
        RegexNodeKind::Sequence => format!("seq({})", children(", ")),
        RegexNodeKind::Alternation => format!("alt({})", children(" | ")),
        RegexNodeKind::Group => {
            let label = match (node.index, &node.name) {
                (Some(index), Some(name)) => format!("#{index} {name}"),
                (Some(index), None) => format!("#{index}"),
                (None, _) => format!("?:{}", flag_list(node)),
            };
            format!("{label}({})", children(""))
        }
        RegexNodeKind::Repetition => {
            let max = node.max.map(|max| max.to_string()).unwrap_or_default();
            let range = match (node.min, node.max) {
                (Some(min), Some(max)) if min == max => format!("{{{min}}}"),
                (Some(min), _) => format!("{{{min},{max}}}"),
                (None, _) => "{?}".to_owned(),
            };
            let lazy = if node.lazy { "?" } else { "" };
            format!("{}{range}{lazy}", children(""))
        }
        RegexNodeKind::Literal => format!("{text:?}"),
        RegexNodeKind::AnyChar => ".".to_owned(),
        RegexNodeKind::Digit => (if node.negated { r"\D" } else { r"\d" }).to_owned(),
        RegexNodeKind::WordChar => (if node.negated { r"\W" } else { r"\w" }).to_owned(),
        RegexNodeKind::Whitespace => (if node.negated { r"\S" } else { r"\s" }).to_owned(),
        RegexNodeKind::UnicodeClass => {
            format!("{}{{{text}}}", if node.negated { r"\P" } else { r"\p" })
        }
        RegexNodeKind::AsciiClass => format!("[:{}{text}:]", if node.negated { "^" } else { "" }),
        RegexNodeKind::Class => format!("{not}class({})", children(" ")),
        RegexNodeKind::Range => format!(
            "{}-{}",
            node.from.as_deref().unwrap_or("?"),
            node.to.as_deref().unwrap_or("?")
        ),
        RegexNodeKind::Intersection => format!("{not}and({})", children(", ")),
        RegexNodeKind::Difference => format!("{not}minus({})", children(", ")),
        RegexNodeKind::SymmetricDifference => format!("{not}xor({})", children(", ")),
        RegexNodeKind::Start => "^".to_owned(),
        RegexNodeKind::End => "$".to_owned(),
        RegexNodeKind::WordBoundary => r"\b".to_owned(),
        RegexNodeKind::NotWordBoundary => r"\B".to_owned(),
        RegexNodeKind::WordStart => r"\<".to_owned(),
        RegexNodeKind::WordEnd => r"\>".to_owned(),
        RegexNodeKind::Flags => format!("flags({})", flag_list(node)),
        RegexNodeKind::Empty => "empty".to_owned(),
    }
}

fn flag_list(node: &RegexNode) -> String {
    node.flags
        .iter()
        .map(|flag| {
            let name = serde_json::to_value(flag.flag)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            if flag.enabled {
                name
            } else {
                format!("-{name}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn structure(pattern: &str) -> String {
    let (node, code) = pattern_structure(pattern);
    assert_eq!(code, None, "{pattern} has no structure");
    sketch(&node.expect("a structure"))
}

#[test]
fn patterns_become_their_structure() {
    let table = [
        // The five examples from the owner's regex-vis screenshot.
        (r"^\d+$", r"seq(^, \d{1,}, $)"),
        (r"^\d*\.\d+$", r#"seq(^, \d{0,}, ".", \d{1,}, $)"#),
        (
            r"^\d*(\.\d+)?$",
            r#"seq(^, \d{0,}, #1(seq(".", \d{1,})){0,1}, $)"#,
        ),
        (
            r"^-?\d*(\.\d+)?$",
            r#"seq(^, "-"{0,1}, \d{0,}, #1(seq(".", \d{1,})){0,1}, $)"#,
        ),
        (
            r"^https?://(www\.)?[a-z0-9.-]+\.[a-z]{2,6}\b",
            r#"seq(^, "http", "s"{0,1}, "://", #1("www."){0,1}, class(a-z 0-9 "." "-"){1,}, ".", class(a-z){2,6}, \b)"#,
        ),
        // Flags, alone and on a group.
        (
            r"(?i)update.*nsw-",
            r#"seq(flags(case_insensitive), "update", .{0,}, "nsw-")"#,
        ),
        (
            r"(?i-s)a",
            r#"seq(flags(case_insensitive -dot_matches_new_line), "a")"#,
        ),
        (
            r"(?i:x264|x265)",
            r#"?:case_insensitive(alt("x264" | "x265"))"#,
        ),
        (r"(?:ab)c", r#"seq(?:("ab"), "c")"#),
        // Groups with a name, in both spellings, and their numbers.
        (r"(?P<year>\d{4})", r"#1 year(\d{4})"),
        (r"(a)(?<b>c)", r#"seq(#1("a"), #2 b("c"))"#),
        // Alternatives, an empty one included.
        ("mkv|mp4|avi", r#"alt("mkv" | "mp4" | "avi")"#),
        ("a|", r#"alt("a" | empty)"#),
        // Classes: negated, Perl, Unicode, ASCII, nested, set operations.
        (r"[^\s\d]", r"not class(\s \d)"),
        (r"\D\W\S.", r"seq(\D, \W, \S, .)"),
        (r"\p{Greek}\PL", r"seq(\p{Greek}, \P{L})"),
        (r"\p{Script=Greek}", r"\p{Script=Greek}"),
        (r"[[:alpha:][:^digit:]]", "class([:alpha:] [:^digit:])"),
        (
            r"[a-z&&[^aeiou]]",
            r#"and(class(a-z), not class("a" "e" "i" "o" "u"))"#,
        ),
        (r"[a-c--b]", r#"minus(class(a-c), class("b"))"#),
        // Repetitions: bounded, open, exact, lazy.
        ("a{2,}b{1,3}?c{3}", r#"seq("a"{2,}, "b"{1,3}?, "c"{3})"#),
        (".+?", ".{1,}?"),
        // Assertions.
        (r"\bS\d{2}\B", r#"seq(\b, "S", \d{2}, \B)"#),
        (r"\b{start}x\b{end}", r#"seq(\<, "x", \>)"#),
        (r"\Ax\z", r#"seq(^, "x", $)"#),
        // Escapes stay the character they stand for; text is not split per character.
        ("Caf\u{e9}\\.\\(1\\)", "\"Caf\u{e9}.(1)\""),
        ("", "empty"),
    ];
    for (pattern, expected) in table {
        assert_eq!(structure(pattern), expected, "pattern {pattern}");
    }
}

#[test]
fn a_pattern_nested_too_deep_has_a_code_instead_of_a_structure() {
    let pattern = format!("{}a{}", "(".repeat(40), ")".repeat(40));
    assert!(
        regex::Regex::new(&pattern).is_ok(),
        "the pattern itself compiles"
    );
    assert_eq!(
        pattern_structure(&pattern),
        (None, Some(STRUCTURE_LIMITS_CODE.to_owned()))
    );
    // Within the limit it is drawn.
    let shallow = format!("{}a{}", "(".repeat(10), ")".repeat(10));
    assert!(pattern_structure(&shallow).0.is_some());
}

#[test]
fn a_pattern_with_too_many_boxes_has_a_code_instead_of_a_structure() {
    let pattern = "(a)".repeat(250);
    assert!(
        regex::Regex::new(&pattern).is_ok(),
        "the pattern itself compiles"
    );
    assert_eq!(
        pattern_structure(&pattern),
        (None, Some(STRUCTURE_LIMITS_CODE.to_owned()))
    );
    // Merged text counts as one box, however long.
    assert_eq!(
        structure(&"a".repeat(1500)),
        format!("{:?}", "a".repeat(1500))
    );
}

#[test]
fn the_structure_serializes_only_what_a_node_carries() {
    let (node, _) = pattern_structure(r"(?<y>\d+?)");
    let json = serde_json::to_value(node.expect("a structure")).expect("serializable");
    assert_eq!(
        json,
        serde_json::json!({
            "kind": "group",
            "index": 1,
            "name": "y",
            "children": [{
                "kind": "repetition",
                "min": 1,
                "lazy": true,
                "children": [{ "kind": "digit" }]
            }]
        })
    );
}
