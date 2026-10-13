//! The database half of the kit (feature `db`).

use std::path::Path;

use rd_db::Database;

/// The file [`temp_database`] opens inside its directory.
pub const DATABASE_FILE: &str = "rdownloader-test.sqlite3";

/// Opens, creating and migrating it, the database file [`DATABASE_FILE`] in `dir`.
///
/// # Panics
///
/// When the database cannot be opened; a test cannot go on without it.
pub async fn temp_database(dir: &Path) -> Database {
    Database::open(dir.join(DATABASE_FILE))
        .await
        .expect("open the test database")
}
