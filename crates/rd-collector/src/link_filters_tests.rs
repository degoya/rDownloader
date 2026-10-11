use super::{LinkFilterContext, LinkFilters, compile_name_pattern};
use rd_core::{
    IngressSource, LinkFilterAction, LinkFilterNameSyntax, LinkFilterRule, LinkFilterRuleId,
};
use url::Url;

fn rule(position: i64, action: LinkFilterAction) -> LinkFilterRule {
    LinkFilterRule {
        id: LinkFilterRuleId::new(),
        name: format!("rule {position}"),
        position,
        enabled: true,
        name_pattern: None,
        name_syntax: LinkFilterNameSyntax::Glob,
        size_min: None,
        size_max: None,
        extensions: Vec::new(),
        hoster: None,
        source: None,
        action,
        package_name: None,
        category_id: None,
    }
}

fn glob(position: i64, action: LinkFilterAction, pattern: &str) -> LinkFilterRule {
    LinkFilterRule {
        name_pattern: Some(pattern.to_owned()),
        ..rule(position, action)
    }
}

fn decide<'a>(
    rules: &'a [LinkFilterRule],
    url: &str,
    file_name: Option<&str>,
    size: Option<u64>,
) -> Option<&'a LinkFilterRule> {
    let url: Url = url.parse().expect("url");
    LinkFilters::new(rules).decide(&LinkFilterContext {
        source: IngressSource::Clipboard,
        url: &url,
        file_name,
        size,
    })
}

#[test]
fn the_first_rule_in_order_decides_and_an_accept_shadows_a_later_hide() {
    // Stored out of order on purpose: the position decides, not the slice.
    let rules = [
        glob(2, LinkFilterAction::Hide, "*.nfo"),
        glob(1, LinkFilterAction::Accept, "readme*"),
    ];
    let url = "https://files.example/a";
    assert_eq!(
        decide(&rules, url, Some("Readme.nfo"), None).map(|rule| rule.action),
        Some(LinkFilterAction::Accept)
    );
    assert_eq!(
        decide(&rules, url, Some("Release.NFO"), None).map(|rule| rule.action),
        Some(LinkFilterAction::Hide)
    );
    assert!(decide(&rules, url, Some("Release.mkv"), None).is_none());
}

#[test]
fn a_disabled_rule_is_never_asked() {
    let mut hide = glob(1, LinkFilterAction::Hide, "*");
    hide.enabled = false;
    let rules = [hide];
    assert!(LinkFilters::new(&rules).is_empty());
    assert!(decide(&rules, "https://files.example/a", Some("x.bin"), None).is_none());
}

#[test]
fn a_glob_is_the_whole_name_with_case_ignored_and_its_other_characters_literal() {
    let pattern = compile_name_pattern("Show.S0?E*.mkv", LinkFilterNameSyntax::Glob)
        .expect("a glob compiles");
    assert!(pattern.is_match("show.s01e02.720p.MKV"));
    assert!(!pattern.is_match("Show.S01E02.mkv.part"));
    // The dot is a dot, not "any character".
    assert!(!pattern.is_match("ShowXS01E02.mkv"));
    // Regex metacharacters in a glob are literal.
    let literal = compile_name_pattern("a+b(1).rar", LinkFilterNameSyntax::Glob).expect("glob");
    assert!(literal.is_match("a+b(1).rar"));
    assert!(!literal.is_match("aab1.rar"));
}

#[test]
fn a_regex_is_searched_as_written_and_one_that_does_not_compile_never_matches() {
    let mut search = glob(1, LinkFilterAction::Hide, r"sample");
    search.name_syntax = LinkFilterNameSyntax::Regex;
    let mut broken = glob(0, LinkFilterAction::Accept, "(unclosed");
    broken.name_syntax = LinkFilterNameSyntax::Regex;
    let rules = [broken, search];
    let url = "https://files.example/a";
    assert_eq!(
        decide(&rules, url, Some("movie-sample.mkv"), None).map(|rule| rule.position),
        Some(1)
    );
    // Case is the writer's choice in a regex.
    assert!(decide(&rules, url, Some("movie-SAMPLE.mkv"), None).is_none());
    // A name pattern never matches a link without a name.
    assert!(decide(&rules, url, None, None).is_none());
}

#[test]
fn size_bounds_are_inclusive_and_an_unknown_size_never_matches_them() {
    let rules = [LinkFilterRule {
        size_min: Some(100),
        size_max: Some(200),
        ..rule(1, LinkFilterAction::Hide)
    }];
    let url = "https://files.example/a";
    assert!(decide(&rules, url, Some("a"), Some(100)).is_some());
    assert!(decide(&rules, url, Some("a"), Some(200)).is_some());
    assert!(decide(&rules, url, Some("a"), Some(99)).is_none());
    assert!(decide(&rules, url, Some("a"), Some(201)).is_none());
    assert!(decide(&rules, url, Some("a"), None).is_none());
}

#[test]
fn extensions_match_the_end_of_the_name_and_the_hoster_its_subdomains() {
    let rules = [LinkFilterRule {
        extensions: vec!["part1.rar".to_owned(), "NFO".to_owned()],
        hoster: Some("rg.example".to_owned()),
        ..rule(1, LinkFilterAction::Hide)
    }];
    assert!(decide(&rules, "https://rg.example/f/1", Some("x.part1.rar"), None).is_some());
    assert!(decide(&rules, "https://www.rg.example/f/1", Some("x.nfo"), None).is_some());
    assert!(decide(&rules, "https://notrg.example/f/1", Some("x.nfo"), None).is_none());
    assert!(decide(&rules, "https://rg.example/f/1", Some("x.part2.rar"), None).is_none());
    // `rar` alone is not the end of `xrar`.
    let rar = [LinkFilterRule {
        extensions: vec!["rar".to_owned()],
        ..rule(1, LinkFilterAction::Hide)
    }];
    assert!(decide(&rar, "https://a.example/1", Some("xrar"), None).is_none());
}

#[test]
fn a_source_condition_holds_only_for_links_that_came_that_way() {
    let rules = [LinkFilterRule {
        source: Some(IngressSource::Subscription),
        ..rule(1, LinkFilterAction::Hide)
    }];
    // `decide` above submits as the clipboard.
    assert!(decide(&rules, "https://a.example/1", Some("x"), None).is_none());
    let url: Url = "https://a.example/1".parse().expect("url");
    let decided = LinkFilters::new(&rules).decide(&LinkFilterContext {
        source: IngressSource::Subscription,
        url: &url,
        file_name: Some("x"),
        size: None,
    });
    assert!(decided.is_some());
}

#[test]
fn a_rule_without_conditions_holds_for_every_link() {
    let rules = [rule(1, LinkFilterAction::Hide)];
    assert!(decide(&rules, "https://a.example/1", None, None).is_some());
}
