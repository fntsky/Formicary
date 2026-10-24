use rusqlite::{Result, Transaction};

pub(super) fn up(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS api_keys (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
             name TEXT NOT NULL,
             key_id TEXT NOT NULL UNIQUE,
             secret_hash BLOB NOT NULL,
             created_at INTEGER NOT NULL,
             last_used_at INTEGER,
             revoked_at INTEGER,
             CHECK (length(key_id) = 24),
             CHECK (length(secret_hash) = 32),
             CHECK (length(name) BETWEEN 1 AND 64)
         );
         CREATE INDEX IF NOT EXISTS api_keys_user_id ON api_keys(user_id);",
    )
}
