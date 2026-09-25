//! A very small JSON reader, because the answer this plugin reads is nested.
//!
//! The SponsorBlock enricher scans its answer with `split`, which is honest there: the body is
//! a flat array of two-field objects. This one is not. A catalogue entry carries arrays inside
//! objects inside an array, and a scan that splits on `{` cannot tell which `name` belongs to
//! which entry. Two hundred lines of parser are cheaper than being subtly wrong about which
//! film a row describes, and a guest cannot pull `serde_json` in without pulling a great deal
//! more with it.
//!
//! Neither a general-purpose parser nor a fast one: it refuses input that is too long or too
//! deeply nested rather than growing to meet it, which is the right answer to a body that
//! arrived from outside.

/// Longest body this will look at. Above this the answer is not one of ours.
///
/// The same number the manifest caps the response at, on purpose: reading is done over a
/// `Vec<char>`, so a body four times this size in memory is the worst case the sandbox's
/// 32 MiB has to hold, and neither limit can quietly outgrow the other.
const MAX_INPUT: usize = 524_288;

/// Deepest nesting accepted. Cinemeta answers nest four or five levels; anything near this
/// limit is somebody probing what the parser does rather than a catalogue entry.
const MAX_DEPTH: usize = 32;

/// One JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    Array(Vec<Json>),
    /// Kept as pairs in document order. Objects here have a handful of keys, so looking one up
    /// by walking them is cheaper than any map would be.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// The value under `key`, if this is an object that has one.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(entries) => entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The value under `key` as text a person could read.
    ///
    /// A number is rendered rather than refused: services are inconsistent about whether a
    /// rating or a year arrives quoted, and a field that vanishes because the source changed
    /// `"2010"` into `2010` would be a defect nobody could see from the outside.
    #[must_use]
    pub fn text_field(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            Self::Text(text) => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_owned())
            }
            Self::Number(number) => Some(render_number(*number)),
            _ => None,
        }
    }

    /// The first `count` entries of a string array under `key`, joined with `", "`.
    #[must_use]
    pub fn text_list(&self, key: &str, count: usize) -> Option<String> {
        let items = self.get(key)?.as_array()?;
        let joined = items
            .iter()
            .filter_map(Self::as_text)
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .take(count)
            .collect::<Vec<_>>()
            .join(", ");
        (!joined.is_empty()).then_some(joined)
    }
}

/// A number without the trailing zeros a float round-trip would add: `2010`, not `2010.0`.
fn render_number(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        (number as i64).to_string()
    } else {
        format!("{number}")
    }
}

/// Reads one JSON document, or nothing at all.
///
/// Nothing is the answer to anything malformed, over-long or over-nested. The caller's job is
/// then to leave the row as it found it, which is what a source talking nonsense deserves.
#[must_use]
pub fn parse(input: &str) -> Option<Json> {
    if input.len() > MAX_INPUT {
        return None;
    }
    let mut reader = Reader {
        chars: input.chars().collect(),
        at: 0,
    };
    reader.skip_space();
    let value = reader.value(0)?;
    reader.skip_space();
    reader.done().then_some(value)
}

struct Reader {
    chars: Vec<char>,
    at: usize,
}

impl Reader {
    fn done(&self) -> bool {
        self.at >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn take(&mut self) -> Option<char> {
        let next = self.peek()?;
        self.at += 1;
        Some(next)
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn literal(&mut self, word: &str) -> bool {
        if self.chars[self.at..].starts_with(&word.chars().collect::<Vec<_>>()[..]) {
            self.at += word.chars().count();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.peek()? {
            '{' => self.object(depth),
            '[' => self.array(depth),
            '"' => self.text().map(Json::Text),
            't' => self.literal("true").then_some(Json::Bool(true)),
            'f' => self.literal("false").then_some(Json::Bool(false)),
            'n' => self.literal("null").then_some(Json::Null),
            _ => self.number(),
        }
    }

    fn object(&mut self, depth: usize) -> Option<Json> {
        self.take()?; // '{'
        let mut entries = Vec::new();
        self.skip_space();
        if self.peek()? == '}' {
            self.at += 1;
            return Some(Json::Object(entries));
        }
        loop {
            self.skip_space();
            let key = self.text()?;
            self.skip_space();
            (self.take()? == ':').then_some(())?;
            self.skip_space();
            let value = self.value(depth + 1)?;
            entries.push((key, value));
            self.skip_space();
            match self.take()? {
                ',' => {}
                '}' => return Some(Json::Object(entries)),
                _ => return None,
            }
        }
    }

    fn array(&mut self, depth: usize) -> Option<Json> {
        self.take()?; // '['
        let mut items = Vec::new();
        self.skip_space();
        if self.peek()? == ']' {
            self.at += 1;
            return Some(Json::Array(items));
        }
        loop {
            self.skip_space();
            items.push(self.value(depth + 1)?);
            self.skip_space();
            match self.take()? {
                ',' => {}
                ']' => return Some(Json::Array(items)),
                _ => return None,
            }
        }
    }

    fn text(&mut self) -> Option<String> {
        (self.take()? == '"').then_some(())?;
        let mut out = String::new();
        loop {
            match self.take()? {
                '"' => return Some(out),
                '\\' => out.push(self.escape()?),
                c if (c as u32) < 0x20 => return None,
                c => out.push(c),
            }
        }
    }

    fn escape(&mut self) -> Option<char> {
        Some(match self.take()? {
            '"' => '"',
            '\\' => '\\',
            '/' => '/',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'u' => return self.unicode_escape(),
            _ => return None,
        })
    }

    /// A `\uXXXX` escape, including the surrogate pair that carries anything above the BMP.
    ///
    /// A lone or malformed surrogate becomes the replacement character rather than failing the
    /// parse: the field it sits in is a film title, and a title with one odd glyph is still a
    /// better answer than no title.
    fn unicode_escape(&mut self) -> Option<char> {
        let first = self.hex4()?;
        if !(0xD800..0xDC00).contains(&first) {
            return Some(char::from_u32(first).unwrap_or('\u{fffd}'));
        }
        if self.peek() != Some('\\') {
            return Some('\u{fffd}');
        }
        self.at += 1;
        if self.take() != Some('u') {
            return Some('\u{fffd}');
        }
        let second = self.hex4()?;
        if !(0xDC00..0xE000).contains(&second) {
            return Some('\u{fffd}');
        }
        let combined = 0x1_0000 + ((first - 0xD800) << 10) + (second - 0xDC00);
        Some(char::from_u32(combined).unwrap_or('\u{fffd}'))
    }

    fn hex4(&mut self) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..4 {
            value = value * 16 + self.take()?.to_digit(16)?;
        }
        Some(value)
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.at;
        if self.peek()? == '-' {
            self.at += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
        {
            self.at += 1;
        }
        let text: String = self.chars[start..self.at].iter().collect();
        text.parse().ok().map(Json::Number)
    }
}

#[cfg(test)]
mod tests {
    use super::{Json, parse};

    #[test]
    fn a_nested_answer_keeps_each_field_with_its_own_entry() {
        // The reason this parser exists: two entries, each with an array inside, and the
        // second entry's name must not be readable as the first one's.
        const BODY: &str = r#"{"metas":[
            {"name":"First","genres":["Action","Crime"],"imdbRating":"8.1"},
            {"name":"Second","genres":["Drama"],"imdbRating":"6.0"}
        ]}"#;
        let document = parse(BODY).expect("parses");
        let metas = document
            .get("metas")
            .and_then(Json::as_array)
            .expect("array");
        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].text_field("name").as_deref(), Some("First"));
        assert_eq!(
            metas[0].text_list("genres", 3).as_deref(),
            Some("Action, Crime")
        );
        assert_eq!(metas[1].text_field("imdbRating").as_deref(), Some("6.0"));
    }

    #[test]
    fn a_number_reads_the_same_as_the_quoted_form() {
        // Services are inconsistent about quoting a year or a rating, and a field that
        // vanished because of that would be invisible from the outside.
        let quoted = parse(r#"{"year":"2010","rating":"8.8"}"#).expect("parses");
        let bare = parse(r#"{"year":2010,"rating":8.8}"#).expect("parses");
        assert_eq!(quoted.text_field("year"), bare.text_field("year"));
        assert_eq!(bare.text_field("year").as_deref(), Some("2010"));
        assert_eq!(bare.text_field("rating").as_deref(), Some("8.8"));
    }

    #[test]
    fn nonsense_is_nothing_rather_than_a_guess() {
        for body in [
            "",
            "{",
            "[1,2",
            r#"{"a":}"#,
            r#"{"a":1}trailing"#,
            "not json at all",
        ] {
            assert_eq!(parse(body), None, "{body} must not parse");
        }
    }

    #[test]
    fn nesting_deep_enough_to_be_an_attack_is_refused() {
        let body = format!("{}1{}", "[".repeat(200), "]".repeat(200));
        assert_eq!(parse(&body), None);
    }

    #[test]
    fn escapes_survive_the_way_a_title_needs_them_to() {
        let document = parse(r#"{"name":"Ocean’s \"Eleven\"!🎬"}"#).expect("parses");
        assert_eq!(
            document.text_field("name").as_deref(),
            Some("Ocean\u{2019}s \"Eleven\"!\u{1f3ac}")
        );
    }

    #[test]
    fn an_empty_or_missing_field_is_absent_rather_than_blank() {
        let document = parse(r#"{"name":"   ","genres":[],"other":null}"#).expect("parses");
        assert_eq!(document.text_field("name"), None);
        assert_eq!(document.text_list("genres", 3), None);
        assert_eq!(document.text_field("other"), None);
        assert_eq!(document.text_field("absent"), None);
    }
}
