# git-uplink

Rust port of the enterprise contribution bridge. The binary is named **`git-uplink`**, so Git treats it as a subcommand:

```bash
cargo install --path .
git uplink status
```

It carries a patch queue on company `main` (`upstream/main` plus every patch that is not `merged` or `dropped`), prepares contributions (message cutoff, export author, affiliation scan), and submits through an upstream-owned private fork after IP approval.

## Install

```bash
cargo build --release
# put target/release/git-uplink on PATH
export PATH="$PWD/target/release:$PATH"
git uplink --help
```

Or `cargo install --path .` (installs `~/.cargo/bin/git-uplink`).

## Commands

```text
git uplink init [--upstream <url>] [--contrib <url>]
git uplink add --title <text> [--from <ref>] [--head <ref>] [--internal-only]
            [--pr <n>] [--pr-url <url>] [--depends-on <id>]... [--push]
git uplink preflight [<id>] [--from <ref>] [--head <ref>] [--depends-on <id>]...
git uplink prepare [--from <ref>] [--head <ref>] [--title <text>] [--pr <n>]
git uplink report <id> [--out <file>]
git uplink status
git uplink approve <id>
git uplink submit <id>
git uplink sync
git uplink merged <id> [--via pr|trailer|manual]
git uplink drop <id> --reason <text>
git uplink rebuild
git uplink resolve <id>
```

`add` is the internal product gate (status `queued`). `approve` / `submit` are the IP gate. On GitHub Enterprise Cloud, dispatch the **oss** Environment workflow; `git uplink report` writes `.uplink/reports/<id>/prepare.md` and `GITHUB_STEP_SUMMARY`.

Queue state lives in `.uplink/queue.json` and `.uplink/patches/*.patch`. Rebuild copies the whole `.uplink` tree, including reports.

## Tests

```bash
cargo test
```

The suite drives real git (temp repos): stacked patches, drop-on-merge, conflicts, concurrent adds, export preflight, prepare/scrub, OSS packets.

## Product-repo workflows

Copy `templates/emu-workflows/` into the company product repository. Those jobs assume `git-uplink` is on `PATH` (install this crate on the runner, or vendor it and `cargo install --path vendor/git-uplink`).

## Relation to the TypeScript dashboard

The playbook and operator UI stay on `main`. This branch is only the Rust engine and CLI, so the tool can be installed as a Git subcommand.
