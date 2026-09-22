# git-uplink

Rust Git subcommand for the Uplink operating model. The binary is **`git-uplink`**, so Git treats it as `git uplink`.

It is for a company on a private forge that must build on a public project, keep unreleased work private, pass IP review, and contribute through an **upstream-owned public contribution fork**. GitHub Enterprise Cloud with Enterprise Managed Users is one such forge. Developers keep one internal branch per change. Merge is the product gate. Company `main` is always:

```text
public upstream/main  +  tooling  +  active upstream[]  +  active internal[]
```

`add` is the internal product gate (status `queued`). `approve` / `submit` are the IP gate. After a submitted patch is conflict-resolved it becomes `amended` until IP approves the delta. On GitHub Enterprise Cloud, dispatch the **to-upstream** Environment workflow (resolve of a submitted patch does this for you); `git uplink report` writes `.uplink/reports/<id>/assessment.md` on `uplink/state` and `GITHUB_STEP_SUMMARY`.

```bash
cargo install --path .
git uplink status
git uplink web-ui
```

`git uplink web-ui` serves the local queue UI from files **embedded in the binary**. `build.rs` runs `npm ci` and `npm run build` in `web/`; the dist is compiled in with `rust-embed`. The command opens a browser (pass `--no-open` to skip).

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
git uplink init [--upstream <url>] [--contrib <url>] [--forge ghec|example-github]
            [--json] [--upgrade] [--adopt-groups <file>]
            [--upstream-remote-name <name>] [--upstream-branch <branch>]
            [--contrib-remote-name <name>] [--internal-branch <branch>]
git uplink add --title <text> [--message <text> | --message-file <path>]
            [--from <ref>] [--head <ref>] [--internal-only]
            [--pr <n>] [--pr-url <url>] [--depends-on <id>]...
git uplink push [--push-remote <remote>]
git uplink refresh
git uplink reset
git uplink preflight [<id>] [--from <ref>] [--head <ref>] [--title <text>]
            [--message <text> | --message-file <path>]
            [--depends-on <id>]...
git uplink assess [--from <ref>] [--head <ref>] [--title <text>]
            [--message <text> | --message-file <path>]
            [--internal-only]
git uplink report <id> [--out <file>] [--extra-dir <path>]
git uplink status [--json]
git uplink approve <id> [--out <file>]
git uplink submit <id>
git uplink submitted <id> --pr-url <url> [--pr <n>] [--push-remote origin]
git uplink sync
git uplink accept-upstream
git uplink gated <id> --pr-url <url> [--pr <n>] [--push-remote origin]
git uplink merged <id> [--via pr|trailer|patch-id|empty-rebase|manual] [--sha <sha>]
git uplink drop <id> [--reason <text>]
git uplink rebuild [--branch <name>] [--push] [--push-remote <remote>]
git uplink resolve <id>
git uplink transfer <id> --to-upstream|--to-internal [--complete]
git uplink web-ui [--port 43721] [--bind 127.0.0.1] [--no-open]
```

`init` writes `.uplink/queue.json` on the orphan branch `uplink/state`, including remote URLs, branch names, and `forge`. `--upstream` / `--contrib` record those URLs and add the remotes. `--forge` is required when creating a queue (`ghec` for GitHub Enterprise Cloud, `example-github` for the worked example). First-time init also installs that forge's workflows plus the shared GitHub pull request template as the dedicated **tooling** patch. If company `main` already matches public upstream, it rebuilds `main` with that tooling patch. If `main` is fast-forward ahead, init leaves `main` alone, records the unique first-parent commits as patches after tooling, and does not push. Group rebase-style history in the terminal UI, or pass `--adopt-groups` JSON (`[{ "commits": ["abc123", "def456"], "title": "…", "intent": "upstream" }]`). Adopt-group `intent` is an input selector for `upstream` vs `internal`; it is not stored on the patch. Merge commits are one row each (the merge SHA, not the hidden PR branch). Then preview with `git uplink rebuild --branch uplink/verify` and, after `git diff main uplink/verify`, publish with `git uplink rebuild --push`. `--upgrade` refreshes the tooling patch in its dedicated slot. A later `git uplink init` with no arguments fetches `origin` `uplink/state`, `uplink/upstream`, and the configured company branch, materializes that local ref without checking it out, and reconstitutes the remotes from the stored URLs; it does not rewrite workflows. `--json` prints that stored config. Queue state lives on `uplink/state` as `.uplink/queue.json` and `.uplink/patches/*.patch`. Company `main` is product-only. Merge lands the change on `main`; import records the patch on `uplink/state` (`upstream[]` by default, `internal[]` with `uplink:internal-only`). Upstream import then rebuilds so the new patch sits under internal and publishes rewritten `main`. Internal import is add-only. Sync rebuilds `main` only when upstream moved.

`add --title` is the queue entry name. `--message` / `--message-file` is the single commit message stored on the patch (PR title, blank line, PR body). HTML comments are stripped. Company `main` keeps the cutoff; contrib export removes it. If neither message flag is set, the title is the whole message. `add` records the patch on local `uplink/state` only. `git uplink push` publishes that branch (`--push-remote` defaults to `origin`). If origin moved, it appends local-only patches onto the remote tip and carries those patch files. `git uplink refresh` fetches origin tracking refs (`uplink/state`, `uplink/upstream`, company main) without moving local branches. `git uplink reset` fetches origin and hard-resets company `main`, `uplink/state`, and `uplink/upstream` so the clone matches origin and `.uplink/` is restored. `Uplink-Depends-On: upl_…` lines in that message (after HTML comments are stripped) become `dependsOn`; `--depends-on` is an optional overlay. Incoming `preflight` reads the same trailers from `--message` / `--message-file`. `drop --reason` defaults to `dropped by operator`.

`submit` exports the patch onto the contrib fork (git only) and prints JSON for `POST /repos/{parent}/pulls` (`head` is the branch from `.branch`; `head_repo` is the contrib repository name). `submitted` records the PR URL, commits the queue, and pushes company `uplink/state`. Merge detection, in order: recorded GitHub PR on the queue → `Uplink-Patch-Id` trailer → `git patch-id --stable` → empty apply. `sync` classifies new public commits: matching company patches apply immediately; unmatched commits write a from-upstream packet and wait. `accept-upstream` promotes the pending SHA after that review. `sync` / `resolve` print `gh.prCreate` JSON for gated conflict PRs and exit 0; `gated` records that company PR on the patch. `git uplink transfer` moves a patch between queues, or prints gated branches for a transfer PR when apply, assess (`--to-upstream`), or preflight fails (also exit 0). `--to-internal` refuses while an active upstream patch still depends on this id. Successful `--to-internal` of a submitted patch prints `gh.prClose` (`url`, `contribBranch`) so Actions can dispatch **Uplink abandon contrib**. Callers use the JSON, not the process status, to open company PRs.

git-uplink shells out to `git`, but it does **not** use the operator’s commit signer, default SSH key, or `GITHUB_TOKEN`. Bot identity and `commit.gpgsign=false` are process-scoped (`git -c`), so `git uplink init` does not rewrite `user.name` / `commit.gpgsign` in the clone. Your own `git commit` in that repo still follows global signing. Network git picks credentials by remote: `origin` uses `UPLINK_INTERNAL_KEY` or `UPLINK_INTERNAL_TOKEN`, `contrib` uses `UPLINK_CONTRIB_KEY` or `UPLINK_CONTRIB_TOKEN`, and `upstream` uses `UPLINK_UPSTREAM_KEY` or `UPLINK_UPSTREAM_TOKEN`. If both KEY and TOKEN are set, KEY wins. A KEY is a path to a **passwordless** private key (`BatchMode=yes`); a passphrase-protected key fails closed. TOKEN rewrites SSH remotes to HTTPS for that invocation. Local `file://` remotes need neither. Without the matching role’s creds, network git fails instead of opening ssh-agent / Touch ID.

## Website and operator UI

The public site (playbook, collaboration notes, [way-of-working.md](way-of-working.md), install, forge setup, GitHub example, and the live lab) is GitHub Pages: [https://npetzall.github.io/git-uplink/](https://npetzall.github.io/git-uplink/). Sources live in `site/` (Vite + React + Tailwind). Do not commit `site/dist`. Enable **Settings → Pages → Source: GitHub Actions**. `.github/workflows/pages.yml` builds `site/`, uploads the Pages artifact, and deploys on push to `main`.

`git uplink web-ui` is the local operator UI for **this checkout**: it reads `.uplink/queue.json` from `uplink/state` in the directory you started in, and can also inspect `origin/uplink/state` after a fetch. Frontend sources live in `web/` (Vite + React + Tailwind). Do not commit `web/dist`; cargo rebuilds it.

## Tests

```bash
cargo test
npm test --prefix site
cargo deny check
```

The suite drives real git (temp repos): stacked patches, drop-on-merge, conflicts, concurrent adds, export preflight, assess/scrub, contribution packets, plus a check that the UI was embedded.

Live lab scenario tests live in `site/` (`npm test --prefix site`): drop-on-merge, internal-only staying off the fork, queued work staying off the fork until to-upstream approval, and every lab step completing. Typecheck is `npm run typecheck --prefix site` and `npm run typecheck --prefix web`.

CI is in `.github/workflows/ci.yml`: `cargo test --locked` and `cargo build --release`, a `web/` job (`npm ci`, typecheck), a `site/` job (`npm ci`, typecheck, vitest, build), `cargo deny` (RustSec advisories plus licenses, bans, and sources), and a `pre-commit` job (`uvx pre-commit`, rustfmt, clippy). [zizmor](https://zizmor.sh/) audits GitHub Actions YAML in this repo and the forge templates; `.github/workflows/zizmor.yml` publishes results via code scanning.

Distribution SBOMs (CycloneDX JSON and SPDX JSON) cover the Rust crate and the embedded `web/` UI. `site/` is excluded because it is GitHub Pages only, not part of the shipped binary. Generate both files locally with [Syft](https://github.com/anchore/syft): `syft dir:.` reads `.syft.yaml`. `.github/workflows/sbom.yml` uploads them as workflow artifacts on push/PR and attaches `git-uplink-<tag>.cdx.json` / `git-uplink-<tag>.spdx.json` when a GitHub Release is published.

## Product-repo workflows

`git uplink init --upstream <url> --contrib <url> --forge ghec` writes the GHEC pack from `templates/ghec/` plus `templates/github/pull_request_template.md`. Those jobs assume `git-uplink` is on `PATH`. Re-run `git uplink init --upgrade` after upgrading the binary to refresh the same internal-only tooling patch. The example walkthrough uses `--forge example-github`.

| Workflow | When |
| --- | --- |
| `uplink-pr.yml` | Every PR to `main` except `uplink:internal-only` — parallel **Uplink upstream assess** (message, cutoff, author, affiliation) and **Uplink upstream preflight** (apply onto public `main` + declared deps, then `UPLINK_PREFLIGHT`) |
| `uplink-import.yml` | Merge to `main` — product gate, records the patch as status `queued` |
| `uplink-sync.yml` | Hourly / manual — fetch upstream; apply flowed-back patches immediately; foreign commits wait on `from-upstream` then `accept-upstream`. Persist conflicts and open a gated PR from `-work` into `uplink/conflict/<id>` (job stays green; freeze is the open `uplink:conflict` PR / `git uplink status`) |
| `uplink-resolve.yml` | Merge of the conflict PR — `git uplink resolve`, rebuild `main` (job stays green even if a later patch conflicts and a new gated PR is opened); if already submitted, status `amended`, cancel any waiting/in-progress submit for that id, and dispatch submit for a delta IP pass |
| `uplink-transfer.yml` | Dispatch to move a patch between queues; `--to-upstream` re-assesses; `--to-internal` refuses while upstream dependents remain; merge of a transfer PR runs `--complete`; close without merge deletes both branches; submitted `--to-internal` dispatches abandon-contrib |
| `uplink-abandon.yml` | Detached close of the public PR and delete of `uplink/<id>` on the contrib fork; waits on Environment `abandon-contrib` (same contrib secrets as `to-upstream`, different reviewers). Public leftover remains until that environment is approved |
| `uplink-gate.yml` | PRs into protected uplink bases (`pull_request_target` from default branch) — no conflict markers; refuse pack-file diffs; transfer-to-upstream also runs export preflight |
| `uplink-submit.yml` | Dispatch with a patch id — `to-upstream` Environment IP gate (full packet or delta), then approve + submit. Skips opening a second PR when `pr_number` is already stored |

Environment setup is in `templates/README.md`. Developer stories: [way-of-working.md](way-of-working.md).

## Try it on GitHub

[`examples/github/`](examples/github/) is a walkthrough on three repositories (`uplink-example-upstream`, `uplink-example-upstream-contrib`, `uplink-example-internal`). Local bootstrap, Actions reset, and apply-able patches: [examples/github/SETUP.md](examples/github/SETUP.md). Stories: [examples/github/README.md](examples/github/README.md).
