//! What a run keeps on disk beside its configuration (RD-1390-02): `link.key`, the page's address
//! for `rdownloader-capture link` and the tray, and `link.html`, which forwards a browser to it.
//!
//! The browser is never started with the page's address. That address carries the secret, and a
//! command line is readable by every local account (`/proc/<pid>/cmdline`, `ps`, the task
//! manager): another account could read it there and link this agent to a server of its own. The
//! browser is handed the path of `link.html` instead, a file only this account can read, whose
//! `<meta http-equiv="refresh">` takes it to the page.

use std::path::{Path, PathBuf};

use url::Url;

use crate::config;

/// The page's address while the agent runs, for `rdownloader-capture link` and the tray.
pub(crate) const LINK_KEY_FILE: &str = "link.key";

/// The file the browser is opened with; it forwards to the page.
pub(crate) const LINK_REDIRECT_FILE: &str = "link.html";

/// `link.key` and `link.html` while the run lasts; removed when it is dropped, at the end of the
/// run.
pub(super) struct LinkKey(Vec<PathBuf>);

impl LinkKey {
    pub(super) fn write(directory: &Path, page: &Url) -> Self {
        let mut written = Vec::new();
        for (name, body) in [
            (LINK_KEY_FILE, format!("{page}\n")),
            (LINK_REDIRECT_FILE, redirect_html(page)),
        ] {
            let path = directory.join(name);
            match write_private(directory, &path, body.as_bytes()) {
                Ok(()) => written.push(path),
                Err(error) => tracing::warn!(
                    %error,
                    file = name,
                    "`rdownloader-capture link` and the tray cannot find the link page"
                ),
            }
        }
        Self(written)
    }
}

impl Drop for LinkKey {
    fn drop(&mut self) {
        for path in self.0.drain(..) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A page that sends the browser on to `page` at once, without a referrer.
pub(crate) fn redirect_html(page: &Url) -> String {
    let target = html_escape(page.as_str());
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">\
         <meta name=\"referrer\" content=\"no-referrer\">\
         <meta http-equiv=\"refresh\" content=\"0;url={target}\">\
         <title>rDownloader Capture</title></head><body></body></html>\n"
    )
}

fn html_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Writes a file only this account can read, like the plaintext token (`capture.token`). On
/// Windows the configuration directory is the account's own profile.
pub(crate) fn write_private(directory: &Path, path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    std::fs::create_dir_all(directory)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        options.mode(0o600);
        let mut file = options.open(path)?;
        // `mode` applies to a new file only; an existing one is narrowed before anything is in it.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(body)
    }
    #[cfg(not(unix))]
    {
        options.open(path)?.write_all(body)
    }
}

/// The page of the agent that runs, from its `link.key`; `None` while none runs. Only an address
/// of this machine's link page is taken.
pub(crate) fn page_of_running_agent(directory: &Path) -> Option<Url> {
    let text = std::fs::read_to_string(directory.join(LINK_KEY_FILE)).ok()?;
    let page = Url::parse(text.trim()).ok()?;
    let ours = page.scheme() == "http"
        && page.host_str() == Some("127.0.0.1")
        && page.path().starts_with("/link/")
        && page.query().is_none();
    ours.then_some(page)
}

/// Opens the running agent's page through its `link.html`: the start, the tray's entry and its
/// shortcut, and `rdownloader-capture link`.
pub(crate) fn open_redirect(directory: &Path) -> std::io::Result<()> {
    let redirect = directory.join(LINK_REDIRECT_FILE);
    if !redirect.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the agent is not running or could not write link.html",
        ));
    }
    open_in_browser(&redirect)
}

/// Opens the link page of the running agent from the tray or a shortcut, on every system.
pub(crate) fn open_link_page() {
    let opened = config::config_directory()
        .map_err(|error| std::io::Error::other(error.to_string()))
        .and_then(|directory| open_redirect(&directory));
    if let Err(error) = opened {
        tracing::warn!(%error, "could not open the link page; see `rdownloader-capture link`");
    }
}

/// The desktop's browser for a file: `open` where the tray links it, `xdg-open` on Linux, which
/// links no desktop crate.
fn open_in_browser(file: &Path) -> std::io::Result<()> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        open::that_detached(file)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(file)
            .spawn()
            .map(|_| ())
    }
}
