//! Parser for `yt-dlp --newline` progress output.

/// One `[download]` progress sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadProgressLine {
    pub percent_tenths: u32,
    pub total_bytes: Option<u64>,
}

/// Parses lines such as `[download]  12.3% of  100.00MiB at 2.00MiB/s ETA 00:40` or
/// `[download]  45.0% of ~  52.10MiB …`; other lines yield `None`.
#[must_use]
pub fn parse_progress_line(line: &str) -> Option<DownloadProgressLine> {
    let rest = line.trim().strip_prefix("[download]")?.trim_start();
    let (percent_text, rest) = rest.split_once('%')?;
    let percent: f64 = percent_text.trim().parse().ok()?;
    if !(0.0..=100.0).contains(&percent) {
        return None;
    }
    let total_bytes = rest
        .trim_start()
        .strip_prefix("of")
        .map(|after| after.trim_start().trim_start_matches('~').trim_start())
        .and_then(|after| after.split_whitespace().next())
        .and_then(parse_size);
    Some(DownloadProgressLine {
        percent_tenths: (percent * 10.0).round() as u32,
        total_bytes,
    })
}

fn parse_size(token: &str) -> Option<u64> {
    let digits_end = token
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(token.len());
    let value: f64 = token[..digits_end].parse().ok()?;
    let unit = token[digits_end..].trim();
    let factor: f64 = match unit {
        "B" | "" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        "KB" => 1000.0,
        "MB" => 1_000_000.0,
        "GB" => 1_000_000_000.0,
        _ => return None,
    };
    Some((value * factor).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::{DownloadProgressLine, parse_progress_line};

    #[test]
    fn parses_regular_and_estimated_totals() {
        assert_eq!(
            parse_progress_line("[download]  12.3% of  100.00MiB at 2.00MiB/s ETA 00:40"),
            Some(DownloadProgressLine {
                percent_tenths: 123,
                total_bytes: Some(104_857_600)
            })
        );
        assert_eq!(
            parse_progress_line("[download]  45.0% of ~  52.10MiB at Unknown speed"),
            Some(DownloadProgressLine {
                percent_tenths: 450,
                total_bytes: Some(54_630_810)
            })
        );
        assert_eq!(
            parse_progress_line("[download] 100% of 3.00KiB in 00:00"),
            Some(DownloadProgressLine {
                percent_tenths: 1000,
                total_bytes: Some(3072)
            })
        );
        assert_eq!(parse_progress_line("[info] Downloading format"), None);
        assert_eq!(parse_progress_line("[download] Destination: x.mp4"), None);
    }
}
