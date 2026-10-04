//! RD-190-06: what the pipeline hands a plugin step about the files it removed, and what it
//! keeps of a step that passed with a warning.

use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use rd_core::{PostprocessKind, PostprocessState};
use rd_db::{Database, NewPackage};
use zip::write::SimpleFileOptions;

use crate::{
    ExtractionConfig, ExtractionService, ExtractionTrigger, PluginStepJob, PluginStepOutcome,
    PluginStepRunner, PluginStepWarning,
    tests::{seed_completed_file, wait_until_finished},
};

const STEP: &str = "019d0000-0000-7000-8000-00000000f00d";

/// A step that writes down what it was offered and answers with one warning.
struct Recording {
    seen: Mutex<Vec<(Vec<String>, Vec<String>)>>,
}

#[async_trait]
impl PluginStepRunner for Recording {
    fn installed(&self, plugin_id: &str) -> bool {
        plugin_id == STEP
    }

    async fn run(&self, _plugin_id: &str, job: PluginStepJob<'_>) -> Result<PluginStepOutcome> {
        self.seen
            .lock()
            .expect("lock")
            .push((job.files.to_vec(), job.removed.to_vec()));
        Ok(PluginStepOutcome::Complete {
            warnings: vec![PluginStepWarning {
                code: "fake_step.unchecked".to_owned(),
                params: [("count".to_owned(), "1".to_owned())].into_iter().collect(),
                message: "1 listed file was not checked".to_owned(),
            }],
        })
    }
}

/// `Film.zip` holding the film and an `.nfo` the cleanup takes away.
fn film_zip() -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, content) in [("film.mkv", &b"film"[..]), ("film.nfo", &b"nfo"[..])] {
        zip.start_file(name, SimpleFileOptions::default())
            .expect("start member");
        zip.write_all(content).expect("write member");
    }
    zip.finish().expect("finish ZIP").into_inner()
}

#[tokio::test]
async fn a_step_is_told_what_the_pipeline_removed_and_its_warning_stays_on_the_step() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({
                "default_level": "delete",
                "unpack_to_subfolder": true,
                "cleanup_extensions": ["nfo"],
                "plugin_steps": [STEP],
            }),
        )
        .await
        .expect("settings");
    let destination = temp.path().join("dl");
    std::fs::create_dir_all(&destination).expect("destination");
    let package = database
        .create_package(NewPackage {
            id: rd_core::PackageId::new(),
            name: "film".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    std::fs::write(destination.join("Film.zip"), film_zip()).expect("archive");
    seed_completed_file(&database, package.id, "Film.zip").await;
    let recording = Arc::new(Recording {
        seen: Mutex::new(Vec::new()),
    });
    let service = ExtractionService::start_with_plugins(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.path().join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: temp.path().join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
        Some(Arc::clone(&recording) as Arc<dyn PluginStepRunner>),
        None,
        None,
    );

    service
        .request(package.id, ExtractionTrigger::Manual)
        .await
        .expect("request");
    wait_until_finished(&service, package.id).await;
    service.shutdown().await;

    let seen = recording.seen.lock().expect("lock").clone();
    assert_eq!(
        seen,
        vec![(
            vec!["Film/film.mkv".to_owned()],
            // The unpacked volume and what the cleanup took out of the unpacked content.
            vec!["Film.zip".to_owned(), "Film/film.nfo".to_owned()],
        )]
    );
    let steps = database
        .list_postprocess_steps(&package.id.to_string())
        .await
        .expect("steps");
    let step = steps
        .iter()
        .find(|step| step.kind == PostprocessKind::PluginStep && step.source_path == STEP)
        .expect("the plugin step");
    assert_eq!(step.state, PostprocessState::Completed, "{step:?}");
    assert_eq!(step.code.as_deref(), Some("fake_step.unchecked"));
    assert_eq!(step.params.get("count").map(String::as_str), Some("1"));
    assert_eq!(
        step.message.as_deref(),
        Some("1 listed file was not checked")
    );
}

const RENAME: &str = "019d0000-0000-7000-8000-00000000fe11";

/// A first step that renames the film, and a second that writes down what it was offered.
struct RenameThenRecord {
    seen: Mutex<Vec<Vec<String>>>,
}

#[async_trait]
impl PluginStepRunner for RenameThenRecord {
    fn installed(&self, plugin_id: &str) -> bool {
        plugin_id == RENAME || plugin_id == STEP
    }

    async fn run(&self, plugin_id: &str, job: PluginStepJob<'_>) -> Result<PluginStepOutcome> {
        if plugin_id == RENAME {
            std::fs::rename(
                job.directory.join("Film").join("film.mkv"),
                job.directory.join("Film").join("Film.2026.mkv"),
            )?;
        } else {
            self.seen.lock().expect("lock").push(job.files.to_vec());
        }
        Ok(PluginStepOutcome::Complete {
            warnings: Vec::new(),
        })
    }
}

/// RD-191-06, PLUG-05: a step after a rename is offered the new name. The list used to be made
/// once for all steps, so the second one asked for a file that was no longer there.
#[tokio::test]
async fn a_step_after_a_rename_is_offered_the_new_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let database = Database::open(temp.path().join("extract.sqlite"))
        .await
        .expect("database");
    database
        .set_setting(
            "service.settings".to_owned(),
            serde_json::json!({
                "default_level": "delete",
                "unpack_to_subfolder": true,
                "cleanup_extensions": ["nfo"],
                "plugin_steps": [RENAME, STEP],
            }),
        )
        .await
        .expect("settings");
    let destination = temp.path().join("dl");
    std::fs::create_dir_all(&destination).expect("destination");
    let package = database
        .create_package(NewPackage {
            id: rd_core::PackageId::new(),
            name: "film".to_owned(),
            destination: destination.to_string_lossy().into_owned(),
            category_id: None,
            priority: rd_core::DownloadPriority::Normal,
            postprocess_level: None,
            script: None,
            enrichment: Vec::new(),
        })
        .await
        .expect("package");
    std::fs::write(destination.join("Film.zip"), film_zip()).expect("archive");
    seed_completed_file(&database, package.id, "Film.zip").await;
    let runner = Arc::new(RenameThenRecord {
        seen: Mutex::new(Vec::new()),
    });
    let service = ExtractionService::start_with_plugins(
        database.clone(),
        ExtractionConfig {
            default_passwords_file: temp.path().join("passwords.txt"),
            rar_timeout: Duration::from_secs(5),
            default_scripts_directory: temp.path().join("scripts"),
            hold: rd_core::PostprocessHold::new(),
            quiet_hold: rd_core::PostprocessHold::new(),
            upload_limit: None,
        },
        Some(Arc::clone(&runner) as Arc<dyn PluginStepRunner>),
        None,
        None,
    );

    service
        .request(package.id, ExtractionTrigger::Manual)
        .await
        .expect("request");
    wait_until_finished(&service, package.id).await;
    service.shutdown().await;

    assert_eq!(
        runner.seen.lock().expect("lock").clone(),
        vec![vec!["Film/Film.2026.mkv".to_owned()]]
    );
}
