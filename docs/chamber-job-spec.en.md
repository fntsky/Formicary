# Chamber and Job Configuration Specification

This document describes the configuration format and runtime behavior used by the startup loader.

## 1. Configuration directory

Set `FORMICARY_CHAMBERS_DIR` in `.env` to the Chamber root directory. This path points directly to the directory containing Chamber subdirectories. Relative paths are resolved from the process working directory. If unset, the default is `./chamber`.

```dotenv
FORMICARY_CHAMBERS_DIR=./chamber
```

The current `make run` command exports values from the repository-root `.env` before starting the server. When launching the binary directly, the launcher must export these environment variables first; the server reads its process environment.

Directory layout:

```text
chamber/
  maintenance/
    chamber.toml
    scripts/
      cleanup.sh
  reporting/
    chamber.toml
```

At startup, the program scans only the root's immediate child directories; it does not recurse. A child directory containing `chamber.toml` is loaded as one Chamber. Directories without that file are ignored. Directories are processed in sorted name order for predictable startup behavior.

Startup must fail if the root directory is missing or unreadable, or if any `chamber.toml` cannot be parsed or validated. This prevents a configuration error from silently disabling jobs. To run with no jobs, create an empty `chamber/` directory.

## 2. `chamber.toml` format

Each Chamber has one `chamber.toml`. Version 1 example:

```toml
[chamber]
name = "maintenance"
schema_version = 1

[[jobs]]
name = "daily-cleanup"
enabled = true
cron = "30 2 * * *"
working_directory = "."
command = "./scripts/cleanup.sh"
timeout = "10m"

[[jobs]]
name = "health-report"
cron = "0 8 * * 1-5"
working_directory = "."
command = "./scripts/health-report.sh"
timeout = "30s"

[staticweb]
directory = "./public"
route = "/assets"
```

Rules:

- `[chamber]` is required. `name` must be non-empty and match the parent directory name. `schema_version` must currently be `1`.
- `[[jobs]]` may appear multiple times or be omitted. Job names must be non-empty and unique within a Chamber.
- Every Job must provide `name`, `cron`, `working_directory`, `command`, and `timeout`.
- `enabled` is optional and defaults to `true`. If `false`, the Job is loaded in a paused state.
- `[staticweb]` is optional and configures one static directory and its URL route; use repeated `[[staticweb]]` entries for multiple mounts.
- Unknown fields should be treated as configuration errors so that typos are not silently ignored.

## 3. Job fields

| Field | Type | Contract |
| --- | --- | --- |
| `name` | string | Required; unique within its Chamber; human-readable job name. |
| `enabled` | boolean | Optional; defaults to `true`. `false` means the Job starts paused. |
| `cron` | string | Required; standard five-field Unix cron: minute, hour, day of month, month, day of week. |
| `working_directory` | string | Required; relative paths resolve from the directory containing this `chamber.toml`; absolute paths are also allowed. The directory must exist. |
| `command` | string | Required; command text executed by the platform shell. |
| `timeout` | string | Required; a positive integer followed by `ms`, `s`, `m`, or `h`; greater than zero and no more than 24 hours. |

## 4. Static Web fields

`[staticweb]` and each `[[staticweb]]` entry contain:

| Field | Type | Contract |
| --- | --- | --- |
| `directory` | string | Required; static file directory. Relative paths resolve from the directory containing this `chamber.toml`; absolute paths are also allowed. The directory must exist. |
| `route` | string | Required; URL mount prefix such as `/assets`. Leading and trailing slashes are optional. Normalized routes must be unique across all loaded Chambers; `/api` and `/ui` prefixes are reserved for the application. |

At startup, the Chamber loader resolves and validates each directory and route, then registers it in the application's static directory registry. A request for `/assets/logo.svg` reads `directory/logo.svg`; requests for the mount prefix or a directory path try to serve that directory's `index.html`. Only `GET` and `HEAD` are served. Overlapping routes use the longest matching prefix. Traversal outside the mount and symlinks that point outside it are not served.

Mount routes must be unique across loaded Chambers. During runtime, a shared `App` handle can add or remove mounts; those runtime changes are not written back to `chamber.toml`. TOML edits still require a server restart.

## 5. Cron time

The specification accepts five-field Unix cron expressions only. Scheduling uses the server's local timezone; per-job timezones are not supported in version 1. Expressions are validated at startup, and an invalid expression causes startup to fail.

## 6. Working directory and command

Relative `working_directory` values resolve from the Chamber directory. For example, `"."` means the directory containing `chamber.toml`. `command` is executed by `ShellRunner` as shell text, so pipes, redirection, and shell syntax are available. Command configuration must come from trusted local files and must not be generated from untrusted user input.

`timeout` is the wall-clock limit for one command execution. When it expires, the runner terminates the command and its child processes and records a timeout. Standard output and standard error go to the service log. A non-zero exit code is logged but does not automatically pause the Job.

## 7. Startup and runtime behavior

- Configuration is loaded only when the program starts. TOML edits require a restart; version 1 does not define hot reload.
- `JobService` is a process-wide singleton. Loaded Jobs are held by shared references in their Chamber.
- A Job runs at most one command at a time. If its previous execution has not finished, the next occurrence does not start concurrently; missed occurrences are skipped.
- Pause/start state is runtime state and is not written back to TOML. After restart, the state is initialized from `enabled` again.
- A Job with `enabled = false` remains present in configuration and in listings, but its scheduled command is not run.

## 8. Validation failures

The loader should report the Chamber path and field context, then fail startup for unreadable TOML, an unsupported schema version, a Chamber name that differs from its directory name, duplicate or empty Job names, missing fields or wrong types, invalid cron syntax, invalid timeout format, a missing working directory, a missing path or non-directory staticweb directory, an invalid or duplicate staticweb route, or unknown fields.
