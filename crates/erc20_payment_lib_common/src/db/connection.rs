use crate::error::PaymentError;
use crate::error::*;
use crate::{err_custom_create, err_from};
use sqlx::migrate::{MigrateError, Migrator};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use std::env;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic;
use std::time::Duration;

static MIGRATOR: Migrator = sqlx::migrate!();

static MEMORY_DATABASE_NUMBER: atomic::AtomicUsize = atomic::AtomicUsize::new(0);

pub async fn create_sqlite_connection(
    path: Option<&Path>,
    memory_name: Option<&str>,
    read_only: bool,
    run_migrations: bool,
) -> Result<SqlitePool, PaymentError> {
    let url = if let Some(path) = path {
        format!(
            "sqlite://{}",
            path.to_str()
                .ok_or_else(|| err_custom_create!("path not convertible to string: {path:?}"))?
        )
    } else if let Some(memory_name) = memory_name {
        format!("file:{}?mode=memory", memory_name)
    } else {
        format!(
            "file:mem_{}?mode=memory",
            MEMORY_DATABASE_NUMBER.fetch_add(1, atomic::Ordering::Relaxed)
        )
    };

    let journal_mode = match env::var("ERC20_LIB_SQLITE_JOURNAL_MODE") {
        Ok(val) => sqlx::sqlite::SqliteJournalMode::from_str(&val).map_err(err_from!())?,
        Err(_) => sqlx::sqlite::SqliteJournalMode::Wal,
    };

    let conn_opt = SqliteConnectOptions::from_str(&url)
        .map_err(err_from!())?
        .journal_mode(journal_mode)
        .read_only(read_only)
        .busy_timeout(Duration::from_secs_f64(1.0))
        .create_if_missing(!read_only);

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(conn_opt)
        .await
        .map_err(err_from!())?;

    if run_migrations {
        MIGRATOR.run(&pool).await.map_err(|e| {
            let file_part = if let Some(path) = path {
                format!("file {}", path.display())
            } else {
                url
            };
            match e {
                MigrateError::VersionMissing(_) => {
                    err_custom_create!(
                        "Error during migration in {file_part}, probably previously run with newer version of application: {e}",
                    )
                }
                _ => {
                    err_custom_create!(
                        "Migration error in {file_part}: {e}",
                    )
                }
            }
        })?
    }

    Ok(pool)
}
