use super::{ScriptContext, arguments, failure_reason, resolve_script, spawn_error};

fn context(dir: &std::path::Path) -> ScriptContext {
    ScriptContext {
        package_id: "pkg-1".to_owned(),
        package_name: "My Release".to_owned(),
        final_dir: dir.to_owned(),
        category: Some("movies".to_owned()),
        kind: "http".to_owned(),
        status: 2,
    }
}

#[test]
fn rejects_traversal_and_missing_scripts() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("ok.sh"), "#!/bin/sh\n").expect("script");
    assert!(resolve_script(temp.path(), "../ok.sh").is_err());
    assert!(resolve_script(temp.path(), "missing.sh").is_err());
    assert!(resolve_script(temp.path(), "ok.sh").is_ok());
}

#[test]
fn arguments_follow_sabnzbd_order() {
    let args = arguments(&context(std::path::Path::new("/pkg")));
    assert_eq!(args.len(), 7);
    assert_eq!(args[1], "My Release");
    assert_eq!(args[4], "movies");
    assert_eq!(args[6], "2");
}

#[test]
fn a_long_standard_error_is_cut_to_its_end() {
    assert_eq!(failure_reason("  nothing found\n"), "nothing found");
    let long = format!("{}the real reason", "x".repeat(1_000));
    let reason = failure_reason(&long);
    assert!(reason.starts_with('…') && reason.ends_with("the real reason"));
    assert!(
        reason.chars().count() <= super::REASON_LIMIT + 1,
        "{reason}"
    );
}

/// Security review 2026-09-28, finding 8: a post-processing script is refused with the same
/// code as an output script, not with a generic spawn failure. Only a batch file's
/// `InvalidInput` is that refusal.
#[test]
fn a_refused_batch_command_line_is_the_stable_code_for_every_script_kind() {
    use std::{io, path::Path};

    let refused = || io::Error::from(io::ErrorKind::InvalidInput);
    for batch in ["run.bat", "RUN.CMD"] {
        assert_eq!(
            spawn_error(Path::new(batch), refused(), "spawn script").to_string(),
            super::BATCH_ARGUMENTS_REFUSED
        );
    }
    let other = spawn_error(Path::new("run.sh"), refused(), "spawn script");
    assert_eq!(other.to_string(), "spawn script");
    let missing = spawn_error(
        Path::new("run.bat"),
        io::Error::from(io::ErrorKind::NotFound),
        "spawn post-processing script",
    );
    assert_eq!(missing.to_string(), "spawn post-processing script");
}

/// RD-130-19: a script whose output is data -- every line, the exit code and both limits.
#[cfg(unix)]
#[tokio::test]
async fn an_output_script_returns_all_of_its_output_or_fails_with_a_reason() {
    use std::time::Duration;

    use super::{OUTPUT_LIMIT, execute_for_output};

    let temp = tempfile::tempdir().expect("tempdir");
    let scripts = temp.path();
    let run = |name: &'static str| {
        let script = scripts.join(name);
        async move {
            execute_for_output(
                &script,
                scripts,
                &[],
                vec![("RD_KIND".to_owned(), "subscription".to_owned())],
                Duration::from_secs(10),
            )
            .await
        }
    };
    // More lines than the tail a post-processing log keeps would hold at the front, and
    // the environment and working directory a link script is promised.
    std::fs::write(
        scripts.join("links.sh"),
        "echo \"kind=$RD_KIND dir=$(pwd)\"\n\
         i=0; while [ $i -lt 2000 ]; do echo \"https://example.test/$i.rar\"; i=$((i+1)); done\n\
         echo noise >&2\n",
    )
    .expect("script");
    let output = run("links.sh").await.expect("run");
    let first = output.lines().next().expect("first line");
    assert!(first.starts_with("kind=subscription dir="), "{first}");
    assert!(output.contains("https://example.test/0.rar\n"));
    assert!(output.contains("https://example.test/1999.rar\n"));
    assert!(!output.contains("noise"), "standard error is not output");

    std::fs::write(
        scripts.join("fails.sh"),
        "echo https://example.test/half.rar\necho 'login refused' >&2\nexit 3\n",
    )
    .expect("script");
    let error = run("fails.sh").await.expect_err("non-zero exit");
    assert_eq!(
        error.to_string(),
        "script exited with status 3: login refused"
    );

    std::fs::write(
        scripts.join("chatty.sh"),
        format!("head -c {} /dev/zero | tr '\\0' 'a'\n", OUTPUT_LIMIT + 1),
    )
    .expect("script");
    let error = run("chatty.sh").await.expect_err("too much output");
    assert!(error.to_string().contains("more than"), "{error}");

    // A script that never stops printing is killed at the limit, not at the timeout.
    std::fs::write(scripts.join("endless.sh"), "yes https://example.test/x\n").expect("script");
    let started = std::time::Instant::now();
    let error = run("endless.sh").await.expect_err("endless output");
    assert!(error.to_string().contains("more than"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));

    std::fs::write(scripts.join("slow.sh"), "sleep 30\n").expect("script");
    let started = std::time::Instant::now();
    let error = execute_for_output(
        &scripts.join("slow.sh"),
        scripts,
        &[],
        Vec::new(),
        Duration::from_millis(300),
    )
    .await
    .expect_err("timeout");
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// RD-150-08: every argument arrives whole and as it was written -- a space, `&`, `;`, a
/// `$` and an empty one included -- because no shell stands between them and the script.
#[cfg(unix)]
#[tokio::test]
async fn an_output_script_gets_each_argument_whole() {
    use std::time::Duration;

    use super::execute_for_output;

    let temp = tempfile::tempdir().expect("tempdir");
    let scripts = temp.path();
    std::fs::write(
        scripts.join("args.sh"),
        "printf '%s\\n' \"count=$#\"\nfor a in \"$@\"; do printf '[%s]\\n' \"$a\"; done\n",
    )
    .expect("script");
    let arguments = [
        "--since",
        "two words",
        "a&b;c",
        "$HOME `id` > out",
        "it's \"quoted\"",
        "",
    ]
    .map(str::to_owned);
    let output = execute_for_output(
        &scripts.join("args.sh"),
        scripts,
        &arguments,
        Vec::new(),
        Duration::from_secs(10),
    )
    .await
    .expect("run");
    assert_eq!(
        output,
        "count=6\n[--since]\n[two words]\n[a&b;c]\n[$HOME `id` > out]\n\
         [it's \"quoted\"]\n[]\n"
    );
    assert!(!scripts.join("out").exists(), "nothing was redirected");
}

/// RD-150-08 on Windows: a batch file gets each argument as one, quoted where `cmd.exe`
/// would otherwise read it as syntax, and one the standard library cannot escape is refused
/// with the stable code rather than handed to `cmd.exe`.
#[cfg(windows)]
#[tokio::test]
async fn a_batch_file_gets_each_argument_whole_or_is_refused() {
    use std::time::Duration;

    use super::{BATCH_ARGUMENTS_REFUSED, execute_for_output};

    let temp = tempfile::tempdir().expect("tempdir");
    let scripts = temp.path();
    std::fs::write(
        scripts.join("args.bat"),
        "@echo off\r\necho 1=%1\r\necho 2=%2\r\necho 3=%3\r\necho 4=%4\r\n",
    )
    .expect("script");
    let run = |arguments: Vec<String>| {
        let script = scripts.join("args.bat");
        async move {
            execute_for_output(
                &script,
                scripts,
                &arguments,
                Vec::new(),
                Duration::from_secs(20),
            )
            .await
        }
    };
    let output = run(["plain", "two words", "a&b;c"].map(str::to_owned).to_vec())
        .await
        .expect("run");
    let lines: Vec<&str> = output.lines().map(str::trim_end).collect();
    assert_eq!(
        lines,
        ["1=plain", "2=\"two words\"", "3=\"a&b;c\"", "4="],
        "{output}"
    );

    let error = run(vec!["line\nbreak".to_owned()])
        .await
        .expect_err("a line break cannot be escaped for cmd.exe");
    assert_eq!(error.to_string(), BATCH_ARGUMENTS_REFUSED);

    // The post-processing run refuses the same way: its arguments carry the package name.
    let mut refused = context(scripts);
    refused.package_name = "line\nbreak".to_owned();
    let error = super::execute(
        &scripts.join("args.bat"),
        scripts,
        &refused,
        Duration::from_secs(20),
    )
    .await
    .expect_err("a line break cannot be escaped for cmd.exe");
    assert_eq!(error.to_string(), BATCH_ARGUMENTS_REFUSED);
}

#[cfg(unix)]
#[tokio::test]
async fn runs_script_with_environment_and_kills_on_timeout() {
    // Imported here rather than at the module head: this is the only test that runs a
    // script, and it is unix-only, so on Windows a module-level import would be unused.
    use std::time::Duration;

    use super::execute;

    let temp = tempfile::tempdir().expect("tempdir");
    let scripts = temp.path().join("scripts");
    let pkg = temp.path().join("pkg");
    std::fs::create_dir_all(&scripts).expect("scripts");
    std::fs::create_dir_all(&pkg).expect("pkg");
    std::fs::write(
        scripts.join("echo_env.sh"),
        "echo \"dir=$RD_FINAL_DIR status=$SAB_PP_STATUS cat=$5\"\necho oops >&2\nexit 3\n",
    )
    .expect("script");
    std::fs::write(scripts.join("sleep.sh"), "sleep 30\n").expect("script");
    let (ok, output) = execute(
        &scripts.join("echo_env.sh"),
        &scripts,
        &context(&pkg),
        Duration::from_secs(10),
    )
    .await
    .expect("run");
    assert!(!ok);
    assert!(output.contains("exit status 3"), "{output}");
    assert!(
        output.contains(&format!("dir={}", pkg.display())),
        "{output}"
    );
    assert!(output.contains("status=2 cat=movies"), "{output}");
    assert!(output.contains("[stderr]\noops"), "{output}");
    let started = std::time::Instant::now();
    let error = execute(
        &scripts.join("sleep.sh"),
        &scripts,
        &context(&pkg),
        Duration::from_millis(300),
    )
    .await
    .expect_err("timeout");
    assert!(error.to_string().contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// Engine audit 1.8, finding 4: a script that logs far past the limit keeps only the end of
/// its output, and is drained while it runs, so it finishes instead of blocking on the pipe.
#[cfg(unix)]
#[tokio::test]
async fn a_chatty_script_keeps_only_the_end_of_its_output() {
    use std::time::Duration;

    use super::{OUTPUT_LIMIT, execute};

    let temp = tempfile::tempdir().expect("tempdir");
    let scripts = temp.path().join("scripts");
    let pkg = temp.path().join("pkg");
    std::fs::create_dir_all(&scripts).expect("scripts");
    std::fs::create_dir_all(&pkg).expect("pkg");
    std::fs::write(
        scripts.join("chatty.sh"),
        "i=0\nwhile [ $i -lt 20000 ]; do echo \"line $i of a long log\"; i=$((i+1)); done\n\
         echo 'the last word'\n",
    )
    .expect("script");
    let (ok, output) = execute(
        &scripts.join("chatty.sh"),
        &scripts,
        &context(&pkg),
        Duration::from_secs(30),
    )
    .await
    .expect("run");
    assert!(ok);
    assert!(output.starts_with('\u{2026}'), "the cut is marked");
    assert!(output.len() <= OUTPUT_LIMIT + '\u{2026}'.len_utf8());
    assert!(output.ends_with("the last word\n"), "{output:.200}");
}
