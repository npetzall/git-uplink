# CLI reference

The binary is **`git-uplink`**, so Git treats it as `git uplink`. Use `git-uplink -h` or `git uplink -h`. Plain `git uplink --help` goes through Git’s man-page path, not clap.

## Synopsis

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
git uplink web-ui [--port 43721] [--no-open]
git uplink version
```

## Where state lives

- Queue state lives on the orphan branch **`uplink/state`** as `.uplink/queue.json` and `.uplink/patches/*.patch`.
- Company `main` is product-only: public upstream, then the tooling patch, then every active `upstream[]` and `internal[]` patch.
- Reports live under `.uplink/reports/<id>/` on `uplink/state`, so a product rebuild never drops them.

## Setup

### `init`

`init` writes `.uplink/queue.json` on `uplink/state`, including remote URLs, branch names, and `forge`.

- `--upstream` / `--contrib` record those URLs and add the remotes.
- `--forge` is required when creating a queue: `ghec` for GitHub Enterprise Cloud, `example-github` for the worked example.
- First-time init also installs that forge's workflows plus the shared GitHub pull request template as the dedicated **tooling** patch.
- `--upgrade` refreshes the tooling patch in its dedicated slot. Re-run it after upgrading the binary.
- `--json` prints the stored config.

A later `git uplink init` with no arguments fetches `origin` `uplink/state`, `uplink/upstream`, and the configured company branch, materializes that local ref without checking it out, and reconstitutes the remotes from the stored URLs. It does not rewrite workflows.

### Adopting an existing `main`

- If company `main` already matches public upstream, init rebuilds `main` with the tooling patch.
- If `main` is fast-forward ahead, init leaves `main` alone and records the unique first-parent commits as patches after tooling.

Group rebase-style history in the terminal UI, or pass `--adopt-groups` JSON:

```json
[{ "commits": ["abc123", "def456"], "title": "…", "intent": "upstream" }]
```

Adopt-group `intent` is `upstream` (the default) or `internal-only`; it is not stored on the patch. Merge commits are one row each (the merge SHA, not the hidden PR branch).

Then preview and publish:

```bash
git uplink rebuild --branch uplink/preview/verify
git diff main uplink/preview/verify
git uplink rebuild --push
```

## Recording changes

### `add`

`add` is the internal product gate. It records the patch with status `queued` on local `uplink/state` only.

- `--title` is the queue entry name.
- `--message` / `--message-file` is the single commit message stored on the patch (PR title, blank line, PR body). HTML comments are stripped. Company `main` keeps the cutoff; contrib export removes it. If neither message flag is set, the title is the whole message.
- `Uplink-Depends-On: upl_…` lines in that message (after HTML comments are stripped) become `dependsOn`. `--depends-on` is an optional overlay.

Merge lands the change on `main`; import records the patch on `uplink/state` (`upstream[]` by default, `internal[]` with `uplink:internal-only`).

- Upstream import rebuilds when an active internal patch must stay on top, and publishes that replay with a plain force push of `main`.
- An upstream import with an empty internal queue, and every internal-only import, only records the patch.

### `push`, `refresh`, `reset`

- **`push`** publishes `uplink/state` (`--push-remote` defaults to `origin`). If origin moved, it appends local-only patches onto the remote tip and carries those patch files.
- **`refresh`** fetches origin tracking refs (`uplink/state`, `uplink/upstream`, company main) without moving local branches.
- **`reset`** fetches origin and hard-resets company `main`, `uplink/state`, and `uplink/upstream` so the clone matches origin and `.uplink/` is restored.

## Checks

- **`preflight`** applies a change onto public `main` plus its declared dependencies. Incoming preflight reads `Uplink-Depends-On` trailers from `--message` / `--message-file`.
- **`assess`** checks the message, cutoff, author, and affiliation of a change.
- **`report <id>`** writes `.uplink/reports/<id>/assessment.md` on `uplink/state` and prints the packet. The submit workflow appends that stdout to `GITHUB_STEP_SUMMARY`. `--extra-dir` prepends company assessment-hook extras.
- **`status`** shows the queue (`--json` for machines).

## Contributing upstream

`approve` / `submit` are the IP gate. On GitHub Enterprise Cloud, dispatch the **to-upstream** Environment workflow instead of calling them by hand; see [Forge packs](https://npetzall.github.io/git-uplink/setup).

- **`submit`** exports the patch onto the contrib fork (git only) and prints JSON for `POST /repos/{parent}/pulls`. `head` is the branch from `.branch`; `head_repo` is `<contrib_owner>/<contrib_repo>`.
- **`submitted`** records the PR URL, commits the queue, and pushes company `uplink/state`.

After a submitted patch is conflict-resolved it becomes **`amended`** until IP approves the delta. Resolve of a submitted patch dispatches a new submit for you.

### Merge detection

`merged` can be recorded explicitly (`--via`), otherwise detection runs in this order:

1. Recorded GitHub PR on the queue
2. `Uplink-Patch-Id` trailer
3. `git patch-id --stable`
4. Empty apply

## Upstream moves and conflicts

- **`sync`** classifies new public commits. Matching company patches apply immediately; unmatched commits write a from-upstream packet and wait. Sync rebuilds `main` only when upstream moved.
- **`accept-upstream`** promotes the pending SHA after that review.
- **`sync`** and **`resolve`** print `gh.prCreate` JSON for gated conflict PRs and exit 0.
- **`gated`** records that company PR on the patch.

## Moving and removing patches

- **`transfer`** moves a patch between queues, or prints gated branches for a transfer PR when apply, assess (`--to-upstream`), or preflight fails (also exit 0).
  - `--to-internal` refuses while an active upstream patch still depends on this id.
  - A successful `--to-internal` of a submitted patch prints `gh.prClose` (`url`, `contribBranch`) so Actions can dispatch **Uplink abandon contrib**.
- **`drop`** removes a patch. `--reason` defaults to `dropped by operator`.
- **`rebuild`** replays `main` from the queue, optionally onto `--branch` for preview, and `--push` publishes it.

> Callers use the JSON, not the process status, to open company PRs.

## `web-ui`

`git uplink web-ui` serves the local operator UI for **this checkout**. It reads `.uplink/queue.json` from `uplink/state` in the directory you started in, and can also inspect `origin/uplink/state` after a fetch. The UI is embedded in the binary. It opens a browser; pass `--no-open` to skip.

## `version`

`git uplink version` (or `git-uplink --version`) prints the package version and the commit it was built from, e.g. `git-uplink 0.1.0 (0c393c4)`.

- A `-dirty` suffix means tracked files other than `Cargo.toml` / `Cargo.lock` had local changes at build time.
- `unknown` means the build had no Git checkout.
- Packagers can set `GIT_UPLINK_COMMIT` to override it.

## Credentials and identity

git-uplink shells out to `git`, but it does **not** use the operator’s commit signer, default SSH key, or `GITHUB_TOKEN`.

- Bot identity and `commit.gpgsign=false` are process-scoped (`git -c`), so `git uplink init` does not rewrite `user.name` / `commit.gpgsign` in the clone. Your own `git commit` in that repo still follows global signing.
- Network git picks credentials by remote:

| Remote | Key | Token | Notes |
| --- | --- | --- | --- |
| `origin` | `UPLINK_INTERNAL_KEY` | `UPLINK_INTERNAL_TOKEN` | Required for SSH. |
| `contrib` | `UPLINK_CONTRIB_KEY` | `UPLINK_CONTRIB_TOKEN` | Required for SSH. |
| `upstream` | `UPLINK_UPSTREAM_KEY` | `UPLINK_UPSTREAM_TOKEN` | An `https://` upstream may omit both and is fetched anonymously. |

- If both KEY and TOKEN are set, KEY wins.
- A KEY is a path to a **passwordless** private key (`BatchMode=yes`); a passphrase-protected key fails closed.
- A TOKEN rewrites SSH remotes to HTTPS for that invocation and is sent when present, including on an already-HTTPS upstream.
- Local `file://` remotes need neither.
- SSH upstream, origin, and contrib without the matching role’s creds fail instead of opening ssh-agent / Touch ID.
