---
name: formicary-local-run
description: Start or smoke-check Formicary from its ignored `tmp/` directory during AI-assisted local development. Use this only for development checks, not as the project's official run or deployment workflow.
---

# Formicary Local Run

Use this only when Codex needs to launch Formicary temporarily while changing or inspecting this repository. This is an AI development aid, not the repository's standard startup procedure, deployment instructions, or production runbook. Leave the Makefile and production documentation unchanged for this convention.

For an AI development smoke run:

1. Work from the Formicary repository root. Keep `/tmp/` in the root `.gitignore`; add it if it is missing.
2. Build only what is needed. Prefer an existing `target/release/Formicary`; otherwise run `cargo build --release`. Build the UI with `npm run build --prefix ui` only when its distribution files are missing or need rebuilding.
3. Ensure `tmp/ui` points to the repository's `ui` directory so the binary can serve the built UI while its current directory is `tmp/`. If `tmp/ui` already exists as a real directory or points elsewhere, inspect it and preserve its contents rather than replacing it automatically.
4. Load the repository `.env` before changing directories, without printing its contents. Resolve a relative `FORMICARY_DATABASE_PATH` against the repository root so the run reuses the existing local database. Set `FORMICARY_CHAMBERS_DIR` to `$repo_root/tmp/chamber`; this keeps AI development Chamber modules in the ignored `tmp/` directory and leaves the repository's normal `chamber/` tree out of this temporary run.
5. Start the binary with `tmp/` as its working directory. For example, after setting `repo_root` and `database_path` as above:

   ```sh
   cd "$repo_root/tmp"
   FORMICARY_DATABASE_PATH="$database_path" \
   FORMICARY_CHAMBERS_DIR="$repo_root/tmp/chamber" \
   "$repo_root/target/release/Formicary"
   ```

Keep the process attached to a manageable terminal session so it can be stopped with Ctrl-C. Verify startup with the configured health endpoint or requested page, then report the URL and that this was an AI development run. Do not remove `tmp/` or its contents wholesale; it may contain local runtime data.
