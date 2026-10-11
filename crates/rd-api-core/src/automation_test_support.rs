//! What the automation engine's tests share: the context its actions reach.

use std::time::Duration;

use crate::automation_actions::ActionContext;

/// What the actions may reach; nothing here dispatches a download.
pub(crate) async fn context(
    database: &rd_db::Database,
    directory: &std::path::Path,
) -> ActionContext {
    let secrets = rd_secrets::SecretStore::open(directory.join("secrets"))
        .await
        .expect("secrets");
    let scheduler = rd_scheduler::SchedulerHandle::start(
        database.clone(),
        rd_scheduler::SchedulerConfig::for_directory(directory.join("downloads")),
        secrets.clone(),
        None,
        Vec::new(),
    )
    .await
    .expect("scheduler");
    let extraction = rd_extract::ExtractionService::start(
        database.clone(),
        rd_extract::ExtractionConfig {
            default_passwords_file: directory.join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: directory.join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
    );
    ActionContext {
        database: database.clone(),
        secrets,
        scheduler,
        extraction,
        links: None,
    }
}
