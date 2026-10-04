//! User post-processing scripts, invoked like SABnzbd scripts (positional arguments plus
//! `RD_*` and `SAB_*` environment variables), without a shell and without the rest of the
//! service's environment.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use rd_core::{PostprocessKind, PostprocessState};
#[cfg(windows)]
use rd_files::NoConsoleWindow as _;
use tokio::io::AsyncReadExt;

use crate::{
    Inner,
    steps::{Outcome, checkpoint, checkpoint_coded, codes},
};

const OUTPUT_LIMIT: usize = 64 * 1024;

/// The whole message of a run the standard library refused to start because it could not
/// escape an argument for a batch file (RD-150-08). A stable code, so the interface can say
/// it in the reader's language; `cmd.exe` would otherwise have read the argument as syntax.
pub const BATCH_ARGUMENTS_REFUSED: &str = "script.batch_arguments_refused";

/// What the script learns about the finished package.
#[derive(Clone, Debug)]
pub(crate) struct ScriptContext {
    pub package_id: String,
    pub package_name: String,
    pub final_dir: PathBuf,
    pub category: Option<String>,
    pub kind: String,
    /// 0 ok, 1 download/verification failed, 2 unpack failed, 3 PAR2 failed.
    pub status: u8,
}

/// Resolves a script name inside the scripts directory; rejects anything that is not a
/// regular file directly below it.
pub(crate) fn resolve_script(directory: &Path, name: &str) -> Result<PathBuf> {
    let valid = !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !valid {
        bail!("invalid script name");
    }
    let root = dunce::canonicalize(directory).context("scripts directory does not exist")?;
    let path = dunce::canonicalize(root.join(name)).context("script not found")?;
    if path.parent() != Some(root.as_path()) {
        bail!("script is outside the scripts directory");
    }
    if !std::fs::metadata(&path)?.is_file() {
        bail!("script is not a regular file");
    }
    Ok(path)
}

/// Builds the command: interpreters by extension, otherwise the file itself, in an environment
/// of its own.
///
/// Security review 2026-09-28, finding 7: a script used to inherit every variable the service
/// was started with. It now gets the allowlist of `rd_postprocess::restrict_environment` - the
/// search path, home, temporary directory, time zone, locale and the few Windows needs - and
/// the `RD_*`/`SAB_*` values its caller sets on top; nothing else of the service's environment.
pub(crate) fn command_for(script: &Path) -> tokio::process::Command {
    let mut command = program_for(script);
    rd_postprocess::restrict_environment(&mut command, &[]);
    command
}

/// The program and its leading arguments for `script`.
fn program_for(script: &Path) -> tokio::process::Command {
    let extension = script
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    #[cfg(windows)]
    {
        let mut command = match extension.as_str() {
            // Started as the program itself rather than through `cmd /C`: only then does the
            // standard library know it is a batch file, escape every argument for `cmd.exe`
            // and refuse one it cannot escape (RD-150-08). Through `cmd /C` an argument is
            // quoted for an ordinary program, and `cmd.exe` reads `&` or `%` in it as syntax.
            "bat" | "cmd" => tokio::process::Command::new(script),
            "ps1" => {
                let mut c = tokio::process::Command::new("powershell");
                c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                    .arg(script);
                c
            }
            "py" => {
                let mut c = tokio::process::Command::new("python");
                c.arg(script);
                c
            }
            _ => tokio::process::Command::new(script),
        };
        command.no_console_window();
        command
    }
    #[cfg(not(windows))]
    {
        let executable = std::fs::metadata(script)
            .map(|meta| {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o111 != 0
            })
            .unwrap_or(false);
        if executable {
            return tokio::process::Command::new(script);
        }
        match extension.as_str() {
            "py" => {
                let mut c = tokio::process::Command::new("python3");
                c.arg(script);
                c
            }
            "sh" => {
                let mut c = tokio::process::Command::new("sh");
                c.arg(script);
                c
            }
            _ => tokio::process::Command::new(script),
        }
    }
}

/// Whether `cmd.exe` runs the script, which parses its command line itself.
fn is_batch(script: &Path) -> bool {
    script
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
        })
}

/// A script that did not start, for [`execute`] and [`execute_for_output`] alike.
///
/// The standard library's refusal to build a batch file's command line becomes
/// [`BATCH_ARGUMENTS_REFUSED`] as the whole message; the script's own name cannot be the cause --
/// `resolve_script` admits no quote and no backslash. Anything else keeps `context`.
fn spawn_error(script: &Path, error: std::io::Error, context: &'static str) -> anyhow::Error {
    if is_batch(script) && error.kind() == std::io::ErrorKind::InvalidInput {
        anyhow::anyhow!(BATCH_ARGUMENTS_REFUSED)
    } else {
        anyhow::Error::new(error).context(context)
    }
}

/// SABnzbd-compatible positional arguments.
pub(crate) fn arguments(context: &ScriptContext) -> Vec<String> {
    vec![
        context.final_dir.to_string_lossy().into_owned(),
        context.package_name.clone(),
        rd_files::sanitize_file_name(&context.package_name),
        String::new(),
        context.category.clone().unwrap_or_default(),
        String::new(),
        context.status.to_string(),
    ]
}

pub(crate) fn environment(context: &ScriptContext, scripts_dir: &Path) -> Vec<(String, String)> {
    let dir = context.final_dir.to_string_lossy().into_owned();
    let clean = rd_files::sanitize_file_name(&context.package_name);
    let category = context.category.clone().unwrap_or_default();
    let status = context.status.to_string();
    let scripts = scripts_dir.to_string_lossy().into_owned();
    vec![
        ("RD_FINAL_DIR".into(), dir.clone()),
        ("RD_PACKAGE_NAME".into(), context.package_name.clone()),
        ("RD_CLEAN_NAME".into(), clean.clone()),
        ("RD_CATEGORY".into(), category.clone()),
        ("RD_STATUS".into(), status.clone()),
        ("RD_PACKAGE_ID".into(), context.package_id.clone()),
        ("RD_KIND".into(), context.kind.clone()),
        ("RD_SCRIPT_DIR".into(), scripts.clone()),
        ("SAB_COMPLETE_DIR".into(), dir),
        ("SAB_FINAL_NAME".into(), clean.clone()),
        ("SAB_FILENAME".into(), context.package_name.clone()),
        ("SAB_CAT".into(), category),
        ("SAB_PP_STATUS".into(), status),
        ("SAB_NZO_ID".into(), context.package_id.clone()),
        ("SAB_SCRIPT_DIR".into(), scripts),
    ]
}

/// Runs the script with a timeout, capturing the tail of its output. Returns `Ok(true)`
/// when it exited with status 0.
pub(crate) async fn execute(
    script: &Path,
    scripts_dir: &Path,
    context: &ScriptContext,
    timeout: Duration,
) -> Result<(bool, String)> {
    let mut command = command_for(script);
    command
        .args(arguments(context))
        .envs(environment(context, scripts_dir))
        .current_dir(&context.final_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| spawn_error(script, error, "spawn post-processing script"))?;
    let stdout = child.stdout.take().context("script stdout")?;
    let stderr = child.stderr.take().context("script stderr")?;
    // Only the tail is shown, so only the tail is kept - one byte past the limit, so the cut
    // below still knows it has to mark one. Reading everything first let a script that logs
    // without end grow the service with it.
    let capture = async {
        tokio::join!(
            rd_files::read_tail(stdout, OUTPUT_LIMIT + 1),
            rd_files::read_tail(stderr, OUTPUT_LIMIT + 1)
        )
    };
    let result = tokio::time::timeout(timeout, async {
        let (out, err) = capture.await;
        let status = child.wait().await?;
        Ok::<_, anyhow::Error>((status, out, err))
    })
    .await;
    match result {
        Ok(Ok((status, out, err))) => {
            let mut text = String::from_utf8_lossy(&out).into_owned();
            if !err.is_empty() {
                text.push_str("\n[stderr]\n");
                text.push_str(&String::from_utf8_lossy(&err));
            }
            let tail: String = if text.len() > OUTPUT_LIMIT {
                let start = text.len() - OUTPUT_LIMIT;
                let boundary = text.ceil_char_boundary(start);
                format!("…{}", &text[boundary..])
            } else {
                text
            };
            let ok = status.success();
            let summary = if ok {
                tail
            } else {
                format!("exit status {}\n{tail}", status.code().unwrap_or(-1))
            };
            Ok((ok, summary))
        }
        Ok(Err(error)) => Err(error),
        Err(_) => {
            let _ = child.kill().await;
            bail!("script timed out after {} seconds", timeout.as_secs())
        }
    }
}

/// Longest excerpt of a failed output script's standard error kept in its failure reason.
const REASON_LIMIT: usize = 300;

/// Runs a script whose standard output is data rather than a log (RD-130-19), and returns
/// all of it.
///
/// [`execute`] keeps the *tail* of a post-processing script's output, because that is where a
/// log explains itself. A script that prints links is the opposite case: every line counts
/// and a cut list is indistinguishable from a whole one. So the same limit becomes a refusal
/// here -- one byte more than [`OUTPUT_LIMIT`] fails the run, and the script is killed rather
/// than drained -- and a non-zero exit fails it too, with the end of its standard error as the
/// reason. Standard error is read on its own task, bounded, so a script that fills it can
/// neither block on a full pipe nor grow this process without limit.
pub(crate) async fn execute_for_output(
    script: &Path,
    scripts_dir: &Path,
    arguments: &[String],
    environment: Vec<(String, String)>,
    timeout: Duration,
) -> Result<String> {
    let mut command = command_for(script);
    command
        .args(arguments)
        .envs(environment)
        .current_dir(scripts_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| spawn_error(script, error, "spawn script"))?;
    let stdout = child.stdout.take().context("script stdout")?;
    let stderr = child.stderr.take().context("script stderr")?;
    let errors = tokio::spawn(async move {
        let mut kept = Vec::new();
        let mut limited = stderr.take(OUTPUT_LIMIT as u64);
        let _ = limited.read_to_end(&mut kept).await;
        // Whatever is left is read and dropped, so the script never blocks on a full pipe.
        let _ = tokio::io::copy(&mut limited.into_inner(), &mut tokio::io::sink()).await;
        kept
    });
    let result = tokio::time::timeout(timeout, async {
        let mut out = Vec::new();
        // One byte past the limit is all it takes to know the limit was passed.
        stdout
            .take(OUTPUT_LIMIT as u64 + 1)
            .read_to_end(&mut out)
            .await
            .context("read script output")?;
        if out.len() > OUTPUT_LIMIT {
            let _ = child.kill().await;
            bail!("script printed more than {OUTPUT_LIMIT} bytes");
        }
        let status = child.wait().await?;
        Ok::<_, anyhow::Error>((status, out))
    })
    .await;
    match result {
        Ok(Ok((status, out))) if status.success() => Ok(String::from_utf8_lossy(&out).into_owned()),
        Ok(Ok((status, _))) => {
            let errors = errors.await.unwrap_or_default();
            let reason = failure_reason(&String::from_utf8_lossy(&errors));
            match status.code() {
                Some(code) if reason.is_empty() => bail!("script exited with status {code}"),
                Some(code) => bail!("script exited with status {code}: {reason}"),
                None => bail!("script was terminated before it finished"),
            }
        }
        Ok(Err(error)) => Err(error),
        Err(_) => {
            let _ = child.kill().await;
            bail!("script timed out after {} seconds", timeout.as_secs())
        }
    }
}

/// The last lines of a failed script's standard error, short enough for a run's history.
fn failure_reason(errors: &str) -> String {
    let errors = errors.trim();
    if errors.len() <= REASON_LIMIT {
        return errors.to_owned();
    }
    let start = errors.ceil_char_boundary(errors.len() - REASON_LIMIT);
    format!("…{}", &errors[start..])
}

pub(crate) async fn run(
    inner: &Inner,
    owner: &str,
    scripts_dir: &Path,
    name: &str,
    context: &ScriptContext,
    timeout: Duration,
) -> Result<bool> {
    crate::steps::stage(
        inner,
        owner,
        rd_core::PostprocessStage::Script,
        Some(name.to_owned()),
    )
    .await?;
    checkpoint(
        inner,
        owner,
        PostprocessKind::Script,
        name,
        PostprocessState::Running,
        None,
        None,
    )
    .await?;
    let outcome = match resolve_script(scripts_dir, name) {
        Ok(path) => execute(&path, scripts_dir, context, timeout).await,
        Err(error) => Err(error),
    };
    let (state, ok, outcome) = match outcome {
        Ok((true, output)) => (
            PostprocessState::Completed,
            true,
            Outcome::detailed(codes::SCRIPT_SUCCEEDED, output),
        ),
        Ok((false, output)) => (
            PostprocessState::Failed,
            false,
            Outcome::detailed(codes::SCRIPT_FAILED, output),
        ),
        Err(error) => (
            PostprocessState::Failed,
            false,
            Outcome::detailed(codes::SCRIPT_FAILED, error.to_string()),
        ),
    };
    checkpoint_coded(
        inner,
        owner,
        PostprocessKind::Script,
        name,
        state,
        None,
        outcome,
    )
    .await?;
    Ok(ok)
}

#[cfg(test)]
mod tests {
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
}
