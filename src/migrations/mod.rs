mod v1;
mod v2;

use std::error::Error;
use std::fmt;

use rusqlite::Connection;

pub const LATEST_VERSION: u32 = 2;

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
            2 => v2::up(&transaction)?,
            missing => return Err(MigrationError::MissingMigration(missing)),
        }
        transaction.pragma_update(None, "user_version", next_version)?;
        transaction.commit()?;
        current_version = next_version;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn migrating_v1_preserves_users_and_sessions() {
        let mut database = Connection::open_in_memory().unwrap();
        database.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        let transaction = database.transaction().unwrap();
        v1::up(&transaction).unwrap();
        transaction.pragma_update(None, "user_version", 1).unwrap();
        transaction.commit().unwrap();

        database
            .execute(
                "INSERT INTO users (id, username, password_hash, created_at) VALUES (1, 'admin', 'hash', 0)",
                [],
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO sessions (token, user_id, expires_at) VALUES ('session', 1, 9999999999)",
                [],
            )
            .unwrap();

        migrate(&mut database).unwrap();

        let version: u32 = database
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, LATEST_VERSION);
        let users: i64 = database
            .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
            .unwrap();
        let sessions: i64 = database
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .unwrap();
        let keys: i64 = database
            .query_row("SELECT COUNT(*) FROM api_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(users, 1);
        assert_eq!(sessions, 1);
        assert_eq!(keys, 0);

        let key_id: String = "0123456789abcdef01234567".into();
        database
            .execute(
                "INSERT INTO api_keys (user_id, name, key_id, secret_hash, created_at)
                 VALUES (1, 'ci', ?1, ?2, 0)",
                params![key_id, [0u8; 32].as_slice()],
            )
            .unwrap();
    }
}
