# git-uplink

Rust Git subcommand for the Uplink operating model. The binary is **`git-uplink`**, so Git treats it as `git uplink`.

It is for a company on GitHub Enterprise Cloud with Enterprise Managed Users that must build on a public project, keep unreleased work private, pass IP review, and contribute through an **upstream-owned private fork**. Developers keep one internal branch per change. Company `main` is bot-owned and always:

```text
public upstream/main  +  every patch that is not merged or dropped
```

`add` is the internal product gate (status `queued`). `approve` / `submit` are the IP gate. After a submitted patch is conflict-resolved it becomes `amended` until IP approves the delta. On GitHub Enterprise Cloud, dispatch the **oss** Environment workflow (resolve of a submitted patch does this for you); `git uplink report` writes `.uplink/reports/<id>/prepare.md` on `uplink/state` and `GITHUB_STEP_SUMMARY`.

```bash
cargo install --path .
git uplink status
git uplink web-ui
```

`git uplink web-ui` serves the operator dashboard from files **embedded in the binary**. `build.rs` runs `npm ci`, `npm test`, and `npm run build` in `web/`; the dist is compiled in with `rust-embed`. The command opens a browser (pass `--no-open` to skip).

Use `git-uplink -h` or `git uplink -h`. Plain `git uplink --help` goes through Git’s man-page path, not clap.

## Install

Building the crate needs **Rust 1.98+** (`rust-toolchain.toml` pins 1.98.1), **Node.js 22** (for the embedded UI), and `npm`.

```bash
cargo install --path .
git uplink -h
```

For a local binary without installing into Cargo’s bin directory:

```bash
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

## Commands

```text
git uplink init [--upstream <url>] [--contrib <url>]
            [--upstream-remote-name <name>] [--upstream-branch <branch>]
            [--contrib-remote-name <name>] [--internal-branch <branch>]
git uplink add --title <text> [--message <text> | --message-file <path>]
            [--from <ref>] [--head <ref>] [--internal-only]
            [--pr <n>] [--pr-url <url>] [--depends-on <id>]...
            [--push] [--refresh <remote>] [--push-remote <remote>]
git uplink preflight [<id>] [--from <ref>] [--head <ref>] [--title <text>]
            [--message <text> | --message-file <path>]
            [--depends-on <id>]...
git uplink prepare [--from <ref>] [--head <ref>] [--title <text>]
            [--message <text> | --message-file <path>]
            [--internal-only]
git uplink report <id> [--out <file>]
git uplink status
git uplink approve <id> [--out <file>]
git uplink submit <id>
git uplink submitted <id> --pr-url <url> [--pr <n>] [--push-remote origin]
git uplink sync
git uplink conflicted <id> --issue-url <url> [--issue <n>] [--push-remote origin]
git uplink merged <id> [--via pr|trailer|patch-id|empty-rebase|manual] [--sha <sha>]
git uplink drop <id> [--reason <text>]
git uplink rebuild
git uplink resolve <id>
git uplink web-ui [--port 43721] [--bind 127.0.0.1] [--no-open]
```

`init` writes `.uplink/queue.json` on the orphan branch `uplink/state`, including remote URLs and branch names. `--upstream` / `--contrib` record those URLs and add the remotes. A later `git uplink init` with no arguments fetches `origin` `uplink/state` and reconstitutes the remotes from the stored URLs. Queue state lives on `uplink/state` as `.uplink/queue.json` and `.uplink/patches/*.patch`. Company `main` is product-only. Import applies the new patch as a fast-forward; sync rebuilds `main` only when upstream moved.

`add --title` is the queue entry name. `--message` / `--message-file` is the single commit message stored on the patch (PR title, blank line, PR body). HTML comments are stripped. Company `main` keeps the cutoff; contrib export removes it. If neither message flag is set, the title is the whole message. `add --push` refreshes `main` from `origin` (or `--refresh`) and force-with-lease pushes the rebuilt branch (`--push-remote` defaults to `origin`). `Uplink-Depends-On: upl_…` lines in that message (after HTML comments are stripped) become `dependsOn`; `--depends-on` is an optional overlay. Incoming `preflight` reads the same trailers from `--message` / `--message-file`. `drop --reason` defaults to `dropped by operator`.

`submit` exports the patch onto the contrib fork (git only) and prints JSON for `gh pr create`. `submitted` records the PR URL, commits the queue, and pushes company `uplink/state`. Merge detection, in order: recorded GitHub PR on the queue → `Uplink-Patch-Id` trailer → `git patch-id --stable` → empty apply. `sync` / `resolve` print issue create/close JSON for `gh`; `conflicted` records the issue on the patch.

git-uplink shells out to `git`, but it does **not** use the operator’s commit signer or default SSH key. Bot identity and `commit.gpgsign=false` are process-scoped (`git -c`), so `git uplink init` does not rewrite `user.name` / `commit.gpgsign` in the clone. Your own `git commit` in that repo still follows global signing. Fetch/push/clone over the network need `UPLINK_GITHUB_TOKEN` or `GITHUB_TOKEN` (SSH remotes are rewritten to HTTPS for that invocation) or a dedicated `UPLINK_SSH_KEY` / `UPLINK_SSH_COMMAND`. Local `file://` remotes need neither. Without those, network git fails instead of opening ssh-agent / Touch ID.

## Dashboard

`git uplink web-ui` is the operator UI: control room, live lab, collaboration notes, system playbook, [way-of-working.md](way-of-working.md), and a **This repo** page that reads `.uplink/queue.json` from `uplink/state` in the directory you started in.

Frontend sources live in `web/` (Vite + React + Tailwind). Do not commit `web/dist`; cargo rebuilds it.

## Tests

```bash
cargo test
npm test --prefix web
cargo deny check
```

The suite drives real git (temp repos): stacked patches, drop-on-merge, conflicts, concurrent adds, export preflight, prepare/scrub, OSS packets, plus a check that the UI was embedded.

`build.rs` runs the Live lab scenario tests in `web/` (`npm test`) before embedding the dashboard. Those cases are the executable spec for drop-on-merge, internal-only staying off the fork, queued work staying off the fork until oss approval, and every lab step completing. You can run them alone with `npm test --prefix web`. Typecheck is `npm run typecheck --prefix web`.

CI is in `.github/workflows/ci.yml`: `cargo test --locked` and `cargo build --release`, a dedicated `web/` job (`npm ci`, typecheck, vitest), and `cargo deny` (RustSec advisories plus licenses, bans, and sources).

## Product-repo workflows

Copy `templates/emu-workflows/` into the company product repository. Those jobs assume `git-uplink` is on `PATH`.

| Workflow | When |
| --- | --- |
| `uplink-prepare.yml` | Every PR to `main` — PR title/body as the commit message, cutoff, export author, affiliation scan |
| `uplink-preflight.yml` | Every PR to `main` — apply onto public `main` + declared deps, then `UPLINK_PREFLIGHT` |
| `uplink-import.yml` | Label `uplink:import` or merge — product gate, status `queued` |
| `uplink-sync.yml` | Hourly / manual — fetch upstream, drop merged patches; rebuild `main` only if upstream moved. Queue commits go to `uplink/state`. Persist conflicts and open an internal issue |
| `uplink-resolve.yml` | Human push to `uplink/conflict/*` — `git uplink resolve`, rebuild `main`; if already submitted, status `amended` and dispatch submit for a delta IP pass; publish a later conflict like sync |
| `uplink-submit.yml` | Dispatch with a patch id — `oss` Environment IP gate (full packet or delta), then approve + submit. Skips opening a second PR when `pr_number` is already stored |

Environment setup is in `templates/README.md`. Developer stories: [way-of-working.md](way-of-working.md).

## Try it on GitHub

[`examples/github/`](examples/github/) is a walkthrough on three repositories (`uplink-example-upstream`, `uplink-example-upstream-contrib`, `uplink-example-internal`). Local bootstrap, Actions reset, and apply-able patches: [examples/github/SETUP.md](examples/github/SETUP.md). Stories: [examples/github/README.md](examples/github/README.md).
