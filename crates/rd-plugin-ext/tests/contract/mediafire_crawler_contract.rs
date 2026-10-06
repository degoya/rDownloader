//! The MediaFire folder crawler, exercised end to end against its built component (RD-103-06).
//!
//! **The provider is a mock.** It answers at the host boundary, so no socket is opened and
//! mediafire.com is not contacted. What it answers with are the API documents captured on
//! 2026-09-21 (`plugins/mediafire-crawler/tests/fixtures/`, see the README there) for the
//! public folder `rww7bhhi0yc1l`; the subfolders' content, a second chunk and the refusals
//! were not captured and are generated here from the captured shape, labelled as such.
//!
//! Everything runs the real `rd_plugin_mediafire_crawler.wasm` inside Wasmtime: the target
//! decision and the walk are tested natively in the plugin crate, this file is where the
//! guest is shown to behave in the host the way the host expects.
//!
//! | Case | Outcome |
//! | --- | --- |
//! | A folder of files and subfolders | links, with names, sizes and the folder they sat in |
//! | A folder wide enough for a second chunk | both chunks read, `chunk=1` then `chunk=2` |
//! | A bare key that is a file | `not_a_folder` as *not mine*: the resolver's turn |
//! | A bare key that is a folder | walked like `/folder/<key>` |
//! | A folder key the API does not know | `folder_unreachable`, not *not mine* |
//! | A private folder, an empty folder, a rate limit | three codes, never an empty package |
//! | A list of file keys | one batched `file/get_info`, the deleted key absent |
//! | A password-protected file in a folder | dropped, because nothing could download it |
//! | A folder that contains itself | read once |
//! | The fixtures | carry no token, session id, address or owner name |

use std::sync::Arc;

#[path = "mediafire_crawler_contract/mock.rs"]
mod mock;

use mock::{
    CONTENT_FILES, CONTENT_FOLDERS, EMPTY_FOLDER, FILE_INFO_BATCH, FILE_INFO_FOLDERKEY,
    FOLDER_INFO, Folder, MISSING_TOKEN, MockMediafire, PRIVATE_FOLDER, RATE_LIMITED, ROOT,
    ROOT_NAME, SUB_FOLDER, captured_tree, chunk, component, crawler, file, info, subfolder,
};

#[tokio::test]
async fn only_folder_addresses_bare_keys_and_key_lists_are_claimed() {
    let bytes = component();
    let host = MockMediafire::new(Vec::new());
    let crawler = crawler(Arc::clone(&host), &bytes);
    for claimed in [
        "https://www.mediafire.com/folder/rww7bhhi0yc1l",
        "https://www.mediafire.com/folder/rww7bhhi0yc1l/Droidfeats",
        "https://www.mediafire.com/folder/rww7bhhi0yc1l/shared",
        "https://www.mediafire.com/?rww7bhhi0yc1l",
        "https://mfi.re/?ipnyzofjcwri357",
        "https://www.mediafire.com/?ipnyzofjcwri357,8ipst0t9u6sibpx",
    ] {
        assert!(crawler.claims(claimed).await.expect("claims"), "{claimed}");
    }
    for left_alone in [
        "https://www.mediafire.com/file/ipnyzofjcwri357/test-10mb.bin/file",
        "https://www.mediafire.com/download/ipnyzofjcwri357",
        "https://www.mediafire.com/",
        "https://www.dropbox.com/sh/abc/h1",
        "https://download1514.mediafire.com/token/rww7bhhi0yc1l/x",
    ] {
        assert!(
            !crawler.claims(left_alone).await.expect("claims"),
            "{left_alone}"
        );
    }
    assert!(host.requests().is_empty(), "claiming reaches nothing");
}

#[tokio::test]
async fn the_captured_folder_yields_its_files_with_names_sizes_and_structure() {
    let bytes = component();
    let host = MockMediafire::new(captured_tree());
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("https://www.mediafire.com/folder/{ROOT}"), None)
        .await
        .expect("call")
        .expect("a listing");

    // 19 captured files at the root, 2 of 3 generated in TestFolder, 1 below it.
    assert_eq!(links.len(), 22, "{links:?}");
    assert_eq!(
        links[0].file_name.as_deref(),
        Some("Galaxy-s9-wallpaper-1.png")
    );
    assert_eq!(links[0].size, Some(3_117_448));
    assert_eq!(links[0].package_hint.as_deref(), Some(ROOT_NAME));
    assert_eq!(
        links[0].url,
        "https://www.mediafire.com/file/8ipst0t9u6sibpx/Galaxy-s9-wallpaper-1.png/file"
    );
    let readme = links
        .iter()
        .find(|link| link.file_name.as_deref() == Some("readme.txt"))
        .expect("readme");
    assert_eq!(
        readme.package_hint.as_deref(),
        Some("Droidfeats Galaxy S9 Walls/TestFolder")
    );
    assert_eq!(readme.size, Some(12));
    let deep = links
        .iter()
        .find(|link| link.file_name.as_deref() == Some("deep.bin"))
        .expect("deep");
    assert_eq!(
        deep.package_hint.as_deref(),
        Some("Droidfeats Galaxy S9 Walls/TestFolder/Sub")
    );
    assert!(
        links
            .iter()
            .all(|link| link.file_name.as_deref() != Some("secret.zip")),
        "a password-protected file is dropped: {links:?}"
    );
    assert!(
        links
            .iter()
            .all(|link| link.url.starts_with("https://www.mediafire.com/file/")),
        "every link is the resolver's canonical file address"
    );

    let requests = host.requests();
    assert_eq!(
        requests[0],
        format!("/api/1.5/folder/get_info.php?folder_key={ROOT}&content_type=&chunk=&quick_key=")
    );
    assert!(requests.contains(&format!(
        "/api/1.5/folder/get_content.php?folder_key={ROOT}&content_type=files&chunk=1&quick_key="
    )));
    assert!(requests.contains(&format!(
        "/api/1.5/folder/get_content.php?folder_key={SUB_FOLDER}&content_type=files&chunk=1&quick_key="
    )));
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.contains("folder/get_info"))
            .count(),
        1,
        "only the crawled folder is asked about itself; subfolders are named by their parent"
    );
}

#[tokio::test]
async fn a_second_chunk_is_read_after_the_first_says_there_is_more() {
    let bytes = component();
    let host = MockMediafire::new(vec![Folder {
        key: ROOT,
        info: info(ROOT, "Wide"),
        chunks: vec![
            (("folders", 1), chunk(ROOT, "folders", 1, &[], false)),
            (
                ("files", 1),
                chunk(ROOT, "files", 1, &[file("aaaaaaaaaaa", "one.bin", 1)], true),
            ),
            (
                ("files", 2),
                chunk(
                    ROOT,
                    "files",
                    2,
                    &[file("bbbbbbbbbbb", "two.bin", 2)],
                    false,
                ),
            ),
        ],
    }]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("https://www.mediafire.com/folder/{ROOT}"), None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), 2);
    assert_eq!(links[1].file_name.as_deref(), Some("two.bin"));
    let requests = host.requests();
    assert!(
        requests
            .iter()
            .any(|request| request.contains("content_type=files&chunk=1"))
    );
    assert!(
        requests
            .iter()
            .any(|request| request.contains("content_type=files&chunk=2"))
    );
    assert!(!requests.iter().any(|request| request.contains("chunk=3")));
}

/// A bare key that the API says is no folder is handed on, not refused.
#[tokio::test]
async fn a_bare_key_that_is_a_file_is_disclaimed_as_not_mine() {
    let bytes = component();
    let host = MockMediafire::new(captured_tree());
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl("https://www.mediafire.com/?ipnyzofjcwri357", None)
        .await
        .expect("call")
        .expect_err("not a folder");
    assert!(refusal.not_mine, "{refusal:?}");
    assert_eq!(
        refusal.code.as_deref(),
        Some("mediafire_crawler.not_a_folder")
    );
    assert_eq!(
        host.requests().len(),
        1,
        "one question, then the resolver's turn"
    );

    // The same key as a folder path is a folder that could not be read: the path decided.
    let host = MockMediafire::new(captured_tree());
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl("https://www.mediafire.com/folder/ipnyzofjcwri357", None)
        .await
        .expect("call")
        .expect_err("unreachable");
    assert!(!refusal.not_mine);
    assert_eq!(
        refusal.code.as_deref(),
        Some("mediafire_crawler.folder_unreachable")
    );
}

#[tokio::test]
async fn a_bare_key_that_is_a_folder_is_walked() {
    let bytes = component();
    let host = MockMediafire::new(captured_tree());
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("https://mfi.re/?{ROOT}"), None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), 22);
}

#[tokio::test]
async fn private_empty_and_rate_limited_folders_are_three_codes_and_never_an_empty_package() {
    let bytes = component();
    let host = MockMediafire::new(vec![Folder {
        key: "zzzzzzzzzzzzz",
        info: PRIVATE_FOLDER.to_owned(),
        chunks: Vec::new(),
    }]);
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl("https://www.mediafire.com/folder/zzzzzzzzzzzzz", None)
        .await
        .expect("call")
        .expect_err("private");
    assert_eq!(
        refusal.code.as_deref(),
        Some("mediafire_crawler.folder_private")
    );
    assert_eq!(host.requests().len(), 1, "a private folder is not listed");

    let host = MockMediafire::new(vec![Folder {
        key: EMPTY_FOLDER,
        info: info(EMPTY_FOLDER, "EmptyFolder"),
        chunks: vec![
            (
                ("folders", 1),
                chunk(EMPTY_FOLDER, "folders", 1, &[], false),
            ),
            (("files", 1), chunk(EMPTY_FOLDER, "files", 1, &[], false)),
        ],
    }]);
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(
            &format!("https://www.mediafire.com/folder/{EMPTY_FOLDER}"),
            None,
        )
        .await
        .expect("call")
        .expect_err("empty");
    assert_eq!(
        refusal.code.as_deref(),
        Some("mediafire_crawler.folder_empty")
    );
    assert!(!refusal.not_mine);

    let host = MockMediafire::failing(200, RATE_LIMITED);
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("https://www.mediafire.com/folder/{ROOT}"), None)
        .await
        .expect("call")
        .expect_err("rate limited");
    assert_eq!(
        refusal.code.as_deref(),
        Some("mediafire_crawler.rate_limited")
    );

    let host = MockMediafire::failing(503, "<html>down</html>");
    let refusal = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("https://www.mediafire.com/folder/{ROOT}"), None)
        .await
        .expect("call")
        .expect_err("503");
    assert_eq!(
        refusal.code.as_deref(),
        Some("mediafire_crawler.http_error")
    );
}

#[tokio::test]
async fn a_key_list_is_answered_by_one_batched_file_call() {
    let bytes = component();
    let host = MockMediafire::with_files(FILE_INFO_BATCH);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(
            "https://www.mediafire.com/?ipnyzofjcwri357,uz9u9zqa0tlk6z7",
            None,
        )
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), 1, "the deleted key is absent: {links:?}");
    assert_eq!(links[0].file_name.as_deref(), Some("test-10mb.bin"));
    assert_eq!(links[0].size, Some(10_485_760));
    assert_eq!(links[0].package_hint, None);
    assert_eq!(
        host.requests(),
        vec![
            "/api/1.5/file/get_info.php?folder_key=&content_type=&chunk=&quick_key=ipnyzofjcwri357,uz9u9zqa0tlk6z7"
                .to_owned()
        ]
    );
}

#[tokio::test]
async fn a_folder_that_contains_itself_is_read_once() {
    let bytes = component();
    let host = MockMediafire::new(vec![Folder {
        key: ROOT,
        info: info(ROOT, "Loop"),
        chunks: vec![
            (
                ("folders", 1),
                chunk(ROOT, "folders", 1, &[subfolder(ROOT, "Loop")], false),
            ),
            (
                ("files", 1),
                chunk(
                    ROOT,
                    "files",
                    1,
                    &[file("aaaaaaaaaaa", "one.bin", 1)],
                    false,
                ),
            ),
        ],
    }]);
    let links = crawler(Arc::clone(&host), &bytes)
        .crawl(&format!("https://www.mediafire.com/folder/{ROOT}"), None)
        .await
        .expect("call")
        .expect("a listing");
    assert_eq!(links.len(), 1);
    assert_eq!(
        host.requests()
            .iter()
            .filter(|request| request.contains("content_type=files"))
            .count(),
        1
    );
}

/// The fixtures are sanitised captures: nothing in them is a token, a session, an address
/// or a person's name.
#[test]
fn fixtures_carry_no_credential_material() {
    for (name, document) in [
        ("folder-info", FOLDER_INFO),
        ("content-files", CONTENT_FILES),
        ("content-folders", CONTENT_FOLDERS),
        ("missing-token", MISSING_TOKEN),
        ("file-info-folderkey", FILE_INFO_FOLDERKEY),
        ("file-info-batch", FILE_INFO_BATCH),
    ] {
        for marker in ["session_token", "api_key", "@", "Vlas", "Santiago", "Mansi"] {
            assert!(!document.contains(marker), "{name} carries `{marker}`");
        }
    }
}
