mod v1;

use std::error::Error;
use std::fmt;

use rusqlite::Connection;

pub const LATEST_VERSION: u32 = 1;

#[derive(Debug)]
pub enum MigrationError {
    Sqlite(rusqlite::Error),
    DatabaseTooNew { database: u32, supported: u32 },
    MissingMigration(u32),
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(formatter, "SQLite error: {error}"),
            Self::DatabaseTooNew {
                database,
                supported,
            } => write!(
                formatter,
                "database version {database} is newer than supported version {supported}"
            ),
            Self::MissingMigration(version) => {
                write!(formatter, "migration v{version} is not registered")
            }
        }
    }
}

impl Error for MigrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::DatabaseTooNew { .. } | Self::MissingMigration(_) => None,
        }
    }
}

impl From<rusqlite::Error> for MigrationError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

pub fn migrate(database: &mut Connection) -> Result<(), MigrationError> {
    let mut current_version: u32 =
        database.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current_version > LATEST_VERSION {
        return Err(MigrationError::DatabaseTooNew {
            database: current_version,
            supported: LATEST_VERSION,
        });
    }

    while current_version < LATEST_VERSION {
        let next_version = current_version + 1;
        let transaction = database.transaction()?;
        match next_version {
            1 => v1::up(&transaction)?,
            missing => return Err(MigrationError::MissingMigration(missing)),
        }
        transaction.pragma_update(None, "user_version", next_version)?;
        transaction.commit()?;
        current_version = next_version;
    }

    Ok(())
}
