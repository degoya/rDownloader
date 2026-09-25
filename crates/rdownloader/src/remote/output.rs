//! Printing results: a table for people, JSON for scripts.

use anyhow::Result;
use serde::Serialize;

/// How a command reports its result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    /// Aligned columns, trimmed to what fits on a line.
    Text,
    /// The server's own JSON, unchanged.
    Json,
}

impl Format {
    #[must_use]
    pub const fn from_flag(json: bool) -> Self {
        if json { Self::Json } else { Self::Text }
    }
}

/// Prints a value as JSON, or hands it to `render` for the text form.
pub fn emit<T: Serialize>(format: Format, value: &T, render: impl FnOnce(&T)) -> Result<()> {
    match format {
        Format::Json => {
            println!("{}", serde_json::to_string_pretty(value)?);
        }
        Format::Text => render(value),
    }
    Ok(())
}

/// Prints rows in aligned columns.
///
/// Written here rather than pulled in as a dependency: the CLI has four tables, all of them
/// short, and column alignment is not worth a crate that has to be kept up to date.
pub fn table(headers: &[&str], rows: &[Vec<String>]) {
    if rows.is_empty() {
        println!("(nothing to show)");
        return;
    }
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| header.chars().count())
        .collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
    }
    let line = |cells: &[String]| {
        let rendered: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(index, cell)| {
                let width = widths.get(index).copied().unwrap_or(0);
                let padding = width.saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(padding))
            })
            .collect();
        println!("{}", rendered.join("  ").trim_end());
    };
    line(
        &headers
            .iter()
            .map(|header| (*header).to_owned())
            .collect::<Vec<_>>(),
    );
    for row in rows {
        line(row);
    }
}

/// Shortens a string to `width` characters, marking what was cut.
#[must_use]
pub fn shorten(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }
    let kept: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Bytes in a form a person reads at a glance.
#[must_use]
pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = value as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::{bytes, shorten};

    #[test]
    fn a_long_value_is_marked_as_cut() {
        assert_eq!(shorten("short", 10), "short");
        assert_eq!(shorten("abcdefghij", 5), "abcd…");
        // Multi-byte characters are counted as characters, not bytes, so a name with
        // umlauts is not cut mid-character.
        assert_eq!(shorten("aeiou-\u{fc}\u{fc}\u{fc}", 8), "aeiou-\u{fc}…");
    }

    #[test]
    fn byte_counts_read_at_a_glance() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
