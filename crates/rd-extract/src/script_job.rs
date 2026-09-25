//! User post-processing scripts, invoked like SABnzbd scripts (positional arguments plus
//! `RD_*` and `SAB_*` environment variables), without a shell.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use rd_core::{PostprocessKind, PostprocessState};
use tokio::io::AsyncReadExt;

use crate::{
    Inner,
    steps::{checkpoint, truncate},
};

const OUTPUT_LIMIT: usize = 64 * 1024;

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

/// Builds the command: interpreters by extension, otherwise the file itself.
pub(crate) fn command_for(script: &Path) -> tokio::process::Command {
    let extension = script
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    #[cfg(windows)]
    {
        let mut command = match extension.as_str() {
            "bat" | "cmd" => {
                let mut c = tokio::process::Command::new("cmd");
                c.arg("/C").arg(script);
                c
            }
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
        command.creation_flags(0x0800_0000);
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
    let mut child = command.spawn().context("spawn post-processing script")?;
    let mut stdout = child.stdout.take().context("script stdout")?;
    let mut stderr = child.stderr.take().context("script stderr")?;
    let capture = async {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let _ = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        (out, err)
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
    environment: Vec<(String, String)>,
    timeout: Duration,
) -> Result<String> {
    let mut command = command_for(script);
    command
        .envs(environment)
        .current_dir(scripts_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().context("spawn script")?;
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
    let (state, ok, message) = match outcome {
        Ok((true, output)) => (PostprocessState::Completed, true, output),
        Ok((false, output)) => (PostprocessState::Failed, false, output),
        Err(error) => (PostprocessState::Failed, false, error.to_string()),
    };
    checkpoint(
        inner,
        owner,
        PostprocessKind::Script,
        name,
        state,
        None,
        Some(truncate(message)),
    )
    .await?;
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::{ScriptContext, arguments, failure_reason, resolve_script};

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
            Vec::new(),
            Duration::from_millis(300),
        )
        .await
        .expect_err("timeout");
        assert!(error.to_string().contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(5));
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
}
