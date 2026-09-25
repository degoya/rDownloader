//! A resolver for XFileSharing installations that carries no site of its own.
//!
//! XFileSharing (XFS) is a hosting script many sites run largely unmodified, which is why
//! `plugins/xfs-common` exists at all: `ddownload`, `katfile` and `filejoker` already drive the
//! same request shapes, the same two-form free flow and the same page markers. This plugin is
//! what is left when the site-specific parts are removed — the standard free flow and nothing
//! else, so one plugin can serve several clones instead of one plugin per clone.
//!
//! What it deliberately does not do:
//!
//! * **No accounts.** A `[provider]` row names one `cookie_scope` and one `secret_reference`,
//!   and a plugin spanning clones can fill neither, because an account at one clone is not an
//!   account at another. It declares `credentials = "none"` instead and stays out of the
//!   accounts list. Whoever has an account at a particular clone still wants a plugin of its
//!   own, the way the three above are.
//! * **No site-specific markers.** A clone that deviates from the standard flow fails with a
//!   named cause rather than being special-cased here; such a clone belongs in its own plugin.
//!
//! The domain list is empty until each entry has been checked against a real link — see
//! `manifest.toml` for why that is the safe state rather than an unfinished one.

/// Hosts this plugin claims, and the single authority for it inside the code.
///
/// Empty on purpose, and it must stay in step with `match_domains` in `manifest.toml`: a host
/// listed in one and not the other is either a link claimed with no way to fetch it or a grant
/// for a link nobody claims. `matches()` consults this list and nothing else, which is what
/// keeps the `does_not_claim_foreign_links` conformance check satisfied by construction.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const HOSTERS: &[&str] = &[
    "1uploadflix.net",
    "1vid.xyz",
    "abstream.to",
    "anafast.org",
    "anonfile.de",
    "asfile.al",
    "audinifer.com",
    "beatexs.com",
    "bezsms.org",
    "brupload.net",
    "btafile.com",
    "callistanise.com",
    "cdn1.site",
    "cdnplus.org",
    "clickndownload.link",
    "clickndownload.org",
    "clicknplay.to",
    "clicknupload.cc",
    "clicknupload.cfd",
    "clicknupload.click",
    "clicknupload.co",
    "clicknupload.download",
    "clicknupload.me",
    "clicknupload.name",
    "clicknupload.one",
    "clicknupload.org",
    "clicknupload.space",
    "clicknupload.to",
    "clicknupload.vip",
    "cloubix.com",
    "cloud-share.in",
    "cloudghost.net",
    "cloudwish.xyz",
    "cosmobox.org",
    "croea.com",
    "dailyuploads.im",
    "dailyuploads.io",
    "dailyuploads.net",
    "daofile.com",
    "datanodes.to",
    "datavaults.co",
    "depic.me",
    "dhtpre.com",
    "dl.uploadflix.com",
    "dosya.co",
    "downloadani.me",
    "downloadwella.com",
    "downup.me",
    "dr0pstream.com",
    "dropload.co",
    "dropload.pro",
    "dsht.link",
    "dupload.net",
    "earnvids.com",
    "easybytez.org",
    "ex-load.com",
    "ezvn.net",
    "fastbit.cc",
    "fastfile.cc",
    "fastream.to",
    "fastt.gg",
    "file-me.top",
    "file-upload.cc",
    "file-upload.download",
    "file-upload.io",
    "file-upload.org",
    "file.al",
    "fileaxa.com",
    "filedot.to",
    "filedot.top",
    "filedot.xyz",
    "filefox.cc",
    "filekeeper.net",
    "fileq.net",
    "fileserve.com",
    "filespace.com",
    "filespayouts.com",
    "filestore.me",
    "fileupload.pw",
    "filextras.com",
    "filezz.cloud",
    "filoz.net",
    "firefiles.org",
    "flashbit.cc",
    "forafile.com",
    "freedl.ink",
    "fsvid.lol",
    "goodstream.one",
    "goodstream.uno",
    "guccihide.store",
    "hanerix.com",
    "hd-vk.com",
    "hdvid.tv",
    "hexload.com",
    "hglamioz.com",
    "hgplaycdn.com",
    "hgplayer.sbs",
    "hot4share.com",
    "hotlink.cc",
    "imagenpic.com",
    "imageshimage.com",
    "imagetwist.com",
    "imagexport.com",
    "imgnova.cc",
    "imgpv.com",
    "isra.cloud",
    "javplaya.com",
    "kenfiles.com",
    "larhu.website",
    "lulu0.ovh",
    "lulustream.com",
    "luluvdoo.com",
    "luluvid.com",
    "media.cm",
    "medixiru.com",
    "mega4up.com",
    "mega4up.org",
    "mega4upload.com",
    "mega4upload.net",
    "miravd.com",
    "miravid.club",
    "modsbase.com",
    "moflix-stream.click",
    "moondl.com",
    "mp4plus.org",
    "mp4upload.com",
    "mwdy.cc",
    "mwdy.org",
    "mycloudz.cc",
    "myfiles.onl",
    "myqloud.org",
    "nikaplayer.com",
    "niramirus.com",
    "nowplay.to",
    "obligao.com",
    "okprime.site",
    "phun.imagetwist.com",
    "playerwish.com",
    "playnixes.com",
    "playrecord.biz",
    "pubg-file.si",
    "qtyfiles.com",
    "rapidcloud.cc",
    "rarlink.com",
    "recordplay.biz",
    "redirect.codes",
    "rutood.com",
    "safedock.io",
    "sakurafile.com",
    "savefiles.com",
    "seedgrabber.com",
    "send.cm",
    "send.now",
    "silkfile.com",
    "streamhg.com",
    "streamruby.com",
    "streamwish.com",
    "subyshare.com",
    "supervideo.cc",
    "tenstream.net",
    "terabytez.org",
    "thefileslocker.com",
    "theuser.cloud",
    "transfaze.com",
    "up-4.net",
    "up-4ever.com",
    "up-4ever.net",
    "up-4ever.org",
    "upbolt.to",
    "upload-4ever.com",
    "upload.tn",
    "upload42.com",
    "uploadao.com",
    "uploadcloud.pro",
    "uploadfiles.eu",
    "uploadflix.cc",
    "uploadflix.com",
    "uploadhive.com",
    "uploadhub.dad",
    "uploadhub.ws",
    "uploadrar.com",
    "uploady.io",
    "uplod.ir",
    "upzur.com",
    "uqload.co",
    "uqload.com",
    "uqload.cx",
    "uqload.io",
    "uqload.is",
    "uqload.net",
    "uqload.ws",
    "usersdrive.com",
    "vibuxer.com",
    "vidhide.com",
    "vidhidehub.com",
    "vidoba.org",
    "vidspeed.org",
    "vidzy.org",
    "vipfile.cc",
    "vipr.im",
    "vtbe.to",
    "vtube.network",
    "wipfiles.net",
    "wishonly.site",
    "world-files.com",
    "xdisk.site",
    "xfilesharing.com",
    "ximagesharing.com",
    "xubster.com",
    "xvideosharing.com",
    "xxembed.com",
];

mod messages;

#[cfg(target_arch = "wasm32")]
mod guest;
mod page;
mod resolver;

/// This plugin's own packaging manifest: the single authority for its identity, domains and
/// capability grants. The native build reads it from here, the component build gets the same
/// file out of the package the host installed, so neither can describe the plugin differently.
#[cfg(not(target_arch = "wasm32"))]
pub const MANIFEST: &str = include_str!("../manifest.toml");

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::XfsGenericResolver;

#[cfg(test)]
mod manifest_tests {
    use super::{HOSTERS, MANIFEST};

    /// Every list in the manifest, by key, as a flat set of strings.
    fn manifest_list(key: &str) -> Vec<String> {
        let start = MANIFEST
            .find(&format!("\n{key} = ["))
            .unwrap_or_else(|| panic!("manifest has no {key}"));
        let rest = &MANIFEST[start..];
        let end = rest.find("\n]").expect("list is closed");
        rest[..end]
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    /// `matches()` consults [`HOSTERS`] and nothing else, while the registry routes links by the
    /// manifest's `match_domains`. A host in one and not the other is silent: either a link this
    /// plugin is handed and refuses, or one it would take and never sees. Nothing at runtime
    /// would say so, which is why it is pinned here.
    #[test]
    fn hosters_and_match_domains_are_the_same_set() {
        let mut manifest = manifest_list("match_domains");
        manifest.sort();
        let mut code: Vec<String> = HOSTERS.iter().map(|host| (*host).to_owned()).collect();
        code.sort();
        assert_eq!(
            code, manifest,
            "HOSTERS in lib.rs and match_domains in manifest.toml must hold the same hosts"
        );
    }

    /// A claimed host the sandbox would refuse is a link taken and then failed at the first
    /// request; the manifest's own two gates have to cover what `match_domains` promises.
    #[test]
    fn every_claimed_host_is_reachable_and_deliverable() {
        for list in ["download_domains", "domains"] {
            let allowed = manifest_list(list);
            for host in HOSTERS {
                assert!(
                    allowed.iter().any(|entry| entry == host),
                    "{host} is claimed but missing from {list}"
                );
                // XFS installations hand the file to a delivery host of their own.
                assert!(
                    allowed.iter().any(|entry| entry == &format!("*.{host}")),
                    "{host}'s delivery subdomains are missing from {list}"
                );
            }
        }
    }

    #[test]
    fn no_host_is_claimed_twice() {
        let mut seen = std::collections::BTreeSet::new();
        for host in HOSTERS {
            assert!(seen.insert(*host), "{host} is listed twice");
        }
    }
}
