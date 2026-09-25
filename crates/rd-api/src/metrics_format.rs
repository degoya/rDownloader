//! The Prometheus text exposition format, written by hand (RD-110-01).
//!
//! The format is small: a `# HELP` and a `# TYPE` line per family, then one line per series
//! of `name{label="value",...} number`, and `# EOF` at the end for OpenMetrics. Label values
//! escape backslash, double quote and newline, and that is the whole grammar this service
//! needs. A metrics crate would add a registry with a lifetime of its own, and nothing here
//! wants one: every scrape derives its figures from the queue, the scheduler and the
//! statistics tables at the moment it is asked.
//!
//! What this module deliberately cannot express is a label whose value is free text. A
//! [`Family`] takes its label values as `&str`, and the collector in `metrics.rs` only ever
//! hands it enum names, provider ids and storage-root ids; the exposition test in
//! `tests/metrics.rs` checks that nothing a person typed reaches the output.

use std::fmt::Write as _;

/// The kind of one metric family, as the `# TYPE` line names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MetricKind {
    Gauge,
    Counter,
    Histogram,
}

impl MetricKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Gauge => "gauge",
            Self::Counter => "counter",
            Self::Histogram => "histogram",
        }
    }
}

/// One series: label pairs and a value.
#[derive(Clone, Debug)]
pub(crate) struct Sample {
    pub labels: Vec<(&'static str, String)>,
    pub value: f64,
}

impl Sample {
    pub(crate) fn new(labels: &[(&'static str, &str)], value: f64) -> Self {
        Self {
            labels: labels
                .iter()
                .map(|(name, value)| (*name, (*value).to_owned()))
                .collect(),
            value,
        }
    }

    pub(crate) fn bare(value: f64) -> Self {
        Self {
            labels: Vec::new(),
            value,
        }
    }
}

/// A metric family: name, help, kind and its samples.
#[derive(Clone, Debug)]
pub(crate) struct Family {
    pub name: &'static str,
    pub help: &'static str,
    pub kind: MetricKind,
    pub samples: Vec<Sample>,
}

/// A histogram family from bucket upper bounds and the observations, without a metrics
/// crate: `_bucket{le}` lines are cumulative, `_sum` and `_count` close the family.
pub(crate) fn histogram(
    name: &'static str,
    help: &'static str,
    bounds: &[f64],
    observations: &[f64],
) -> Family {
    let mut samples = Vec::with_capacity(bounds.len() + 3);
    let mut sum = 0.0;
    for observation in observations {
        sum += observation;
    }
    for bound in bounds {
        let count = observations
            .iter()
            .filter(|value| **value <= *bound)
            .count();
        samples.push(Sample {
            labels: vec![("le", format_value(*bound))],
            value: count as f64,
        });
    }
    samples.push(Sample {
        labels: vec![("le", "+Inf".to_owned())],
        value: observations.len() as f64,
    });
    samples.push(Sample {
        labels: vec![("__suffix", "_sum".to_owned())],
        value: sum,
    });
    samples.push(Sample {
        labels: vec![("__suffix", "_count".to_owned())],
        value: observations.len() as f64,
    });
    Family {
        name,
        help,
        kind: MetricKind::Histogram,
        samples,
    }
}

/// Renders the families as one exposition, ending in `# EOF`.
pub(crate) fn render(families: &[Family]) -> String {
    let mut out = String::new();
    for family in families {
        let _ = writeln!(out, "# HELP {} {}", family.name, escape_help(family.help));
        let _ = writeln!(out, "# TYPE {} {}", family.name, family.kind.as_str());
        for sample in &family.samples {
            write_sample(&mut out, family, sample);
        }
    }
    out.push_str("# EOF\n");
    out
}

fn write_sample(out: &mut String, family: &Family, sample: &Sample) {
    let mut name = family.name.to_owned();
    let mut labels = Vec::new();
    for (label, value) in &sample.labels {
        if *label == "__suffix" {
            name.push_str(value);
        } else {
            labels.push((label, value));
        }
    }
    if family.kind == MetricKind::Histogram && labels.iter().any(|(label, _)| **label == "le") {
        name.push_str("_bucket");
    }
    out.push_str(&name);
    if !labels.is_empty() {
        out.push('{');
        for (index, (label, value)) in labels.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(out, "{label}=\"{}\"", escape_label(value));
        }
        out.push('}');
    }
    let _ = writeln!(out, " {}", format_value(sample.value));
}

/// A value as Prometheus reads it: integers without a fraction, everything else shortest.
fn format_value(value: f64) -> String {
    if value.is_infinite() {
        return if value > 0.0 { "+Inf" } else { "-Inf" }.to_owned();
    }
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn escape_help(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::{Family, MetricKind, Sample, histogram, render};

    /// The golden shape of one gauge family with and without labels.
    #[test]
    fn renders_the_exposition_format() {
        let families = [
            Family {
                name: "rdownloader_up",
                help: "Whether the service answers.",
                kind: MetricKind::Gauge,
                samples: vec![Sample::bare(1.0)],
            },
            Family {
                name: "rdownloader_queue_downloads",
                help: "Downloads by kind and state.",
                kind: MetricKind::Gauge,
                samples: vec![
                    Sample::new(&[("kind", "http"), ("state", "queued")], 3.0),
                    Sample::new(&[("kind", "usenet"), ("state", "downloading")], 1.0),
                ],
            },
            Family {
                name: "rdownloader_transfer_bytes_total",
                help: "Bytes moved.",
                kind: MetricKind::Counter,
                samples: vec![Sample::new(&[("kind", "http")], 1.5e9)],
            },
        ];
        assert_eq!(
            render(&families),
            "# HELP rdownloader_up Whether the service answers.\n\
             # TYPE rdownloader_up gauge\n\
             rdownloader_up 1\n\
             # HELP rdownloader_queue_downloads Downloads by kind and state.\n\
             # TYPE rdownloader_queue_downloads gauge\n\
             rdownloader_queue_downloads{kind=\"http\",state=\"queued\"} 3\n\
             rdownloader_queue_downloads{kind=\"usenet\",state=\"downloading\"} 1\n\
             # HELP rdownloader_transfer_bytes_total Bytes moved.\n\
             # TYPE rdownloader_transfer_bytes_total counter\n\
             rdownloader_transfer_bytes_total{kind=\"http\"} 1500000000\n\
             # EOF\n"
        );
    }

    /// Backslash, quote and newline are the three characters the format escapes.
    #[test]
    fn escapes_label_values_and_help() {
        let family = Family {
            name: "x",
            help: "line one\nline \\ two",
            kind: MetricKind::Gauge,
            samples: vec![Sample::new(&[("label", "a\"b\\c\nd")], 0.5)],
        };
        assert_eq!(
            render(&[family]),
            "# HELP x line one\\nline \\\\ two\n# TYPE x gauge\nx{label=\"a\\\"b\\\\c\\nd\"} 0.5\n# EOF\n"
        );
    }

    /// Buckets are cumulative, `+Inf` counts everything, sum and count close the family.
    #[test]
    fn histogram_buckets_are_cumulative() {
        let family = histogram(
            "wait",
            "Seconds waited.",
            &[10.0, 60.0],
            &[5.0, 30.0, 300.0],
        );
        assert_eq!(
            render(&[family]),
            "# HELP wait Seconds waited.\n\
             # TYPE wait histogram\n\
             wait_bucket{le=\"10\"} 1\n\
             wait_bucket{le=\"60\"} 2\n\
             wait_bucket{le=\"+Inf\"} 3\n\
             wait_sum 335\n\
             wait_count 3\n\
             # EOF\n"
        );
    }
}
