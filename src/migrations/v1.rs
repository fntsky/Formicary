use rusqlite::{Result, Transaction};

pub(super) fn up(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS users (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             username TEXT NOT NULL UNIQUE,
             password_hash TEXT NOT NULL,
             created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS sessions (
             token TEXT PRIMARY KEY,
             user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
             expires_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS sessions_expires_at ON sessions(expires_at);",
    )
}
