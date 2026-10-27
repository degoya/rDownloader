//! The two phases of a run (RD-1390-02), with a scratch configuration directory, a keyring in
//! memory, a browser that only counts, and a linked phase that serves what the agent's serves --
//! Click'n'Load's routes with the link page's -- without the agent's background tasks.

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use axum::{Router, http::StatusCode};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{
    LINK_PAGE_SHOWN_FILE, Surroundings,
    files::{LINK_KEY_FILE, LINK_REDIRECT_FILE, redirect_html},
    link_page_url, page_of_running_agent, run_phases,
};
use crate::{
    DesktopSinks, agent_run,
    cli::{RunArgs, default_run_args},
    client::CaptureClient,
    cnl,
    config::Connection,
    link_page::{
        LinkSecret,
        tests::{Answer, TOKEN, fixture},
    },
    relaunch::Replaced,
};

/// The keyring of these tests: the last link stored.
type Keyring = Arc<Mutex<Option<(Url, String)>>>;

/// A run's surroundings in `directory`, and what the browser was asked to open.
struct Scene {
    directory: PathBuf,
    keyring: Keyring,
    opened: Arc<Mutex<Vec<Url>>>,
    /// The browser cannot be started.
    no_browser: Arc<std::sync::atomic::AtomicBool>,
}

impl Scene {
    fn new(name: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("rd-capture-phase-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("create the scratch directory");
        Self {
            directory,
            keyring: Arc::new(Mutex::new(None)),
            opened: Arc::new(Mutex::new(Vec::new())),
            no_browser: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    fn surroundings(&self) -> Surroundings {
        let (stored, read) = (self.keyring.clone(), self.keyring.clone());
        let opened = self.opened.clone();
        let no_browser = self.no_browser.clone();
        Surroundings {
            directory: self.directory.clone(),
            store: Arc::new(
                move |service: &Url, token: &str, _insecure: bool| -> Result<()> {
                    *rd_testkit::lock(&stored) = Some((service.clone(), token.to_owned()));
                    Ok(())
                },
            ),
            connect: Arc::new(move |_args: &RunArgs| -> Result<Option<Connection>> {
                Ok(rd_testkit::lock(&read)
                    .clone()
                    .map(|(service, token)| Connection { service, token }))
            }),
            program: None,
            check_every: Duration::from_millis(20),
            open: Arc::new(move |page: &Url| -> std::io::Result<()> {
                rd_testkit::lock(&opened).push(page.clone());
                if no_browser.load(Ordering::SeqCst) {
                    return Err(std::io::Error::other("no browser"));
                }
                Ok(())
            }),
        }
    }

    fn opened(&self) -> usize {
        rd_testkit::lock(&self.opened).len()
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Run arguments on a port of this test's own, with the page opened by itself unless `quiet`.
async fn arguments(quiet: bool) -> (RunArgs, u16) {
    let port = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        listener.local_addr().expect("the bound address").port()
    };
    let mut args = default_run_args();
    args.cnl_listen = vec![SocketAddr::from(([127, 0, 0, 1], port))];
    args.clipboard = false;
    args.no_notifications = true;
    args.no_tray = true;
    args.no_link_page = quiet;
    (args, port)
}

/// The linked phase as the agent serves it, without its background tasks.
async fn stand_in(
    args: RunArgs,
    connection: Connection,
    cancellation: CancellationToken,
    _desktop: Option<DesktopSinks>,
    link_router: Router,
) -> Result<()> {
    let client = CaptureClient::new(connection.service, connection.token)?;
    let router = cnl::routes(client).merge(link_router);
    let listeners =
        agent_run::bind_listeners(args.cnl_listen, None, &mut JoinSet::new(), &cancellation)
            .await?;
    let mut servers = JoinSet::new();
    for (address, listener) in listeners {
        servers.spawn(cnl::serve(
            address,
            listener,
            router.clone(),
            cancellation.clone(),
        ));
    }
    while let Some(result) = servers.join_next().await {
        result??;
    }
    Ok(())
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("build a client")
}

/// Waits, a few seconds at most, until `url` answers with `status`.
async fn until_answered(url: &str, status: StatusCode) {
    for _ in 0..200 {
        if let Ok(answer) = client().get(url).send().await
            && answer.status() == status
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{url} never answered {status}");
}

/// The page of the run in `scene`, once its `link.key` is there and it answers.
async fn running_page(scene: &Scene) -> Url {
    for _ in 0..200 {
        if let Some(page) = page_of_running_agent(&scene.directory) {
            until_answered(page.as_str(), StatusCode::OK).await;
            return page;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no link.key appeared");
}

/// Links through the page as the browser would.
async fn link(page: &Url, port: u16, service: &str) {
    let answer = client()
        .post(page.as_str())
        .header("Origin", format!("http://127.0.0.1:{port}"))
        .json(&serde_json::json!({ "service": service, "token": TOKEN }))
        .send()
        .await
        .expect("answered");
    assert_eq!(answer.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_unlinked_agent_serves_only_the_link_page() {
    let scene = Scene::new("unlinked");
    let (args, port) = arguments(true).await;
    let cancellation = CancellationToken::new();
    let run = tokio::spawn(run_phases(
        args,
        cancellation.clone(),
        None,
        scene.surroundings(),
        stand_in,
    ));
    running_page(&scene).await;
    let flash = client()
        .get(format!("http://127.0.0.1:{port}/flash"))
        .send()
        .await
        .expect("answered");
    assert_eq!(flash.status(), StatusCode::NOT_FOUND, "no Click'n'Load yet");
    cancellation.cancel();
    run.await.expect("the run ends").expect("without an error");
}

#[tokio::test]
async fn linking_switches_on_click_and_load() {
    let service = fixture("rDownloader", Answer::Accept).await;
    let scene = Scene::new("switch");
    let (args, port) = arguments(true).await;
    let cancellation = CancellationToken::new();
    let run = tokio::spawn(run_phases(
        args,
        cancellation.clone(),
        None,
        scene.surroundings(),
        stand_in,
    ));
    let page = running_page(&scene).await;
    link(&page, port, &service.service()).await;
    until_answered(&format!("http://127.0.0.1:{port}/flash"), StatusCode::OK).await;
    // The page stays on the port, for a new link.
    until_answered(page.as_str(), StatusCode::OK).await;
    assert_eq!(
        rd_testkit::lock(&scene.keyring)
            .as_ref()
            .map(|(_, token)| token.as_str()),
        Some(TOKEN)
    );
    cancellation.cancel();
    run.await.expect("the run ends").expect("without an error");
}

#[tokio::test]
async fn relinking_rebinds_without_a_bind_error() {
    let service = fixture("rDownloader", Answer::Accept).await;
    let scene = Scene::new("relink");
    let (args, port) = arguments(true).await;
    let phases = Arc::new(AtomicUsize::new(0));
    let counted = phases.clone();
    let cancellation = CancellationToken::new();
    let run = tokio::spawn(run_phases(
        args,
        cancellation.clone(),
        None,
        scene.surroundings(),
        move |args, connection, phase, desktop, router| {
            counted.fetch_add(1, Ordering::SeqCst);
            stand_in(args, connection, phase, desktop, router)
        },
    ));
    let page = running_page(&scene).await;
    link(&page, port, &service.service()).await;
    until_answered(&format!("http://127.0.0.1:{port}/flash"), StatusCode::OK).await;
    link(&page, port, &service.service()).await;
    for _ in 0..200 {
        if phases.load(Ordering::SeqCst) == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        phases.load(Ordering::SeqCst),
        2,
        "one linked phase per link"
    );
    until_answered(&format!("http://127.0.0.1:{port}/flash"), StatusCode::OK).await;
    assert!(!run.is_finished(), "the new phase got the port");
    cancellation.cancel();
    run.await.expect("the run ends").expect("without an error");
}

#[tokio::test]
async fn the_link_key_is_written_and_removed() {
    let scene = Scene::new("key");
    let (args, port) = arguments(true).await;
    let cancellation = CancellationToken::new();
    let run = tokio::spawn(run_phases(
        args,
        cancellation.clone(),
        None,
        scene.surroundings(),
        stand_in,
    ));
    let page = running_page(&scene).await;
    assert_eq!(page.port(), Some(port));
    assert!(page.path().starts_with("/link/"));
    // The browser is opened with this file, never with the address: a command line is readable
    // by every local account, the file by this one only.
    let redirect =
        std::fs::read_to_string(scene.directory.join(LINK_REDIRECT_FILE)).expect("link.html");
    assert!(redirect.contains(&format!(
        r#"<meta http-equiv="refresh" content="0;url={page}">"#
    )));
    assert!(redirect.contains(r#"<meta name="referrer" content="no-referrer">"#));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        for name in [LINK_KEY_FILE, LINK_REDIRECT_FILE] {
            let mode = std::fs::metadata(scene.directory.join(name))
                .expect(name)
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{name} is this account's alone");
        }
    }
    cancellation.cancel();
    run.await.expect("the run ends").expect("without an error");
    assert!(!scene.directory.join(LINK_KEY_FILE).exists());
    assert!(!scene.directory.join(LINK_REDIRECT_FILE).exists());
}

/// What `link.html` carries is escaped for the attribute it stands in.
#[test]
fn the_redirect_escapes_what_it_carries() {
    // `url` percent-encodes quotes and angle brackets itself; `&` is left to this escape.
    let page = Url::parse("http://127.0.0.1:9666/link/abc?a=1&b=2").expect("valid URL");
    let html = redirect_html(&page);
    assert!(
        html.contains(r#"content="0;url=http://127.0.0.1:9666/link/abc?a=1&amp;b=2""#),
        "{html}"
    );
    assert!(!html.contains("a=1&b"));
}

#[tokio::test]
async fn the_page_opens_by_itself_once_per_install() {
    let scene = Scene::new("open-once");
    for (round, expected) in [(1, 1), (2, 1)] {
        let (args, _) = arguments(false).await;
        let cancellation = CancellationToken::new();
        let run = tokio::spawn(run_phases(
            args,
            cancellation.clone(),
            None,
            scene.surroundings(),
            stand_in,
        ));
        let page = running_page(&scene).await;
        assert_eq!(scene.opened(), expected, "round {round}");
        if round == 1 {
            assert_eq!(rd_testkit::lock(&scene.opened)[0], page, "this run's page");
        }
        cancellation.cancel();
        run.await.expect("the run ends").expect("without an error");
        assert!(scene.directory.join(LINK_PAGE_SHOWN_FILE).exists());
    }

    let quiet = Scene::new("open-never");
    let (args, _) = arguments(true).await;
    let cancellation = CancellationToken::new();
    let run = tokio::spawn(run_phases(
        args,
        cancellation.clone(),
        None,
        quiet.surroundings(),
        stand_in,
    ));
    running_page(&quiet).await;
    assert_eq!(quiet.opened(), 0, "--no-link-page");
    cancellation.cancel();
    run.await.expect("the run ends").expect("without an error");
    assert!(!quiet.directory.join(LINK_PAGE_SHOWN_FILE).exists());
}

#[test]
fn the_page_address_is_the_loopback_one_with_the_secret() {
    let secret = LinkSecret::generate();
    let page = link_page_url(9666, &secret);
    assert_eq!(
        page.as_str(),
        format!("http://127.0.0.1:9666/link/{}", secret.as_str())
    );
}

#[test]
fn only_a_page_of_this_machine_is_taken_from_the_key_file() {
    let scene = Scene::new("key-file");
    let path = scene.directory.join(LINK_KEY_FILE);
    assert_eq!(
        page_of_running_agent(&scene.directory),
        None,
        "no agent runs"
    );
    for foreign in [
        "https://evil.example/link/abc",
        "http://127.0.0.1:9666/elsewhere",
        "file:///etc/passwd",
    ] {
        std::fs::write(&path, foreign).expect("write");
        assert_eq!(page_of_running_agent(&scene.directory), None, "{foreign}");
    }
    std::fs::write(&path, "http://127.0.0.1:9666/link/abc\n").expect("write");
    assert_eq!(
        page_of_running_agent(&scene.directory).map(String::from),
        Some("http://127.0.0.1:9666/link/abc".to_owned())
    );
}

/// A page that could not be opened is tried again at the next start: the marker says "shown",
/// and it was not.
#[tokio::test]
async fn a_page_that_did_not_open_opens_at_the_next_start() {
    let scene = Scene::new("open-failed");
    for (attempts, browser) in [(1, false), (2, true)] {
        scene.no_browser.store(!browser, Ordering::SeqCst);
        let (args, _) = arguments(false).await;
        let cancellation = CancellationToken::new();
        let run = tokio::spawn(run_phases(
            args,
            cancellation.clone(),
            None,
            scene.surroundings(),
            stand_in,
        ));
        running_page(&scene).await;
        assert_eq!(scene.opened(), attempts);
        assert_eq!(
            scene.directory.join(LINK_PAGE_SHOWN_FILE).exists(),
            browser,
            "the marker only once the browser started"
        );
        cancellation.cancel();
        run.await.expect("the run ends").expect("without an error");
    }
}

/// An update that replaces the program while the agent is not linked ends the run as one while
/// it is linked does: as `Replaced`, so the process continues as the new program (RD-190-07). One
/// watch for the whole run, its baseline the file at the start; a run whose file stays as it was
/// ends without one.
#[tokio::test]
async fn an_update_while_unlinked_hands_over_to_the_new_program() {
    let scene = Scene::new("relaunch");
    let program = scene.directory.join("rdownloader-capture");
    std::fs::write(&program, b"the program as it started").expect("write the program");

    let (args, _) = arguments(true).await;
    let mut around = scene.surroundings();
    around.program = Some(program.clone());
    let cancellation = CancellationToken::new();
    let unchanged = tokio::spawn(run_phases(
        args,
        cancellation.clone(),
        None,
        around,
        stand_in,
    ));
    running_page(&scene).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancellation.cancel();
    unchanged
        .await
        .expect("the run ends")
        .expect("no update, no hand-over");

    let (args, _) = arguments(true).await;
    let mut around = scene.surroundings();
    around.program = Some(program.clone());
    let run = tokio::spawn(run_phases(
        args,
        CancellationToken::new(),
        None,
        around,
        stand_in,
    ));
    running_page(&scene).await;
    std::fs::write(&program, b"the updated program, longer than the first").expect("replace it");
    let error = tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("the run ends by itself")
        .expect("the run's task")
        .expect_err("as the replaced program");
    assert_eq!(
        error
            .downcast_ref::<Replaced>()
            .map(|replaced| replaced.program.clone()),
        Some(program)
    );
}
