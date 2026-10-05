//! What one package's post-processing runs with.
//!
//! Every choice has the same precedence — the package's own value where it has one, else its
//! category's override, else the global setting — so it is resolved here, once, rather than
//! inline at each step of the pipeline (audit 1.9.1, INTAKE-08).

use rd_core::{Category, DownloadPackage, PostprocessLevel, PostprocessSettings};

use crate::{ExtractionTrigger, Inner, cleanup_job};

/// The resolved post-processing choices of one package.
pub(crate) struct PackageSettings {
    pub(crate) level: PostprocessLevel,
    pub(crate) script: Option<String>,
    pub(crate) rules: cleanup_job::CleanupRules,
    pub(crate) sfv_verify: bool,
    pub(crate) upload: Option<String>,
    pub(crate) delete_par2: bool,
    /// The installed ones only.
    pub(crate) plugin_steps: Vec<String>,
    pub(crate) malware_scan: bool,
    /// Off for a forced run, whatever the setting says.
    pub(crate) safe_postproc: bool,
    pub(crate) unpack_to_subfolder: bool,
    /// Usenet packages only, whatever the setting says (RD-1100-07).
    pub(crate) direct_unpack: bool,
    /// Off for torrents, whatever the setting says.
    pub(crate) recursive_unpack: bool,
    /// The category's sort templates (RD-1100-08); `None` for torrents, whose payload keeps
    /// seeding from where it is.
    pub(crate) sorting: Option<rd_core::SortTemplates>,
}

impl PackageSettings {
    pub(crate) fn resolve(
        inner: &Inner,
        package: &DownloadPackage,
        category: Option<&Category>,
        settings: &PostprocessSettings,
        trigger: ExtractionTrigger,
    ) -> Self {
        let mut level = package
            .postprocess_level
            .or(category.and_then(|category| category.postprocess_level))
            .unwrap_or_else(|| settings.effective_default_level());
        if trigger.is_manual() && level < PostprocessLevel::Unpack {
            level = PostprocessLevel::Unpack;
        }
        // A torrent payload may still be seeding; deleting archive volumes would corrupt it.
        if package.kind == rd_core::DownloadKind::Torrent {
            level = level.min(PostprocessLevel::Unpack);
        }
        let script = package
            .script
            .clone()
            .or(category.and_then(|category| category.script.clone()))
            .filter(|name| !name.trim().is_empty());
        let rules = cleanup_job::CleanupRules {
            // Same precedence as the level and script above: the category overrides the global
            // list, and an empty override switches cleanup off for that category.
            extensions: category
                .and_then(|category| category.cleanup_extensions.clone())
                .unwrap_or_else(|| settings.cleanup_extensions.clone()),
            ignore_samples: settings.ignore_samples,
            sample_max_bytes: settings.sample_max_bytes.get(),
        };
        // Same precedence as level, script and cleanup: category override, else the global setting.
        let sfv_verify = category
            .and_then(|category| category.sfv_verify)
            .unwrap_or(settings.sfv_verify);
        // Upload target with the usual precedence: category override, else the global setting.
        let upload_enabled = category
            .and_then(|category| category.upload_enabled)
            .unwrap_or(settings.upload_enabled);
        let upload = upload_enabled
            .then(|| {
                category
                    .and_then(|category| category.upload_remote.clone())
                    .or_else(|| settings.upload_remote.clone())
            })
            .flatten()
            .map(|remote| remote.trim().to_owned())
            .filter(|remote| !remote.is_empty());
        // Same precedence again: category override, else the global setting.
        let delete_par2 = category
            .and_then(|category| category.delete_par2)
            .unwrap_or(settings.delete_par2);
        // Same precedence once more, with one difference: an empty category list is not "no
        // override" but "none here", which is how a category switches a globally enabled step off.
        // Steps whose plugin is not installed are dropped while planning rather than queued as
        // rows nothing would ever pick up.
        let plugin_steps: Vec<String> = category
            .and_then(|category| category.plugin_steps.clone())
            .unwrap_or_else(|| settings.plugin_steps.clone())
            .into_iter()
            .filter(|plugin_id| inner.plugin_step_installed(plugin_id))
            .collect();
        // Same precedence once more: category override, else the global switch (RD-190-14).
        let malware_scan = category
            .and_then(|category| category.malware_scan)
            .unwrap_or(settings.malware_scan_enabled);
        // SABnzbd's `safe_postproc`: the *only* place a verification failure decides what else
        // runs. With it off — or for one run, when somebody asked for it anyway — intact
        // archives beside a broken recovery set are unpacked instead of being locked away.
        let safe_postproc = category
            .and_then(|category| category.safe_postproc)
            .unwrap_or(settings.safe_postproc)
            && trigger != ExtractionTrigger::Force;
        // Same precedence as the rest: category override, else the global setting (RD-170-16).
        let unpack_to_subfolder = category
            .and_then(|category| category.unpack_to_subfolder)
            .unwrap_or(settings.unpack_to_subfolder);
        // Same precedence again (RD-1100-07). Only Usenet tells a complete file from one with
        // missing articles before the package is verified, which is what lets a volume be
        // handed to the tool while the rest is still arriving.
        let direct_unpack = category
            .and_then(|category| category.direct_unpack)
            .unwrap_or(settings.direct_unpack)
            && package.kind == rd_core::DownloadKind::Usenet;
        // A torrent payload may still be seeding; deleting the inner intermediates that
        // recursion produces would corrupt it, so recursion stays off for torrents.
        let recursive_unpack = category
            .and_then(|category| category.recursive_unpack)
            .unwrap_or(settings.recursive_unpack)
            && package.kind != rd_core::DownloadKind::Torrent;
        // No precedence: sorting is the category's alone, there is no global template.
        let sorting = category
            .and_then(|category| category.sorting.clone())
            .and_then(rd_core::SortTemplates::normalized)
            .filter(|_| package.kind != rd_core::DownloadKind::Torrent);
        Self {
            level,
            script,
            rules,
            sfv_verify,
            upload,
            delete_par2,
            plugin_steps,
            malware_scan,
            safe_postproc,
            unpack_to_subfolder,
            direct_unpack,
            recursive_unpack,
            sorting,
        }
    }
}
