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
mod tests;
