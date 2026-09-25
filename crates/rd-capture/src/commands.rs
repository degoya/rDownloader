//! The one-shot subcommands: they run once, do their one thing and end the process. Nothing
//! here belongs to the long-lived agent.

use anyhow::{Context, Result};
use reqwest::multipart;

use crate::{
    cli::{ConfigureArgs, HandleArgs, IntegrationArgs, IntegrationCommand, OpenArgs},
    client::{self, Purpose},
    config, os_integration, scheme,
};

pub(crate) fn configure(args: ConfigureArgs) -> Result<()> {
    let token = match args.token {
        Some(token) => token,
        None => read_token_from_stdin()?,
    };
    config::save(&args.service, &token, args.allow_insecure_service)?;
    println!("Capture connection stored securely: {}", args.service);
    Ok(())
}

/// Takes the capture token off standard input.
///
/// Reads to the end rather than one line, so `configure --token-stdin < token.txt` and a pipe
/// from a password manager both work; the value is trimmed, so a trailing newline is not part
/// of the token.
fn read_token_from_stdin() -> Result<String> {
    use std::io::Read;

    let mut token = String::new();
    std::io::stdin()
        .read_to_string(&mut token)
        .context("read the capture token from standard input")?;
    let token = token.trim().to_owned();
    if token.is_empty() {
        anyhow::bail!("no capture token arrived on standard input");
    }
    Ok(token)
}

/// Reads a file, refusing it the moment it turns out to be bigger than `limit`.
///
/// The limit used to be measured with `metadata()` and the file read afterwards with no limit at
/// all, which measured one file and read another: between the two calls a file can grow and a
/// symlink can be pointed somewhere else, and the 64 MiB cap was then simply not there. Reading
/// one byte past the limit and refusing on that byte closes the window -- nothing beyond it is
/// ever pulled into memory. `open` is reachable both through the `.nzb` file association and
/// through `rdownloader://`, so its input is not the user's to vouch for (RD-109-03).
async fn read_at_most(path: &std::path::Path, limit: usize) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    let mut content = Vec::new();
    let read = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    file.take(read)
        .read_to_end(&mut content)
        .await
        .with_context(|| format!("read {}", path.display()))?;
    if content.len() > limit {
        anyhow::bail!("{} exceeds {limit} bytes", path.display());
    }
    Ok(content)
}

pub(crate) async fn open(args: OpenArgs) -> Result<()> {
    let content = read_at_most(&args.path, rd_collector::MAX_NZB_BYTES).await?;
    let name = args
        .path
        .file_name()
        .and_then(|value| value.to_str())
        .context("NZB filename is not Unicode")?
        .to_owned();
    let form = multipart::Form::new().part(
        "file",
        multipart::Part::bytes(content)
            .file_name(name)
            .mime_str("application/x-nzb")?,
    );
    let response = client::build(Purpose::Request)?
        .post(args.agent.join("rdownloader/nzb")?)
        .multipart(form)
        .send()
        .await?;
    if !response.status().is_success() {
        anyhow::bail!("Capture-Agent returned HTTP {}", response.status());
    }
    println!("NZB handed to the LinkGrabber: {}", args.path.display());
    Ok(())
}

/// Acts on one `rdownloader://` address.
///
/// Everything the address is allowed to mean is decided in `scheme::parse`; this only
/// forwards the result to the agent that is already running, using the same endpoints the
/// clipboard and file association use. No new way into the service is opened here.
pub(crate) async fn handle(args: HandleArgs) -> Result<()> {
    match scheme::parse(&args.url)? {
        scheme::Action::Links(links) => {
            let response = client::build(Purpose::Request)?
                .post(args.agent.join("flash/add")?)
                .body(links_form_body(&links))
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .send()
                .await?;
            if !response.status().is_success() {
                anyhow::bail!("capture agent returned HTTP {}", response.status());
            }
            println!("{} link(s) handed to the LinkGrabber", links.len());
            Ok(())
        }
        scheme::Action::OpenFile(path) => {
            open(OpenArgs {
                agent: args.agent,
                path,
            })
            .await
        }
    }
}

/// The `application/x-www-form-urlencoded` body the scheme handler posts to `flash/add`.
///
/// Built with `url`, which this crate already depends on for `Url::join`, rather than with a
/// hand-written percent-encoder beside it. The comment that used to justify writing one out --
/// "a dependency for one function is not worth the supply chain" -- was simply not true: no
/// dependency was being avoided, only a second and less-tested implementation of the same
/// algorithm was being kept (RD-109-16).
///
/// `form_urlencoded::Serializer` and not `byte_serialize`: the former is the encoding that goes
/// with the declared content type, and it is what the receiver decodes. The one visible
/// difference is that a space becomes `+` rather than `%20`; the receiver is `flash/add` in this
/// same binary, which takes the body through `axum::Form` and therefore through
/// `form_urlencoded::parse`, and that resolves `+` back to a space. The test below checks that
/// round trip rather than the bytes on the wire, because what has to be unchanged is what the
/// receiver ends up with.
fn links_form_body(links: &[String]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("urls", &links.join("\n"))
        .finish()
}

pub(crate) fn integration(args: IntegrationArgs, kind: os_integration::Kind) -> Result<()> {
    let executable = std::env::current_exe().context("locate capture executable")?;
    match args.command {
        IntegrationCommand::Install => os_integration::install(kind, &executable),
        IntegrationCommand::Remove => os_integration::remove(kind),
    }
}

pub(crate) fn autostart(args: IntegrationArgs) -> Result<()> {
    match args.command {
        IntegrationCommand::Install => {
            // `is_paired`, not `load`: this only has to know whether there is anything to
            // connect with, and `load` reads the keyring for it -- which is a second macOS
            // Keychain prompt on top of the agent's own, for an answer it does not need
            // (RD-109-04).
            if !config::is_paired(None) {
                anyhow::bail!(
                    "capture agent is not configured; run \
                     `rdownloader-capture configure --token-stdin` first"
                );
            }
            let executable = std::env::current_exe().context("locate capture executable")?;
            rd_autostart::install(rd_autostart::Target::Capture, &executable)?;
            println!("rDownloader capture autostart installed for the next login.");
        }
        IntegrationCommand::Remove => {
            rd_autostart::remove(rd_autostart::Target::Capture)?;
            println!("rDownloader capture autostart removed.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{links_form_body, read_at_most};

    /// The size cap has to hold against the file that is actually read, not against the one
    /// `metadata()` happened to see a moment earlier (RD-109-03).
    #[tokio::test]
    async fn a_file_over_the_limit_is_refused_while_it_is_read() {
        let path = std::env::temp_dir().join(format!(
            "rd-capture-read-at-most-{}.bin",
            std::process::id()
        ));
        std::fs::write(&path, vec![b'x'; 100]).expect("write the sample");

        let refused = read_at_most(&path, 50)
            .await
            .expect_err("a file over the limit is not read");
        assert!(
            refused.to_string().contains("exceeds 50 bytes"),
            "{refused}"
        );

        // Exactly at the limit still reads, so the boundary is inclusive rather than off by one.
        assert_eq!(
            read_at_most(&path, 100).await.expect("at the limit").len(),
            100
        );
        assert_eq!(
            read_at_most(&path, 4096)
                .await
                .expect("under the limit")
                .len(),
            100
        );

        std::fs::remove_file(&path).expect("clean up the sample");
    }

    /// What has to be unchanged is what the receiver ends up with, not the bytes on the wire:
    /// `form_urlencoded` writes a space as `+` where the hand-written encoder wrote `%20`, and
    /// both are valid in this content type (RD-109-16).
    #[test]
    fn the_scheme_handler_body_survives_decoding_unchanged() {
        let links = vec![
            "https://example.com/a file.bin".to_owned(),
            "https://example.com/?q=a+b&r=c=d".to_owned(),
            "https://example.com/Gr\u{fc}\u{df}e/\u{4e2d}\u{6587}.bin".to_owned(),
            "magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01&dn=a b".to_owned(),
        ];
        let body = links_form_body(&links);

        // The primitive `axum::Form` decodes with, through `serde_urlencoded`.
        let decoded: Vec<(String, String)> = url::form_urlencoded::parse(body.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert_eq!(
            decoded.len(),
            1,
            "one field, whatever is in it: {decoded:?}"
        );
        assert_eq!(decoded[0].0, "urls");
        assert_eq!(
            decoded[0].1,
            links.join("\n"),
            "every link has to arrive exactly as it went in"
        );

        // Each character the encoding could have eaten, named so a failure says which one.
        for character in [' ', '+', '&', '=', '\u{fc}'] {
            assert!(
                decoded[0].1.contains(character),
                "{character:?} did not survive the round trip: {}",
                decoded[0].1
            );
        }
        // And the separator the receiver splits on is still a separator.
        assert_eq!(decoded[0].1.lines().count(), links.len());
    }
}
