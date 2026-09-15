# git-uplink

Rust Git subcommand for the Uplink operating model. The binary is named **`git-uplink`**, so Git treats it as `git uplink`.

It carries a patch queue on company `main` (`upstream/main` plus every patch that is not `merged` or `dropped`), prepares contributions (message cutoff, export author, affiliation scan), and submits through an upstream-owned private fork after IP approval.

```bash
cargo install --path .
git uplink status
git uplink web-ui
```

`git uplink web-ui` serves the operator dashboard from files **embedded in the binary**. `build.rs` runs `npm ci` / `npm run build` in `web/` and the dist is compiled in with `rust-embed`. The command opens a browser to the server (pass `--no-open` to skip).

Use `git-uplink -h` or `git uplink -h`. Plain `git uplink --help` goes through Git’s man-page path, not clap.

## Install

```bash
cargo build --release
export PATH="$PWD/target/release:$PATH"
git uplink -h
```

Building the crate needs **Rust 1.83+**, **Node.js 22** (for the embedded UI), and `npm`.

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
git uplink web-ui [--port 43721] [--bind 127.0.0.1] [--no-open]
```

`add` is the internal product gate (status `queued`). `approve` / `submit` are the IP gate. On GitHub Enterprise Cloud, dispatch the **oss** Environment workflow; `git uplink report` writes `.uplink/reports/<id>/prepare.md` and `GITHUB_STEP_SUMMARY`.

Queue state lives in `.uplink/queue.json` and `.uplink/patches/*.patch`. Rebuild copies the whole `.uplink` tree, including reports.

## Dashboard

`git uplink web-ui` is the operator UI: control room, live lab, collaboration notes, system playbook, [way-of-working.md](way-of-working.md), and a **This repo** page that reads `.uplink/queue.json` from the directory you started in.

Frontend sources live in `web/` (Vite + React + Tailwind). Do not commit `web/dist`; cargo rebuilds it.

## Tests

```bash
cargo test
```

The suite drives real git (temp repos): stacked patches, drop-on-merge, conflicts, concurrent adds, export preflight, prepare/scrub, OSS packets, plus a check that the UI was embedded.

`cargo test` also runs the Live lab scenario tests in `web/` (`npm test`) before embedding the dashboard. Those cases are the executable spec for drop-on-merge, internal-only staying off the fork, and every lab step completing. You can run them alone with `npm test --prefix web`.

## Product-repo workflows

Copy `templates/emu-workflows/` into the company product repository. Those jobs assume `git-uplink` is on `PATH`. Environment setup is in `templates/README.md`. Developer stories: [way-of-working.md](way-of-working.md).

## Playbook on `main`

The TypeScript playbook and Next.js dashboard remain on `main`. This branch is the Rust engine, CLI, and embedded UI.
