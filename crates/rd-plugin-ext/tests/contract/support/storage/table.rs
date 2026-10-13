//! The provider table: one row per cloud drive for its crawler and the shape of its three
//! packages, and one for its sign-in where it runs through the shared flow (`sign_in` beside
//! this). Every value here is what the provider's module asserted before RD-1280-05 moved it.

use rd_plugin_host::OAuthFlowManifest::{Device, Redirect};

use super::{Shape, crawler::Crawler};

pub mod sign_in;

/// One cloud drive: its crawler and its packages' shape.
pub struct Provider {
    pub crawler: Crawler,
    pub shape: Shape,
}

const BOX_RESOLVER: &str = include_str!("../../../../../../plugins/box/manifest.toml");
const BOX_CRAWLER: &str = include_str!("../../../../../../plugins/box-crawler/manifest.toml");
const BOX_OAUTH: &str = include_str!("../../../../../../plugins/box-oauth/manifest.toml");

pub const BOX: Provider = Provider {
    crawler: Crawler {
        component: "rd-plugin-box-crawler",
        manifest: BOX_CRAWLER,
        folder: "https://app.box.com/folder/987654321",
        claimed: &[
            "https://app.box.com/folder/987654321",
            "https://app.box.com/s/abc123def456",
            "https://app.box.com/s/abc123def456?shared_link_password=hunter2",
            "https://contoso.app.box.com/folder/1",
            "https://app.box.com/s/abc123def456/folder/7",
        ],
        foreign: &[
            "https://app.box.com/file/123456789",
            "https://app.box.com/s/abc123def456/file/42",
            "https://app.box.com.evil.test/folder/1",
            "https://ddownload.com/f/abc",
            "https://api.box.com/2.0/files/1/content",
        ],
        refusals: &[
            (
                403,
                include_str!("../../../fixtures/box/error_forbidden.json"),
                "box_crawler.access_denied",
            ),
            (
                404,
                include_str!("../../../fixtures/box/error_not_found.json"),
                "box_crawler.folder_unreachable",
            ),
            (
                429,
                include_str!("../../../fixtures/box/error_rate_limit.json"),
                "box_crawler.rate_limited",
            ),
            (
                401,
                include_str!("../../../fixtures/box/error_unauthorized.json"),
                "box_crawler.sign_in_required",
            ),
        ],
        prose: &["insufficient permission", "redacted-request-id"],
    },
    shape: Shape {
        slug: "box",
        resolver: BOX_RESOLVER,
        crawler: BOX_CRAWLER,
        sign_in: BOX_OAUTH,
        resolver_domains: &["api.box.com"],
        crawler_domains: &["api.box.com"],
        sign_in_domains: &["account.box.com", "api.box.com"],
        flows: &[Redirect],
        secret_reference: None,
        secret_domains: None,
        // The API route and the storage host it redirects to, never `*`: a hoster for its own
        // cloud.
        download_domains: Some(&["api.box.com", "*.boxcloud.com"]),
        sign_in_secrets: &["box_client_secret"],
    },
};

const DROPBOX_RESOLVER: &str = include_str!("../../../../../../plugins/dropbox/manifest.toml");
const DROPBOX_CRAWLER: &str =
    include_str!("../../../../../../plugins/dropbox-crawler/manifest.toml");
const DROPBOX_OAUTH: &str = include_str!("../../../../../../plugins/dropbox-oauth/manifest.toml");

pub const DROPBOX: Provider = Provider {
    crawler: Crawler {
        component: "rd-plugin-dropbox-crawler",
        manifest: DROPBOX_CRAWLER,
        folder: "https://www.dropbox.com/home/Show",
        claimed: &[
            "https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1&dl=0",
            "https://www.dropbox.com/sh/abc/h1/Season%201?dl=0",
            "https://www.dropbox.com/home/Show",
            "https://www.dropbox.com/home",
        ],
        foreign: &[
            "https://www.dropbox.com/s/abc/release.bin?dl=0",
            "https://www.dropbox.com/scl/fi/abc/release.bin?rlkey=k1",
            "https://www.dropbox.com/scl/fo/abc/h1?rlkey=k1&preview=e01.mkv",
            "https://www.dropbox.com/home/Show?preview=e01.mkv",
            "https://dropbox.com.evil.test/sh/abc/h1",
            "https://ddownload.com/f/abc",
        ],
        refusals: &[
            (
                401,
                include_str!("../../../fixtures/dropbox/error_expired_access_token.json"),
                "dropbox_crawler.sign_in_required",
            ),
            (
                429,
                include_str!("../../../fixtures/dropbox/error_too_many_requests.json"),
                "dropbox_crawler.rate_limited",
            ),
            (
                409,
                include_str!("../../../fixtures/dropbox/error_path_not_found.json"),
                "dropbox_crawler.folder_unreachable",
            ),
        ],
        prose: &["A sentence Dropbox wrote"],
    },
    shape: Shape {
        slug: "dropbox",
        resolver: DROPBOX_RESOLVER,
        crawler: DROPBOX_CRAWLER,
        sign_in: DROPBOX_OAUTH,
        resolver_domains: &["api.dropboxapi.com", "content.dropboxapi.com"],
        crawler_domains: &["api.dropboxapi.com"],
        sign_in_domains: &["www.dropbox.com", "api.dropboxapi.com"],
        flows: &[Redirect],
        secret_reference: Some("dropbox_access_token"),
        // The RPC host the resolver calls and the content host the transfer goes to, which is
        // what lets the scheduler put the bearer on the transfer itself (RD-106-04, rule 6).
        secret_domains: Some(&["api.dropboxapi.com", "content.dropboxapi.com"]),
        download_domains: Some(&["content.dropboxapi.com"]),
        sign_in_secrets: &[],
    },
};

const GOOGLE_RESOLVER: &str = include_str!("../../../../../../plugins/google-drive/manifest.toml");
const GOOGLE_CRAWLER: &str =
    include_str!("../../../../../../plugins/google-drive-crawler/manifest.toml");
const GOOGLE_OAUTH: &str =
    include_str!("../../../../../../plugins/google-drive-oauth/manifest.toml");

pub const GOOGLE_DRIVE: Provider = Provider {
    crawler: Crawler {
        component: "rd-plugin-google-drive-crawler",
        manifest: GOOGLE_CRAWLER,
        folder: "https://drive.google.com/drive/folders/redacted-folder-id-0001",
        claimed: &[
            "https://drive.google.com/drive/folders/1A2b3C",
            "https://drive.google.com/drive/u/0/folders/1A2b3C?usp=sharing",
            "https://drive.google.com/drive/u/1/shared-drives/1A2b3C",
            "https://drive.google.com/folderview?id=1A2b3C",
        ],
        foreign: &[
            "https://drive.google.com/file/d/1A2b3C/view",
            "https://docs.google.com/document/d/1A2b3C/edit",
            "https://www.googleapis.com/drive/v3/files/1A2b3C?alt=media",
            "https://drive.google.com.evil.test/drive/folders/1A2b3C",
            "https://ddownload.com/f/abc",
        ],
        // The two file-level 403s mean at a listing what neither means at the resolver: the
        // folder could not be read, and the crawler says exactly that.
        refusals: &[
            (
                401,
                include_str!("../../../fixtures/google_drive/file_auth_error.json"),
                "google_drive_crawler.sign_in_required",
            ),
            (
                429,
                include_str!("../../../fixtures/google_drive/file_rate_limited.json"),
                "google_drive_crawler.rate_limited",
            ),
            (
                403,
                include_str!("../../../fixtures/google_drive/file_quota_exceeded.json"),
                "google_drive_crawler.folder_unreachable",
            ),
            (
                403,
                include_str!("../../../fixtures/google_drive/file_virus_scan_warning.json"),
                "google_drive_crawler.folder_unreachable",
            ),
        ],
        prose: &["Invalid Credentials", "rate limit exceeded"],
    },
    shape: Shape {
        slug: "google_drive",
        resolver: GOOGLE_RESOLVER,
        crawler: GOOGLE_CRAWLER,
        sign_in: GOOGLE_OAUTH,
        resolver_domains: &["www.googleapis.com"],
        crawler_domains: &["www.googleapis.com"],
        sign_in_domains: &["accounts.google.com", "oauth2.googleapis.com"],
        flows: &[Redirect],
        secret_reference: Some("google_drive_access_token"),
        secret_domains: Some(&["www.googleapis.com"]),
        download_domains: None,
        sign_in_secrets: &[],
    },
};

const ONEDRIVE_RESOLVER: &str = include_str!("../../../../../../plugins/onedrive/manifest.toml");
const ONEDRIVE_CRAWLER: &str =
    include_str!("../../../../../../plugins/onedrive-crawler/manifest.toml");
const ONEDRIVE_OAUTH: &str = include_str!("../../../../../../plugins/onedrive-oauth/manifest.toml");

pub const ONEDRIVE: Provider = Provider {
    crawler: Crawler {
        component: "rd-plugin-onedrive-crawler",
        manifest: ONEDRIVE_CRAWLER,
        folder: "https://1drv.ms/f/s!AkXy_Zabc-DEF",
        claimed: &[
            "https://1drv.ms/f/s!AkXy_Zabc-DEF",
            "https://1drv.ms/f/c/ab12cd34ef56/EaBcDeFgHiJ?e=abc",
            "https://contoso-my.sharepoint.com/:f:/g/personal/someone_contoso_com/EaBc?e=x",
            "https://contoso.sharepoint.com/:f:/s/Team/EaBcDeF",
            "https://onedrive.live.com/?id=ABC123%21456&cid=ABC123",
            "https://onedrive.live.com/redir?resid=ABC123%21456&authkey=%21AB",
        ],
        foreign: &[
            "https://1drv.ms/u/s!AkXy_Zabc-DEF",
            "https://contoso.sharepoint.com/:b:/s/Team/EaBcDeF",
            "https://onedrive.live.com/view.aspx?resid=ABC123%21456",
            "https://graph.microsoft.com/v1.0/shares/u!aHR0/items/01ABC",
            "https://graph.microsoft.com/v1.0/shares/u!aHR0/driveItem",
            "https://1drv.ms.evil.test/f/s!abc",
            "https://ddownload.com/f/abc",
        ],
        // That a throttle carries Microsoft's own `Retry-After` is proven at the resolver, in
        // `plugins/onedrive/src/native/tests.rs`; a crawl refusal carries no category.
        refusals: &[
            (
                401,
                include_str!("../../../fixtures/onedrive/item_unauthenticated.json"),
                "onedrive_crawler.sign_in_required",
            ),
            (
                429,
                include_str!("../../../fixtures/onedrive/item_rate_limited.json"),
                "onedrive_crawler.rate_limited",
            ),
            (
                403,
                include_str!("../../../fixtures/onedrive/item_access_denied.json"),
                "onedrive_crawler.access_denied",
            ),
            (
                404,
                include_str!("../../../fixtures/onedrive/item_not_found.json"),
                "onedrive_crawler.folder_unreachable",
            ),
            (
                400,
                include_str!("../../../fixtures/onedrive/item_invalid_request.json"),
                "onedrive_crawler.folder_unreachable",
            ),
        ],
        prose: &["Access denied", "throttled", "could not be found"],
    },
    shape: Shape {
        slug: "onedrive",
        resolver: ONEDRIVE_RESOLVER,
        crawler: ONEDRIVE_CRAWLER,
        sign_in: ONEDRIVE_OAUTH,
        resolver_domains: &["graph.microsoft.com"],
        crawler_domains: &["graph.microsoft.com"],
        // Microsoft's sign-in hosts, plus the host a device code is typed at, because the
        // verification address is gated on it.
        sign_in_domains: &[
            "login.microsoftonline.com",
            "microsoft.com",
            "www.microsoft.com",
        ],
        flows: &[Redirect, Device],
        secret_reference: Some("onedrive_access_token"),
        secret_domains: Some(&["graph.microsoft.com"]),
        download_domains: None,
        sign_in_secrets: &[],
    },
};

const PCLOUD_RESOLVER: &str = include_str!("../../../../../../plugins/pcloud/manifest.toml");
const PCLOUD_CRAWLER: &str = include_str!("../../../../../../plugins/pcloud-crawler/manifest.toml");
const PCLOUD_OAUTH: &str = include_str!("../../../../../../plugins/pcloud-oauth/manifest.toml");

/// pCloud answers its refusals under HTTP 200; the number in the body is what refuses.
pub const PCLOUD: Provider = Provider {
    crawler: Crawler {
        component: "rd-plugin-pcloud-crawler",
        manifest: PCLOUD_CRAWLER,
        folder: "https://my.pcloud.com/#/filemanager?folder=42",
        // pCloud gives no handle but `fileid`: an address carrying one is the resolver's, an
        // address without one is the crawler's, whatever the link turns out to hold.
        claimed: &[
            "https://my.pcloud.com/#/filemanager?folder=42",
            "https://e.pcloud.com/#/filemanager?folder=0",
            "https://e.pcloud.link/publink/show?code=XZredacted",
            "https://my.pcloud.com/#page=publink&code=XZredacted",
            "https://u.pcloud.link/publink/show?code=XZredacted",
        ],
        foreign: &[
            "https://my.pcloud.com/#/filemanager?folder=42&fileid=5001",
            "https://e.pcloud.link/publink/show?code=XZredacted&fileid=7",
            "https://pcloud.com.evil.test/#/filemanager?folder=42",
            "https://api.pcloud.com/listfolder?folderid=42",
            "https://ddownload.com/f/abc",
        ],
        refusals: &[
            (
                200,
                include_str!("../../../fixtures/pcloud/error_invalid_access_token.json"),
                "pcloud_crawler.sign_in_required",
            ),
            (
                200,
                include_str!("../../../fixtures/pcloud/error_too_many_requests.json"),
                "pcloud_crawler.rate_limited",
            ),
            (
                200,
                include_str!("../../../fixtures/pcloud/error_file_not_found.json"),
                "pcloud_crawler.folder_unreachable",
            ),
            (
                200,
                include_str!("../../../fixtures/pcloud/error_link_unavailable.json"),
                "pcloud_crawler.link_unavailable",
            ),
        ],
        prose: &["A sentence pCloud wrote"],
    },
    shape: Shape {
        slug: "pcloud",
        resolver: PCLOUD_RESOLVER,
        crawler: PCLOUD_CRAWLER,
        sign_in: PCLOUD_OAUTH,
        resolver_domains: &["api.pcloud.com", "eapi.pcloud.com"],
        crawler_domains: &["api.pcloud.com", "eapi.pcloud.com"],
        sign_in_domains: &["my.pcloud.com", "api.pcloud.com", "eapi.pcloud.com"],
        flows: &[Redirect],
        secret_reference: None,
        secret_domains: None,
        // The content servers pCloud picks per request cannot be enumerated, and are not among
        // the hosts the token may be sent to: the ticket is already authorised.
        download_domains: Some(&["*.pcloud.com"]),
        // The application secret and nothing else: the sign-in writes the token through
        // `store-oauth-token` and has no business reading it back.
        sign_in_secrets: &["pcloud_client_secret"],
    },
};
