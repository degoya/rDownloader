//! The structure of a pattern, for the regex editor's diagram (RD-1140-06).
//!
//! Parsed with `regex-syntax`, the grammar the `regex` crate compiles with, so the diagram shows
//! the pattern as the service runs it rather than as a JavaScript engine would read it. The walk
//! reads the syntax tree (the `Ast`), not the compiled form, so literals and classes stay as they
//! were written. Depth and node count are bounded: past either, the tester answers a stable code
//! instead of a structure.

use regex_syntax::ast::{
    self, AssertionKind, Ast, ClassAsciiKind, ClassPerlKind, ClassSet, ClassSetBinaryOpKind,
    ClassSetItem, ClassUnicodeKind, ClassUnicodeOpKind, FlagsItemKind, GroupKind, RepetitionKind,
    RepetitionRange,
};
use serde::Serialize;
use utoipa::ToSchema;

/// Nesting deeper than this is not drawn; every group, repetition or class is one level.
const MAX_DEPTH: usize = 32;
/// More nodes than this are not drawn: a diagram that long no longer explains anything.
const MAX_NODES: usize = 400;
/// The tester's answer when a pattern compiles but is too large to draw.
pub(crate) const STRUCTURE_LIMITS_CODE: &str = "category_rule.regex_structure_limits";

/// One box of the diagram. Which fields a node carries depends on its `kind`.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
pub struct RegexNode {
    pub kind: RegexNodeKind,
    /// `literal`: the text, adjacent characters merged; `unicode_class`, `ascii_class`: the name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// `range`: the first and the last character.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Character classes: everything except what they name.
    #[serde(default, skip_serializing_if = "is_false")]
    pub negated: bool,
    /// `group`: the capture group's number; absent for a non-capturing group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// `group`: the capture group's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `repetition`: the least number of times.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<u32>,
    /// `repetition`: the most number of times; absent means without limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    /// `repetition`: matches as few times as possible (`*?`, `+?`, `??`, `{n,m}?`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub lazy: bool,
    /// `flags`, and a non-capturing `group` such as `(?i:…)`: the flags switched on or off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<RegexFlag>,
    /// `sequence`, `alternation`, `class` and the set operations: their parts in order;
    /// `group` and `repetition`: the one node they wrap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(no_recursion)]
    pub children: Vec<RegexNode>,
}

/// What a node stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegexNodeKind {
    /// Its children one after another.
    Sequence,
    /// One of its children.
    Alternation,
    /// `(…)`, `(?<name>…)`, `(?:…)`.
    Group,
    /// Its one child, `min` to `max` times.
    Repetition,
    Literal,
    /// `.`
    AnyChar,
    /// `\d`, negated `\D`.
    Digit,
    /// `\w`, negated `\W`.
    WordChar,
    /// `\s`, negated `\S`.
    Whitespace,
    /// `\pL`, `\p{Greek}`, `\p{Script=Greek}`.
    UnicodeClass,
    /// `[[:alpha:]]`.
    AsciiClass,
    /// `[…]`: one of its children.
    Class,
    /// `a-z` inside a class.
    Range,
    /// `[a&&b]`: in both children.
    Intersection,
    /// `[a--b]`: in the first child, not in the second.
    Difference,
    /// `[a~~b]`: in exactly one of the children.
    SymmetricDifference,
    /// `^`, `\A`.
    Start,
    /// `$`, `\z`.
    End,
    /// `\b`.
    WordBoundary,
    /// `\B`.
    NotWordBoundary,
    /// `\b{start}`, `\<`, `\b{start-half}`.
    WordStart,
    /// `\b{end}`, `\>`, `\b{end-half}`.
    WordEnd,
    /// `(?i)`: flags from here on.
    Flags,
    /// Matches the empty string, as an empty alternative does.
    Empty,
}

#[derive(Debug, PartialEq, Serialize, ToSchema)]
pub struct RegexFlag {
    pub flag: RegexFlagName,
    /// False for a flag after `-`, as in `(?-i)`.
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegexFlagName {
    /// `i`
    CaseInsensitive,
    /// `m`
    MultiLine,
    /// `s`
    DotMatchesNewLine,
    /// `U`
    SwapGreed,
    /// `u`
    Unicode,
    /// `R`
    Crlf,
    /// `x`
    IgnoreWhitespace,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl RegexNode {
    fn of(kind: RegexNodeKind) -> Self {
        Self {
            kind,
            text: None,
            from: None,
            to: None,
            negated: false,
            index: None,
            name: None,
            min: None,
            max: None,
            lazy: false,
            flags: Vec::new(),
            children: Vec::new(),
        }
    }

    fn text(kind: RegexNodeKind, text: String, negated: bool) -> Self {
        Self {
            text: Some(text),
            negated,
            ..Self::of(kind)
        }
    }

    fn parent(kind: RegexNodeKind, children: Vec<RegexNode>) -> Self {
        Self {
            children,
            ..Self::of(kind)
        }
    }
}

/// The pattern's structure, or the code that says why there is none.
///
/// Called only for a pattern that compiled, so the parse cannot fail in practice: `regex` parses
/// with this parser and these defaults. Should it fail anyway, there is no structure and no code.
pub(crate) fn pattern_structure(pattern: &str) -> (Option<RegexNode>, Option<String>) {
    let Ok(ast) = ast::parse::Parser::new().parse(pattern) else {
        return (None, None);
    };
    let mut walk = Walk { nodes: 0 };
    match walk.node(&ast, 0) {
        Ok(node) => (Some(node), None),
        Err(TooLarge) => (None, Some(STRUCTURE_LIMITS_CODE.to_owned())),
    }
}

/// The walk stopped at a limit.
struct TooLarge;

struct Walk {
    nodes: usize,
}

impl Walk {
    /// Counts one node at `depth`, refusing past either limit.
    fn count(&mut self, depth: usize) -> Result<(), TooLarge> {
        self.nodes += 1;
        if self.nodes > MAX_NODES || depth > MAX_DEPTH {
            return Err(TooLarge);
        }
        Ok(())
    }

    fn node(&mut self, ast: &Ast, depth: usize) -> Result<RegexNode, TooLarge> {
        self.count(depth)?;
        Ok(match ast {
            Ast::Empty(_) => RegexNode::of(RegexNodeKind::Empty),
            Ast::Flags(set) => RegexNode {
                flags: flags(&set.flags),
                ..RegexNode::of(RegexNodeKind::Flags)
            },
            Ast::Literal(literal) => {
                RegexNode::text(RegexNodeKind::Literal, literal.c.to_string(), false)
            }
            Ast::Dot(_) => RegexNode::of(RegexNodeKind::AnyChar),
            Ast::Assertion(assertion) => RegexNode::of(assertion_kind(&assertion.kind)),
            Ast::ClassUnicode(class) => unicode_class(class),
            Ast::ClassPerl(class) => perl_class(class),
            Ast::ClassBracketed(class) => self.bracketed(class, depth)?,
            Ast::Repetition(repetition) => self.repetition(repetition, depth)?,
            Ast::Group(group) => self.group(group, depth)?,
            Ast::Alternation(alternation) => RegexNode::parent(
                RegexNodeKind::Alternation,
                self.each(&alternation.asts, depth)?,
            ),
            Ast::Concat(concat) => self.sequence(&concat.asts, depth)?,
        })
    }

    fn each(&mut self, asts: &[Ast], depth: usize) -> Result<Vec<RegexNode>, TooLarge> {
        asts.iter().map(|ast| self.node(ast, depth + 1)).collect()
    }

    /// A concatenation, with adjacent literals merged into one text box (`http`, not `h t t p`);
    /// a sequence of one box is that box.
    fn sequence(&mut self, asts: &[Ast], depth: usize) -> Result<RegexNode, TooLarge> {
        let mut children: Vec<RegexNode> = Vec::new();
        for ast in asts {
            if let (Ast::Literal(literal), Some(last)) = (ast, children.last_mut())
                && last.kind == RegexNodeKind::Literal
                && let Some(text) = last.text.as_mut()
            {
                text.push(literal.c);
                continue;
            }
            children.push(self.node(ast, depth + 1)?);
        }
        if children.len() == 1
            && let Some(only) = children.pop()
        {
            return Ok(only);
        }
        Ok(RegexNode::parent(RegexNodeKind::Sequence, children))
    }

    fn repetition(
        &mut self,
        repetition: &ast::Repetition,
        depth: usize,
    ) -> Result<RegexNode, TooLarge> {
        let (min, max) = match &repetition.op.kind {
            RepetitionKind::ZeroOrOne => (0, Some(1)),
            RepetitionKind::ZeroOrMore => (0, None),
            RepetitionKind::OneOrMore => (1, None),
            RepetitionKind::Range(RepetitionRange::Exactly(count)) => (*count, Some(*count)),
            RepetitionKind::Range(RepetitionRange::AtLeast(min)) => (*min, None),
            RepetitionKind::Range(RepetitionRange::Bounded(min, max)) => (*min, Some(*max)),
        };
        Ok(RegexNode {
            min: Some(min),
            max,
            lazy: !repetition.greedy,
            children: vec![self.node(&repetition.ast, depth + 1)?],
            ..RegexNode::of(RegexNodeKind::Repetition)
        })
    }

    fn group(&mut self, group: &ast::Group, depth: usize) -> Result<RegexNode, TooLarge> {
        let mut node = RegexNode {
            children: vec![self.node(&group.ast, depth + 1)?],
            ..RegexNode::of(RegexNodeKind::Group)
        };
        match &group.kind {
            GroupKind::CaptureIndex(index) => node.index = Some(*index),
            GroupKind::CaptureName { name, .. } => {
                node.index = Some(name.index);
                node.name = Some(name.name.clone());
            }
            GroupKind::NonCapturing(group_flags) => node.flags = flags(group_flags),
        }
        Ok(node)
    }

    fn bracketed(
        &mut self,
        class: &ast::ClassBracketed,
        depth: usize,
    ) -> Result<RegexNode, TooLarge> {
        Ok(RegexNode {
            negated: class.negated,
            ..self.class_set(&class.kind, depth)?
        })
    }

    /// A set as a `class` box listing its items, or a set operation over two such boxes.
    fn class_set(&mut self, set: &ClassSet, depth: usize) -> Result<RegexNode, TooLarge> {
        match set {
            ClassSet::Item(item) => {
                let mut children = Vec::new();
                self.class_items(item, depth + 1, &mut children)?;
                Ok(RegexNode::parent(RegexNodeKind::Class, children))
            }
            ClassSet::BinaryOp(operation) => {
                let kind = match operation.kind {
                    ClassSetBinaryOpKind::Intersection => RegexNodeKind::Intersection,
                    ClassSetBinaryOpKind::Difference => RegexNodeKind::Difference,
                    ClassSetBinaryOpKind::SymmetricDifference => RegexNodeKind::SymmetricDifference,
                };
                let children = vec![
                    self.class_operand(&operation.lhs, depth + 1)?,
                    self.class_operand(&operation.rhs, depth + 1)?,
                ];
                Ok(RegexNode::parent(kind, children))
            }
        }
    }

    /// One side of a set operation; a bracketed side is its own box, not a box inside a box.
    fn class_operand(&mut self, set: &ClassSet, depth: usize) -> Result<RegexNode, TooLarge> {
        self.count(depth)?;
        match set {
            ClassSet::Item(ClassSetItem::Bracketed(class)) => self.bracketed(class, depth),
            _ => self.class_set(set, depth),
        }
    }

    /// Appends the boxes one class item stands for: a union its members, an empty item none.
    fn class_items(
        &mut self,
        item: &ClassSetItem,
        depth: usize,
        into: &mut Vec<RegexNode>,
    ) -> Result<(), TooLarge> {
        if let ClassSetItem::Union(union) = item {
            for member in &union.items {
                self.class_items(member, depth, into)?;
            }
            return Ok(());
        }
        if let ClassSetItem::Empty(_) = item {
            return Ok(());
        }
        self.count(depth)?;
        into.push(match item {
            ClassSetItem::Literal(literal) => {
                RegexNode::text(RegexNodeKind::Literal, literal.c.to_string(), false)
            }
            ClassSetItem::Range(range) => RegexNode {
                from: Some(range.start.c.to_string()),
                to: Some(range.end.c.to_string()),
                ..RegexNode::of(RegexNodeKind::Range)
            },
            ClassSetItem::Ascii(class) => RegexNode::text(
                RegexNodeKind::AsciiClass,
                ascii_name(&class.kind).to_owned(),
                class.negated,
            ),
            ClassSetItem::Unicode(class) => unicode_class(class),
            ClassSetItem::Perl(class) => perl_class(class),
            ClassSetItem::Bracketed(class) => self.bracketed(class, depth)?,
            ClassSetItem::Union(_) | ClassSetItem::Empty(_) => return Ok(()),
        });
        Ok(())
    }
}

fn assertion_kind(kind: &AssertionKind) -> RegexNodeKind {
    match kind {
        AssertionKind::StartLine | AssertionKind::StartText => RegexNodeKind::Start,
        AssertionKind::EndLine | AssertionKind::EndText => RegexNodeKind::End,
        AssertionKind::WordBoundary => RegexNodeKind::WordBoundary,
        AssertionKind::NotWordBoundary => RegexNodeKind::NotWordBoundary,
        AssertionKind::WordBoundaryStart
        | AssertionKind::WordBoundaryStartAngle
        | AssertionKind::WordBoundaryStartHalf => RegexNodeKind::WordStart,
        AssertionKind::WordBoundaryEnd
        | AssertionKind::WordBoundaryEndAngle
        | AssertionKind::WordBoundaryEndHalf => RegexNodeKind::WordEnd,
    }
}

fn perl_class(class: &ast::ClassPerl) -> RegexNode {
    let kind = match class.kind {
        ClassPerlKind::Digit => RegexNodeKind::Digit,
        ClassPerlKind::Word => RegexNodeKind::WordChar,
        ClassPerlKind::Space => RegexNodeKind::Whitespace,
    };
    RegexNode {
        negated: class.negated,
        ..RegexNode::of(kind)
    }
}

fn unicode_class(class: &ast::ClassUnicode) -> RegexNode {
    let name = match &class.kind {
        ClassUnicodeKind::OneLetter(letter) => letter.to_string(),
        ClassUnicodeKind::Named(name) => name.clone(),
        // `!=` is carried by `is_negated`, so the name reads `Script=Greek` either way.
        ClassUnicodeKind::NamedValue { name, value, op } => match op {
            ClassUnicodeOpKind::Colon => format!("{name}:{value}"),
            ClassUnicodeOpKind::Equal | ClassUnicodeOpKind::NotEqual => {
                format!("{name}={value}")
            }
        },
    };
    RegexNode::text(RegexNodeKind::UnicodeClass, name, class.is_negated())
}

fn ascii_name(kind: &ClassAsciiKind) -> &'static str {
    match kind {
        ClassAsciiKind::Alnum => "alnum",
        ClassAsciiKind::Alpha => "alpha",
        ClassAsciiKind::Ascii => "ascii",
        ClassAsciiKind::Blank => "blank",
        ClassAsciiKind::Cntrl => "cntrl",
        ClassAsciiKind::Digit => "digit",
        ClassAsciiKind::Graph => "graph",
        ClassAsciiKind::Lower => "lower",
        ClassAsciiKind::Print => "print",
        ClassAsciiKind::Punct => "punct",
        ClassAsciiKind::Space => "space",
        ClassAsciiKind::Upper => "upper",
        ClassAsciiKind::Word => "word",
        ClassAsciiKind::Xdigit => "xdigit",
    }
}

fn flags(flags: &ast::Flags) -> Vec<RegexFlag> {
    let mut enabled = true;
    let mut out = Vec::new();
    for item in &flags.items {
        match &item.kind {
            FlagsItemKind::Negation => enabled = false,
            FlagsItemKind::Flag(flag) => out.push(RegexFlag {
                flag: match flag {
                    ast::Flag::CaseInsensitive => RegexFlagName::CaseInsensitive,
                    ast::Flag::MultiLine => RegexFlagName::MultiLine,
                    ast::Flag::DotMatchesNewLine => RegexFlagName::DotMatchesNewLine,
                    ast::Flag::SwapGreed => RegexFlagName::SwapGreed,
                    ast::Flag::Unicode => RegexFlagName::Unicode,
                    ast::Flag::CRLF => RegexFlagName::Crlf,
                    ast::Flag::IgnoreWhitespace => RegexFlagName::IgnoreWhitespace,
                },
                enabled,
            }),
        }
    }
    out
}

#[cfg(test)]
#[path = "regex_tester_structure_tests.rs"]
mod tests;
