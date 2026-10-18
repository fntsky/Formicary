# Scheduled Job runtime check

Run the real-server smoke test from the repository root:

```sh
./tests/job/run.sh
```

The script builds Formicary, creates a temporary Chamber and database under `tests/job/`, starts the server with `tests/job/` as its working directory, and checks that a real shell command runs on its cron tick. It aligns server startup to seconds 28–30 of a minute so the first five-field `* * * * *` tick should happen about 30 seconds later. Depending on when you start the script, alignment can take up to about one minute. It verifies the command did not run immediately and that exactly one marker was written. Temporary runtime files are removed when the script exits.

Requirements: Cargo, Python 3, and a Unix-like shell environment. All runtime data is temporary and removed when the script exits.
