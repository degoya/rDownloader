use std::time::Duration;

use super::{PROGRESS_INTERVAL, ProgressThrottle, prepare, run_to_output};

#[tokio::test]
async fn a_tool_that_was_not_found_is_unsupported_and_names_itself() {
    let failure = prepare("yt-dlp", None, crate::Capability::MediaDownload)
        .await
        .expect_err("missing tool fails");
    // The three runners reported exactly this before the scaffolding was lifted; the
    // code and the parameter are what the web client translates and offers to install.
    assert_eq!(failure.code.as_deref(), Some("media.tool_missing"));
    assert!(matches!(
        failure.category,
        rd_core::FailureKind::Unsupported
    ));
    assert_eq!(
        failure.params.get("tool").map(String::as_str),
        Some("yt-dlp")
    );
    assert_eq!(failure.message, "yt-dlp is not installed or not configured");
}

#[tokio::test]
async fn a_binary_that_does_not_exist_is_a_spawn_failure_and_not_a_timeout() {
    let mut command = tokio::process::Command::new("rd-tools-no-such-binary-exists");
    let result = run_to_output(&mut command, Duration::from_secs(30)).await;
    // The two failures are handed back separately on purpose. rd-media reports a timeout
    // as `media.probe_timeout` and a failed spawn as `media.tool_error`, and rd-stream
    // gives them different contexts; collapsing them here would take that choice away.
    // A missing binary must also not sit out the full timeout before it is noticed.
    assert!(matches!(result, Ok(Err(_))));
}

#[cfg(unix)]
#[tokio::test]
async fn both_streams_are_captured_whatever_the_caller_configured() {
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "printf out; printf err >&2"]);
    // Set here deliberately, and ignored: `Command::output` pipes stdout and stderr
    // unconditionally. rd-stream's probe carried exactly this line and had stderr
    // captured for as long as it existed, which is why `run_to_output` offers no stdio
    // argument — it cannot honour one. If tokio ever stops overriding it, this fails.
    command.stderr(std::process::Stdio::null());
    let output = run_to_output(&mut command, Duration::from_secs(30))
        .await
        .expect("the tool finished inside the timeout")
        .expect("/bin/sh spawns");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "out");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "err");
}

/// Engine audit 1.8, finding 5: yt-dlp, gallery-dl and streamlink inherited the service's
/// whole environment. A tool now sees the allowlist and what its caller set, nothing else.
#[cfg(unix)]
#[tokio::test]
async fn a_tool_sees_the_allowlist_and_what_its_caller_set() {
    let mut command = tokio::process::Command::new("/usr/bin/env");
    command.env("RD_SET_BY_CALLER", "kept");
    let output = run_to_output(&mut command, Duration::from_secs(30))
        .await
        .expect("the tool finished inside the timeout")
        .expect("env spawns");
    let mut seen = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    seen.sort();
    let mut expected = rd_files::kept_variables(std::env::vars_os(), rd_files::TOOL_VARIABLES)
        .into_iter()
        .map(|(name, value)| format!("{}={}", name.to_string_lossy(), value.to_string_lossy()))
        .chain([
            "RD_SET_BY_CALLER=kept".to_owned(),
            // `speak_utf8`: the Python tools write UTF-8 to their pipes.
            "PYTHONIOENCODING=utf-8".to_owned(),
            "PYTHONUTF8=1".to_owned(),
        ])
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(seen, expected);
}

/// Engine audit 1.8, finding 4: a tool that writes far more to stderr than is kept neither
/// blocks on the pipe nor grows the service; the end, where the error is, survives.
#[cfg(unix)]
#[tokio::test]
async fn a_chatty_tool_keeps_only_the_end_of_its_stderr() {
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args([
        "-c",
        "i=0; while [ $i -lt 20000 ]; do echo \"WARNING: retrying segment $i\" >&2; \
         i=$((i+1)); done; echo 'ERROR: the last word' >&2",
    ]);
    let mut process =
        super::ToolProcess::spawn(&mut command, "chatty", super::Stdout::Discarded).expect("spawn");
    let status = tokio::time::timeout(Duration::from_secs(30), process.wait())
        .await
        .expect("the tool never blocked on a full pipe")
        .expect("wait");
    assert!(status.success());
    let stderr = process.stderr().await;
    assert!(
        stderr.len() <= super::STDERR_TAIL,
        "{} bytes kept",
        stderr.len()
    );
    assert!(stderr.ends_with("ERROR: the last word\n"), "{stderr:.200}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_tool_that_never_answers_is_a_timeout() {
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "sleep 30"]);
    let result = run_to_output(&mut command, Duration::from_millis(50)).await;
    // `kill_on_drop` is what keeps this from leaving a `sleep` behind: the timeout drops
    // the future that owns the child, and the child goes with it.
    assert!(result.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn a_line_that_is_not_utf8_is_read_lossily_and_the_run_goes_on() {
    // cp1252 bytes for "Ryoya \u{2013} ok" and a line after it: the strict reader failed the
    // whole download at the first one.
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "printf 'Ryoya \\226 ok\\r\\nnext\\n'"]);
    let mut process =
        super::ToolProcess::spawn(&mut command, "cp1252", super::Stdout::Read).expect("spawn");
    let pending = std::future::pending::<()>;
    assert_eq!(
        process.next_line(pending()).await.expect("first line"),
        super::ToolLine::Line("Ryoya \u{fffd} ok".to_owned())
    );
    assert_eq!(
        process.next_line(pending()).await.expect("second line"),
        super::ToolLine::Line("next".to_owned())
    );
    assert_eq!(
        process.next_line(pending()).await.expect("end"),
        super::ToolLine::End
    );
}

#[test]
fn a_fresh_throttle_is_due_and_stays_marked() {
    let mut throttle = ProgressThrottle::new(PROGRESS_INTERVAL);
    assert!(throttle.due());
    throttle.mark();
    assert!(!throttle.due());
}

/// Audit 2026-10-08, TR-05: a tool that stops talking is killed and reported, instead of
/// holding its slot until somebody stops the download by hand.
#[cfg(unix)]
#[tokio::test]
async fn a_tool_that_goes_silent_is_killed_at_its_silence_limit() {
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "echo first; exec sleep 30"]);
    let mut process = super::ToolProcess::spawn(&mut command, "silent", super::Stdout::Read)
        .expect("spawn")
        .with_silence_limit(Duration::from_millis(200));
    let pending = std::future::pending::<()>;
    assert_eq!(
        process.next_line(pending()).await.expect("first line"),
        super::ToolLine::Line("first".to_owned())
    );
    let answer = tokio::time::timeout(Duration::from_secs(10), process.next_line(pending()))
        .await
        .expect("the silence limit ended the wait")
        .expect("next line");
    assert_eq!(answer, super::ToolLine::TimedOut);
}

/// The silence limit is not a deadline: every line starts it again, so a run that keeps
/// talking outlives it many times over.
#[cfg(unix)]
#[tokio::test]
async fn a_tool_that_keeps_talking_outlives_its_silence_limit() {
    let mut command = tokio::process::Command::new("/bin/sh");
    command.args(["-c", "for i in 1 2 3 4 5 6; do echo $i; sleep 0.3; done"]);
    let mut process = super::ToolProcess::spawn(&mut command, "talking", super::Stdout::Read)
        .expect("spawn")
        .with_silence_limit(Duration::from_secs(1));
    let mut lines = Vec::new();
    loop {
        match process
            .next_line(std::future::pending())
            .await
            .expect("line")
        {
            super::ToolLine::Line(line) => lines.push(line),
            super::ToolLine::End => break,
            other => panic!("the run was cut short: {other:?}"),
        }
    }
    assert_eq!(lines, ["1", "2", "3", "4", "5", "6"]);
}
