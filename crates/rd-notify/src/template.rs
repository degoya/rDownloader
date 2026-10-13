//! A rule's own wording (RD-1280-02): text with `{placeholder}` holes and nothing else.
//!
//! * `{name}` names one [`Placeholder`]; `{{` and `}}` write a brace. No conditions, loops or
//!   expressions.
//! * A name the table lacks is refused when the rule is saved, with a stable code. A known one
//!   the event does not fill, or fills with nothing, becomes empty text.
//! * Values are data: their control characters become spaces, and a title stays on one line,
//!   because a mail's subject is a header.
//! * What comes out is cut to what every destination takes (Discord's 256-character title and
//!   2000-character message, ntfy's 4096 bytes); a part that comes out empty is sent in its
//!   built-in wording.
//!
//! The service fills a template when it queues the delivery, so a retry and a quiet-hours
//! summary carry the same text, and the webhook body keeps its structure: the template fills
//! only its `title` and `body`.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone};

use crate::{
    model::{NotificationEvent, NotificationRule, Severity},
    placeholder::Placeholder,
};

/// Longest title template a rule may hold, in characters.
pub const MAX_TITLE_TEMPLATE_CHARS: usize = 200;
/// Longest text template a rule may hold, in characters.
pub const MAX_BODY_TEMPLATE_CHARS: usize = 2000;
/// Longest filled title: Discord's embed title takes 256 characters.
pub const MAX_TITLE_CHARS: usize = 250;
/// Longest filled text: Discord's message takes 2000 characters, Telegram's 4096.
pub const MAX_BODY_CHARS: usize = 2000;
/// Longest filled text in bytes: ntfy takes a message of 4096 bytes.
pub const MAX_BODY_BYTES: usize = 4000;

/// Which of a rule's two templates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemplatePart {
    Title,
    Body,
}

impl TemplatePart {
    /// The request field it is checked under, for the error's `field` parameter.
    #[must_use]
    pub const fn field(self) -> &'static str {
        match self {
            Self::Title => "title_template",
            Self::Body => "body_template",
        }
    }

    const fn max_template_chars(self) -> usize {
        match self {
            Self::Title => MAX_TITLE_TEMPLATE_CHARS,
            Self::Body => MAX_BODY_TEMPLATE_CHARS,
        }
    }
}

/// Why a template was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TemplateError {
    /// A brace is not closed, not opened, or encloses nothing.
    Syntax,
    /// `{name}` names no placeholder.
    UnknownPlaceholder { name: String },
    /// Longer than its part allows.
    TooLong { max: usize },
}

impl TemplateError {
    /// The stable code the interface translates.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Syntax => "notification.template_syntax",
            Self::UnknownPlaceholder { .. } => "notification.template_unknown_placeholder",
            Self::TooLong { .. } => "notification.template_too_long",
        }
    }
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax => f.write_str(
                "a brace is not closed, not opened or encloses nothing; write {{ or }} for a brace",
            ),
            Self::UnknownPlaceholder { name } => write!(f, "`{{{name}}}` is no placeholder"),
            Self::TooLong { max } => write!(f, "the template is longer than {max} characters"),
        }
    }
}

impl std::error::Error for TemplateError {}

#[derive(Debug, Eq, PartialEq)]
enum Piece {
    Text(String),
    Field(Placeholder),
}

/// Checks one template of a rule without filling it.
///
/// # Errors
///
/// [`TemplateError`] for a template that is too long, an unbalanced or empty brace, or a name
/// the placeholder table lacks.
pub fn validate_template(part: TemplatePart, template: &str) -> Result<(), TemplateError> {
    let max = part.max_template_chars();
    if template.chars().count() > max {
        return Err(TemplateError::TooLong { max });
    }
    parse(template).map(|_| ())
}

fn parse(template: &str) -> Result<Vec<Piece>, TemplateError> {
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut chars = template.chars().peekable();
    while let Some(next) = chars.next() {
        match next {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                text.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                text.push('}');
            }
            '{' => {
                let mut name = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some('{') | None => return Err(TemplateError::Syntax),
                        Some(inner) => name.push(inner),
                    }
                }
                let name = name.trim();
                if name.is_empty() {
                    return Err(TemplateError::Syntax);
                }
                let field = Placeholder::from_name(name).ok_or_else(|| {
                    TemplateError::UnknownPlaceholder {
                        name: name.chars().take(40).collect(),
                    }
                })?;
                if !text.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                }
                pieces.push(Piece::Field(field));
            }
            '}' => return Err(TemplateError::Syntax),
            other => text.push(other),
        }
    }
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
    Ok(pieces)
}

/// What one occasion fills the placeholders with. A placeholder never set reads as empty.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Values(BTreeMap<Placeholder, String>);

impl Values {
    /// The event, its severity and the moment, as `HH:MM` and `YYYY-MM-DD` in `at`'s zone: the
    /// service passes its own (`chrono::Local`).
    #[must_use]
    pub fn stamped<Tz: TimeZone>(event: NotificationEvent, at: &DateTime<Tz>) -> Self
    where
        Tz::Offset: std::fmt::Display,
    {
        let severity = match event.severity() {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        Self::default()
            .with(Placeholder::Event, event.code())
            .with(Placeholder::Severity, severity)
            .with(Placeholder::Time, at.format("%H:%M").to_string())
            .with(Placeholder::Date, at.format("%Y-%m-%d").to_string())
    }

    /// Sets one value; an empty one is left unset, which reads the same.
    pub fn set(&mut self, placeholder: Placeholder, value: impl Into<String>) {
        let value = value.into();
        if value.is_empty() {
            self.0.remove(&placeholder);
        } else {
            self.0.insert(placeholder, value);
        }
    }

    /// [`Self::set`], chained.
    #[must_use]
    pub fn with(mut self, placeholder: Placeholder, value: impl Into<String>) -> Self {
        self.set(placeholder, value);
        self
    }

    /// Takes over every value `other` holds.
    pub fn extend(&mut self, other: &Self) {
        for (placeholder, value) in &other.0 {
            self.0.insert(*placeholder, value.clone());
        }
    }

    /// The value of one placeholder, empty when it has none.
    #[must_use]
    pub fn get(&self, placeholder: Placeholder) -> &str {
        self.0.get(&placeholder).map_or("", String::as_str)
    }
}

/// A byte count as the `{size}` placeholder writes it: `1.4 GiB`, `512 B`.
#[must_use]
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Fills `template`; `None` for one that does not parse, which a saved rule never holds.
#[must_use]
pub fn render(part: TemplatePart, template: &str, values: &Values) -> Option<String> {
    let mut out = String::new();
    for piece in parse(template).ok()? {
        match piece {
            Piece::Text(text) => out.push_str(&text),
            Piece::Field(field) => out.push_str(&one_line(values.get(field))),
        }
    }
    Some(match part {
        TemplatePart::Title => cut(one_line(&out).trim(), MAX_TITLE_CHARS, usize::MAX),
        TemplatePart::Body => cut(
            out.replace("\r\n", "\n").trim(),
            MAX_BODY_CHARS,
            MAX_BODY_BYTES,
        ),
    })
}

/// `text` with every control character a space: a value is data, and a title is one line.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// `text` cut to `max_chars` characters and `max_bytes` bytes, an ellipsis marking the cut.
fn cut(text: &str, max_chars: usize, max_bytes: usize) -> String {
    if text.chars().count() <= max_chars && text.len() <= max_bytes {
        return text.to_owned();
    }
    const ELLIPSIS: char = '\u{2026}';
    let mut out = String::new();
    for (count, character) in text.chars().enumerate() {
        if count + 1 >= max_chars
            || out.len() + character.len_utf8() + ELLIPSIS.len_utf8() > max_bytes
        {
            break;
        }
        out.push(character);
    }
    out.push(ELLIPSIS);
    out
}

impl NotificationRule {
    /// Whether the rule words its notifications itself.
    #[must_use]
    pub fn has_template(&self) -> bool {
        [&self.title_template, &self.body_template]
            .into_iter()
            .any(|template| {
                template
                    .as_deref()
                    .is_some_and(|text| !text.trim().is_empty())
            })
    }

    /// The title and the text this rule sends for an occasion whose built-in wording is `title`
    /// and `body`: its own templates filled with `values`, `{title}` and `{message}` being the
    /// built-in ones. A part without a template, or one that comes out empty, keeps the built-in
    /// wording.
    #[must_use]
    pub fn wording(&self, title: &str, body: &str, values: &Values) -> (String, String) {
        let mut values = values.clone();
        values.set(Placeholder::Title, title);
        values.set(Placeholder::Message, body);
        let fill = |part: TemplatePart, template: Option<&String>, built_in: &str| {
            template
                .filter(|text| !text.trim().is_empty())
                .and_then(|text| render(part, text, &values))
                .filter(|filled| !filled.is_empty())
                .unwrap_or_else(|| built_in.to_owned())
        };
        (
            fill(TemplatePart::Title, self.title_template.as_ref(), title),
            fill(TemplatePart::Body, self.body_template.as_ref(), body),
        )
    }
}

#[cfg(test)]
#[path = "template_tests.rs"]
mod tests;
