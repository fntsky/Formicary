---
name: sqlite-migrations
description: Add or review SQLite schema changes in Formicary using one immutable, sequential Rust migration file per database version.
---

# Formicary SQLite migrations

Use this skill whenever a change adds, removes, or modifies persistent SQLite schema or stored data.

## Required workflow

1. Read the current database version from SQLite `PRAGMA user_version` and inspect `src/migrations.rs` plus the existing files in `src/migrations/`.
2. Add the next sequential migration as `src/migrations/vN.rs`. Put every schema or data transformation for that version in this file.
3. Register the migration in `src/migrations.rs` and advance `LATEST_VERSION` to the same number. The migration runner applies each missing version in order and stores the new `user_version` in the same transaction.
4. Never edit a migration that may already have been applied. Make a new `vN.rs` for every later database change, even when the change is small.
5. Preserve existing data, make the migration safe to run within a transaction, and keep unrelated startup logic out of migration files.
6. Keep `PRAGMA user_version` as the single schema-version source. Do not add a second version table or update the version outside the migration runner.

## Review checklist

- Versions are contiguous and the dispatcher includes every version up to `LATEST_VERSION`.
- The new migration is the only place that performs its schema or data change.
- Existing databases migrate forward without losing user or session data unless the migration explicitly requires a reviewed transformation.
- Startup rejects a database whose version is newer than the code supports.
