# Static Web runtime check

Run the real-server smoke test from the repository root:

```sh
./tests/staticweb/run.sh
```

The script builds Formicary, writes isolated Chamber configurations and static files under a temporary directory in `tests/staticweb/`, starts the server, and checks that Chamber-loaded routes serve their files. It covers the single `[staticweb]` table, repeated `[[staticweb]]` entries, index files, `HEAD`, unmatched routes, traversal rejection, and symlink containment where the platform allows symlink creation. Temporary runtime files are removed when the script exits.

Requirements: Cargo, Python 3, and a Unix-like shell environment. All runtime data is temporary and removed when the script exits.
