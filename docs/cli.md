<!-- Generated from src/cli.rs. Do not edit; run `UPLINK_BLESS=1 sfw cargo test --locked --test cli_docs`. -->

# CLI reference

Developers open PRs and merge them; they never push main. `add` records a merged PR as a queued patch on `uplink/state`. `assess` uses the PR title and body as the single commit message, adds a co-author trailer, strips the internal section before contrib export, and scans for company affiliation.

On GitHub Enterprise Cloud, contribution approval is the to-upstream Environment; `approve` and `submit` run after that review. git uplink talks to git only; workflows use gh for GitHub and follow-up commands (`submitted`, `gated`) to record results.

The binary is `git-uplink`, so Git treats it as `git uplink`. Use `git-uplink -h` or `git uplink -h` for a summary, and `git uplink <command> --help` for one command. Plain `git uplink --help` goes through Git's man-page path; install the pages with `git uplink man`.

## Synopsis

```text
git uplink init [--upstream <url>] [--contrib <url>] [--upstream-remote-name <name>]
            [--upstream-branch <branch>] [--contrib-remote-name <name>]
            [--internal-branch <branch>] [--forge <forge>] [--upgrade]
            [--adopt-groups <path>] [--json] [--preflight <cmd>]
            [--redact-keyword <word>]... [--internal-domain <domain>]...
git uplink add --title <text> [--message <text> | --message-file <path>] [--from <ref>]
            [--head <ref>] [--internal-only] [--pr <n>] [--pr-url <url>]
            [--base-branch <branch>] [--depends-on <id>]... [--preflight-result <path>]
            [--json]
git uplink push [--push-remote <remote>]
git uplink refresh
git uplink reset
git uplink rebase [--plan] [--head <rev>] [--json] [--no-fetch]
git uplink preflight [<id>] [--from <ref>] [--head <ref>] [--title <text>]
            [--message <text> | --message-file <path>] [--depends-on <id>]...
            [--internal-only] [--command-only] [--hooks <rev>] [--json]
git uplink assess [--from <ref>] [--head <ref>] [--title <text>]
            [--message <text> | --message-file <path>] [--internal-only | --patch <id>]
            [--package <dir>] [--json]
git uplink report <id> [--out <path>] [--extra-dir <path>] [--store-extras]
            [--extra-source <url>] [--assess-result <path>]
git uplink status [--json]
git uplink doctor [--json]
git uplink approve <id> [--out <path>] [--reviewed <token>]
git uplink submit <id> [--push] [--preflight-result <path>]
git uplink submitted <id> --pr-url <url> [--pr <n>] [--push-remote <remote>]
git uplink sync [--merged-pr <id=sha>]...
git uplink accept-upstream [--sha <sha>] [--fetch-only] [--preflight-result <path>]
            [--preflight-only]
git uplink gated <id> --pr-url <url> [--pr <n>] [--push-remote <remote>]
git uplink merged <id> [--via <how>] [--sha <sha>]
git uplink drop <id> [--reason <text>]
git uplink rebuild [--branch <name>] [--push] [--push-remote <remote>]
            [--preflight-result <path>] [--preflight-only]
git uplink resolve <id> [--preflight-result <path>] [--preflight-only]
git uplink transfer <id> (--to-upstream | --to-internal) [--complete]
            [--preflight-result <path>] [--preflight-only]
git uplink amend <id> [--complete] [--title <text>]
            [--message <text> | --message-file <path>] [--preflight-result <path>]
            [--preflight-only]
git uplink web-ui [--port <port>] [--no-open]
git uplink man <dir>
git uplink version
```

## Setup

### `init`

Create the queue on uplink/state, or restore this clone from origin.

Writes `.uplink/queue.json` on `uplink/state`, including remote URLs, branch names, and the forge. First-time init also installs that forge's workflows plus the shared GitHub pull request template as the dedicated tooling patch.

Init also creates the local orphan branch `uplink/hooks` for company hooks (assessment hook guide and starter, toolchain hook stub, `preflight.sh`) when neither this clone nor `origin` has it. Init never pushes it and never changes an existing file on it. While creating it, init asks for the settings that `--preflight`, `--redact-keyword`, and `--internal-domain` answer.

A later `git uplink init` with no arguments fetches `origin` `uplink/state`, `uplink/upstream`, and the configured company branch, materializes that local ref (and `uplink/hooks` when origin has it) without checking it out, and reconstitutes the remotes from the stored URLs. It does not rewrite workflows.

- `--upstream <url>`: Public upstream URL; recorded and added as a remote.
- `--contrib <url>`: Contrib fork URL; recorded and added as a remote.
- `--upstream-remote-name <name>`: Name of the upstream remote.
- `--upstream-branch <branch>`: Branch of public upstream to follow.
- `--contrib-remote-name <name>`: Name of the contrib remote.
- `--internal-branch <branch>`: Company branch that carries the patches.
- `--forge <forge>`: Forge pack to install; required when creating a queue.

  The former names `ghec` and `example-github` are still accepted, and are read from queues that stored them.
  - `github`: The base GitHub pack, for github.com and GitHub Enterprise Cloud.
  - `try-it-on-github`: The base pack adjusted for the worked example.
- `--upgrade`: Refresh the tooling patch in its slot; re-run after upgrading the binary.

  Also creates `uplink/hooks` for queues initialized before it existed, and adds files a newer pack brings as one commit on top.
- `--adopt-groups <path>`: JSON file of commit groups when internal is ahead of upstream.
- `--json`: Print the stored queue config as JSON.
- `--preflight <cmd>`: Command a new preflight.sh on uplink/hooks starts with (asked in a terminal when omitted).
- `--redact-keyword <word>`: uplink.toml: word that must not appear in a contribution (repeatable, or comma-separated).
- `--internal-domain <domain>`: uplink.toml: internal email domain to flag in the export (repeatable, or comma-separated).

## Recording changes

### `add`

Record a merged change as a queued patch on local uplink/state.

`add` is the internal product gate. It records the patch with status `queued` on local `uplink/state` only.

Merge lands the change on `main`; import records the patch on `uplink/state` (`upstream[]` by default, `internal[]` with `uplink:internal-only`). Upstream import rebuilds when an active internal patch must stay on top, and publishes that replay with a plain force push of `main`. An upstream import with an empty internal queue, and every internal-only import, only records the patch.

- `--title <text>`: Queue entry name.
- `--message <text>`: The single commit message stored on the patch (PR title, blank line, PR body).

  HTML comments are stripped. Company `main` keeps the cutoff; contrib export removes it. If neither message flag is set, the title is the whole message. `Uplink-Depends-On: upl_...` lines in the message become `dependsOn` and are left out of the public message.
- `--message-file <path>`: Read the commit message from a file (- reads stdin).
- `--from <ref>`: Base revision, the company branch by default (fetched from origin if missing).
- `--head <ref>`: Head revision, HEAD by default (fetched from origin if missing).
- `--internal-only`: Record on the internal queue: never exported, leak scan skipped.
- `--pr <n>`: Number of the company PR that merged the change.
- `--pr-url <url>`: URL of the company PR that merged the change.
- `--base-branch <branch>`: Branch the company PR merged into.

  Refused unless it is the company branch: a change merged into any other branch has not passed the company branch's review. Import passes the PR's base branch.
- `--depends-on <id>`: Patch this one depends on, on top of the message trailers (repeatable).
- `--preflight-result <path>`: Take the verdict of preflight.sh from this file instead of running it.

  The file is the output of `git uplink preflight --json` from a job without credentials. It is used only when it is for the tree this command builds; otherwise the command fails and preflight has to run again.
- `--json`: Print the patch as JSON, with `readyToSubmit`.

  `readyToSubmit` lists the patches this import made ready to submit: upstream-bound, queued, and every upstream dependency merged. Import dispatches Uplink submit for them when the repository variable `UPLINK_AUTO_SUBMIT` is `true`.

### `push`

Publish local uplink/state, restacking unique patches if origin moved.

If origin moved, it appends local-only patches onto the remote tip and carries those patch files. Then it publishes a local `uplink/hooks` that is new or ahead of origin, never with force. A refused hooks push (for example by the hooks ruleset, or a token without workflows scope) is a warning; open a pull request against `uplink/hooks` instead.

- `--push-remote <remote>`: Remote to push to, origin by default.

### `refresh`

Fetch origin tracking refs without moving local branches.

The refs are `uplink/state`, `uplink/upstream`, and company main.

### `reset`

Fetch origin and hard-reset company main, uplink/state, and uplink/upstream.

Afterwards the clone matches origin and `.uplink/` is restored.

### `rebase`

Rebase the current branch onto company main after a rebuild replaced it.

A rebuild writes every commit of `main` again, so a branch cut from the old `main` shares only public upstream with the new one and its pull request lists the old patches as its own. `rebase` finds the commit of the old `main` the branch started from and runs `git rebase --onto origin/main <that commit>`. A branch that is only behind gets a plain `git rebase origin/main`. A conflict stops as `git rebase` does; continue with `git rebase --continue`.

It runs in any clone of the company repository and needs no `git uplink init`. It fetches `main`, `uplink/state` and `uplink/upstream` from origin, which needs `UPLINK_INTERNAL_TOKEN` or `UPLINK_INTERNAL_KEY` as every git-uplink fetch does; after a `git fetch origin` of your own, `--no-fetch` needs neither. The rebase itself is plain `git rebase` with your identity, signing and hooks.

The commits a rebuild writes carry an `Uplink-Patch-Id` trailer, and each rebuild lists the other commits of the `main` it replaced in `.uplink/previous-main.json` on `uplink/state`. When the newest such commit in the branch has anything else under it, `rebase` refuses rather than drop it.

The forge pack comments the command on a pull request whose `main` was replaced. With the repository variable `UPLINK_AUTO_REBASE` set to `true`, or the label `uplink:rebase` on the pull request, Uplink rebase does it and pushes the branch.

- `--plan`: Print what the rebase would do and change nothing.
- `--head <rev>`: Plan for this commit instead of the checked-out branch.
- `--json`: Print the plan as JSON: `state` (`current`, `behind`, `replaced`, `unknown`), `forkPoint`, `command`.
- `--no-fetch`: Use origin's branches as last fetched instead of fetching them.

## Checks

### `preflight`

Apply a change onto public main plus its dependencies and run preflight.sh.

With `<id>` it checks a queued patch; without, the incoming change between `--from` and `--head`. Incoming preflight reads `Uplink-Depends-On` trailers from `--message` / `--message-file`.

`--command-only` applies nothing and just runs `preflight.sh` in the current tree; the gate uses it for internal-only patches.

The output of `preflight.sh` is shown as it runs. The verdict lists what the script ran on: `uplink/upstream`, each dependency applied onto it, then the change; for `--command-only`, the commits the current tree has on top of `uplink/upstream`.

- `<id>`: Queued patch to check instead of an incoming change.
- `--from <ref>`: Base revision, main by default (fetched from origin if missing).
- `--head <ref>`: Head revision, HEAD by default (fetched from origin if missing).
- `--title <text>`: Title of the incoming change.
- `--message <text>`: Commit message of the incoming change (PR title, blank line, PR body).
- `--message-file <path>`: Read the commit message from a file (- reads stdin).
- `--depends-on <id>`: Patch the change depends on, on top of the message trailers (repeatable).
- `--internal-only`: The incoming change is internal-only; export preflight is skipped.
- `--command-only`: Only run preflight.sh in the current tree.
- `--hooks <rev>`: Read preflight.sh from this revision instead of uplink/hooks, to try a change to it.
- `--json`: Print the result as JSON, for `--preflight-result` of the command that records it.

  `comment` in it is the verdict as markdown, for a pass and for a failure, and `tested` what the script ran on. The output of `preflight.sh` goes to stderr.

### `assess`

Check the message, cutoff, author, and affiliation of a change.

The PR title and body are the single commit message. Assess adds a co-author trailer, strips the internal section before contrib export, and scans for company affiliation using `uplink.toml`.

`--package <dir>` writes the assessment package: `assessment.json` (the result, the public message, the `uplink.toml` settings it scanned with, and for a patch its queue entry and the `uplink/state` commit it was read at), `assessment.md` (the report assess prints), and `change.patch` (what was assessed: the diff of the change, or the patch file). The pull request checks and Uplink submit upload it for the company assessment hook, and `report --assess-result` stores it with the patch. The package is written before assess exits non-zero on findings.

- `--from <ref>`: Base revision, main by default (fetched from origin if missing).
- `--head <ref>`: Head revision, HEAD by default (fetched from origin if missing).
- `--title <text>`: Title of the change.
- `--message <text>`: Commit message of the change (PR title, blank line, PR body).
- `--message-file <path>`: Read the commit message from a file (- reads stdin).
- `--internal-only`: Assess as an internal-only change: never exported, leak scan skipped.
- `--patch <id>`: Assess a queued patch in its layer, with its stored title and message.

  Without `--from` and `--head` the patch file in the queue is assessed as it is; nothing is applied. That assessment also has the `dependencies` check, which fails while an upstream-bound `Uplink-Depends-On` patch is not merged upstream, and its package says in `storedExtras` whether the assessment-hook result stored with the patch is still for its content. With them, the change between the two revisions is assessed as that patch. `--title` or `--message[-file]` override the stored ones, for example for a conflict resolution or amend. The gate uses it on conflict-resolution PRs.
- `--package <dir>`: Write the assessment package (assessment.json, assessment.md, change.patch) to this directory.
- `--json`: Print assessment.json instead of the report.

### `report`

Write the contribution packet for a patch and print it.

Writes `.uplink/reports/<id>/assessment.md` on `uplink/state` and prints the packet. The submit workflow appends that stdout to `GITHUB_STEP_SUMMARY`.

The packet of an upstream-bound patch shows an assessment of the patch file as it is in the queue, made with today's `uplink.toml`. The pull request check and import assess the change before it is a patch. Uplink submit runs `assess --patch <id> --package` and passes the result with `--assess-result`; without it, report assesses the patch file itself. Either way the result is stored with the patch. Nothing is applied: whether the patch still applies on upstream is what `preflight` checks. When the assessment has findings, the packet is still written and printed, and report exits non-zero.

Stored extras lead the packet while the patch content is unchanged (same stable patch id, not `amended`). Submit runs the hook from branch `uplink/hooks` on the assessment package when nothing current is stored (see `assessment-hook.md` on that branch); a failed hook adds a warning note instead of failing submit, and is not stored.

- `<id>`: Patch to report on.
- `--out <path>`: Write the packet here instead of .uplink/reports/\<id>/assessment.md.
- `--extra-dir <path>`: Directory of \*.md files prepended to the packet. Without it, extras stored for the unchanged patch are used.
- `--store-extras`: Also store --extra-dir as the patch's extras for later packets.
- `--extra-source <url>`: Where the stored extras came from, such as the hook run URL.
- `--assess-result <path>`: Store this assessment instead of assessing here: `assessment.json` of `assess --patch <id> --package`.

  Refused when it is not for the patch as it is now: the patch file, title, message or `uplink.toml` changed since, or the result does not match what assessing the patch gives. The company assessment hook read the same package, so this also covers --extra-dir.

### `status`

Show the queue.

- `--json`: Print the queue status as JSON, for machines.

### `doctor`

Check the setup of this clone.

Checks the queue and recorded URLs, remotes and whether upstream and contrib are reachable, `origin/uplink/state`, the forge tooling patch and workflows on the company branch, `uplink/hooks` (present locally, has the toolchain hook and a valid `uplink.toml`, pushed to origin), `UPLINK_*` credentials, pending adoption, and the company branch against `uplink/upstream`.

Credential checks fail on a machine without the `UPLINK_*` variables; that is expected outside CI.

- `--json`: Print the doctor report as JSON.

## Contributing upstream

### `approve`

Record the to-upstream approval of a patch and write its receipt.

`approve` and `submit` are the IP gate. On GitHub Enterprise Cloud, dispatch the to-upstream Environment workflow instead of calling them by hand; see Forge packs at https://npetzall.github.io/git-uplink/setup

- `<id>`: Patch to approve.
- `--out <path>`: Write the receipt here instead of .uplink/reports/\<id>/approval.md.
- `--reviewed <token>`: Review token of the packet that was reviewed.

  `report` writes it to `.uplink/reports/<id>/review-token`. It names what the patch changes, the public title and the public message. What the patch changes is the lines it adds and removes per file, plus the files it creates, deletes, renames or replaces; the unchanged lines around them are not part of it. When the patch no longer has this token, nothing is approved. Without it, the patch is approved as it is now. A patch its last approval already covers gets no second one.

### `submit`

Build the export commit on uplink/upstream and print the PR request as JSON.

Exports the patch for the contrib fork (git only) and prints JSON for `POST /repos/{parent}/pulls`. `head` is the branch from `.branch`; `head_repo` is `<contrib_owner>/<contrib_repo>`. The forge creates the signed contrib commit from the printed `contribCommit`.

- `<id>`: Approved patch to submit.
- `--push`: Force-push the local, unsigned export commit to contrib instead.
- `--preflight-result <path>`: Take the verdict of preflight.sh from this file instead of running it.

  The file is the output of `git uplink preflight --json` from a job without credentials. It is used only when it is for the tree this command builds; otherwise the command fails and preflight has to run again.

### `submitted`

Record the upstream PR of a submitted patch and push uplink/state.

Records the PR URL, commits the queue, and pushes company `uplink/state`. Refused unless the patch is approved or already submitted: a public PR exists only for content that was approved.

- `<id>`: Patch that was submitted.
- `--pr-url <url>`: URL of the upstream pull request.
- `--pr <n>`: Pull request number, read from the URL when omitted.
- `--push-remote <remote>`: Remote to push uplink/state to. Default: `origin`.

## Ingesting upstream

### `sync`

Account for new public commits and rebuild main when upstream moved.

A patch is merged when a commit has its `git patch-id --stable`, or when `--merged-pr` reports its public pull request merged. An `Uplink-Patch-Id` trailer alone is not enough. The merged patches are applied on `uplink/upstream` and compared with the new public main. When nothing else changed, `uplink/upstream` moves and `main` is rebuilt. Otherwise the remaining diff is written to a from-upstream packet and waits for `accept-upstream`; patches are marked merged only then.

On a conflict it prints `gh.prCreate` JSON for the gated conflict PR and exits 0.

`readyToSubmit` in the output lists the queued patches whose last unmerged upstream dependency this sync marked merged, also when the sync ends in a conflict on another patch.

- `--merged-pr <id=sha>`: A recorded public pull request the forge reports as merged, with its merge commit. Repeat it per patch; the sync workflow passes these. One that names a commit outside the new range is ignored.

### `accept-upstream`

Promote a pending public main after from-upstream environment approval.

Moves `uplink/upstream`, marks the patches the packet listed as merged, and rebuilds `main`.

The pending upstream brings changes that are not ours, so it is promoted only when `preflight.sh` passes on it: that makes `uplink/upstream` the known good commit a failing rebuild is bisected from. When it fails, nothing is moved and the command fails. See `rebuild` for the check on the rebuilt tree.

- `--sha <sha>`: The pending public main that was reviewed. Refuses when the queue now holds a different one.
- `--fetch-only`: Fetch public upstream so the pending upstream is in this clone, and change nothing.

  For the step of a job without write credentials that holds the upstream token, before the step that runs `--preflight-only` without it.
- `--preflight-result <path>`: Take the verdict of preflight.sh from this file instead of running it.

  The file is the output of this command with `--preflight-only`, from a job without credentials. It is used only when it is for the trees this command builds; otherwise the command fails and preflight has to run again.
- `--preflight-only`: Run preflight.sh on what this command would test, print the result as JSON, change nothing.

  For a job without credentials; pass the output to the same command with `--preflight-result`. Exits non-zero when the pending upstream fails.

### `merged`

Record that upstream merged a patch.

`merged` records it explicitly with `--via`. Otherwise `sync` detects a merge from a commit with the patch's `git patch-id --stable`, from the recorded public PR reported with `sync --merged-pr`, or from an empty apply. An `Uplink-Patch-Id` trailer alone does not count: use `merged` when upstream took the patch in another form.

- `<id>`: Patch that was merged.
- `--via <how>`: How the merge was detected. Default: `manual`.
  - `pr`: The GitHub PR recorded on the queue was merged.
  - `trailer`: An upstream commit carries the `Uplink-Patch-Id` trailer.
  - `patch-id`: `git patch-id --stable` matches an upstream commit.
  - `empty-rebase`: The patch applies empty on upstream.
  - `manual`: Recorded by an operator.
- `--sha <sha>`: Upstream commit that carries the patch.

## Edit

### `transfer`

Move a patch between the internal and upstream queues.

When apply, assess (`--to-upstream`), or preflight fails it prints gated branches for a transfer PR instead, and exits 0.

`--to-internal` refuses while an active upstream patch still depends on this id. A successful `--to-internal` of a submitted patch prints `gh.prClose` (`url`, `contribBranch`) so Actions can dispatch Uplink abandon contrib. `readyToSubmit` lists the patch after a `--to-upstream` when no upstream dependency of it is unmerged.

- `<id>`: Patch to move.
- `--to-upstream`: Move the patch to the upstream queue.
- `--to-internal`: Move the patch to the internal queue.
- `--complete`: Finish a gated transfer after the work PR is merged.
- `--preflight-result <path>`: Take the verdict of preflight.sh from this file instead of running it.

  The file is the output of `git uplink preflight --json`, or of this command with `--preflight-only`, from a job without credentials. It is used only when it is for the tree this command builds; otherwise the command fails and preflight has to run again.
- `--preflight-only`: Run preflight.sh on what this command would test, print the result as JSON, change nothing.

  For a job without credentials; pass the output to the same command with `--preflight-result`. Exits non-zero when the result is not a pass. With `--complete` the checkout is left on the squashed work, so use a clone made for it.

### `amend`

Revise a patch through a gated PR from uplink/amend/\<id>-work.

Amends an internal-only patch, an upstream patch not yet submitted, or a submitted one a maintainer asked to change. Conflicted, merged, dropped, and tooling patches cannot be amended.

Without `--complete` it replays the queue on `uplink/upstream` up to and including the patch, cuts protected `uplink/amend/<id>` there, and cuts `uplink/amend/<id>-work` one empty commit ahead so a draft PR can open at once. The queue is not touched. It prints `base`, `work`, and `gh.prCreate` (`draft: true`, label `uplink:amend`; the body is the stored message after an HTML-comment instruction block).

`--complete` runs on the merged base (or `-work`). It squashes everything above the patch's own commit into the patch, takes `--title` / `--message[-file]` as the new title and message, re-assesses, and runs export preflight (upstream) or the preflight script (internal). A failing upstream assessment is refused and the branch is left as it was. A submitted patch becomes `amended` (IP approves the delta, then submit force-pushes the contrib branch) and a patch that was not submitted becomes `queued`, unless the amend adds and removes the same lines as before and leaves the public title and message as they were: then the last approval still covers the patch and it stays `submitted` or `approved`. Then `main` is rebuilt, and a follow-on conflict prints like `resolve`. A merge with no code or message change prints `changed: false` and leaves the queue as it was.

- `<id>`: Patch to amend.
- `--complete`: Fold the merged amend PR into the patch.
- `--title <text>`: New patch title.
- `--message <text>`: New commit message, as the PR title and body.
- `--message-file <path>`: New commit message, as the PR title and body (- reads stdin).
- `--preflight-result <path>`: Take the verdict of preflight.sh from this file instead of running it.

  The file is the output of `git uplink preflight --json`, or of this command with `--preflight-only`, from a job without credentials. It is used only when it is for the tree this command builds; otherwise the command fails and preflight has to run again.
- `--preflight-only`: Run preflight.sh on what this command would test, print the result as JSON, change nothing.

  For a job without credentials; pass the output to the same command with `--preflight-result`. Exits non-zero when the result is not a pass. The checkout is left on the squashed work, so use a clone made for it.

### `drop`

Remove a patch from the queue.

- `<id>`: Patch to drop.
- `--reason <text>`: Why it was dropped; "dropped by operator" when omitted.

### `rebuild`

Replay main from the queue.

Applies the active patches in order onto `uplink/upstream`, one commit each. A patch that does not apply goes to the conflict gate.

When every patch applies and the result is not the tree `main` already has, `preflight.sh` runs on it. If it fails, `git bisect` runs the script between `uplink/upstream` (known good) and the rebuilt tree (known bad), and the first patch it fails on goes to the conflict gate: `uplink/conflict/<id>` is the queue before the patch and `uplink/conflict/<id>-work` has the patch applied, for the fix. `main` stays at the last build that passed. When `uplink/upstream` is not known to pass with the current `preflight.sh`, it is tested first; if it fails too, the command fails and no patch is blamed.

Every command that rebuilds does this: `sync`, `accept-upstream`, `resolve`, `amend --complete`, `transfer`, `drop` and `merged`. `add` does not: what it imports was merged into `main` by a reviewed pull request, and its rebuild only moves that change under the internal patches.

- `--branch <name>`: Rebuild onto uplink/preview/\<name> instead of company main (preview; does not mutate the queue or push).
- `--push`: Push uplink/state and the rebuilt branch after rebuild.
- `--push-remote <remote>`: Remote for --push, origin by default.
- `--preflight-result <path>`: Take the verdict of preflight.sh from this file instead of running it.

  The file is the output of this command with `--preflight-only`, from a job without credentials. It is used only when it is for the trees this command builds; otherwise the command fails and preflight has to run again.
- `--preflight-only`: Run preflight.sh on what this command would test, print the result as JSON, change nothing.

  For a job without credentials; pass the output to the same command with `--preflight-result`. Exits 0 also when a patch fails: the command then gates that patch.

### `gated`

Record the company PR that gates a conflict.

- `<id>`: Patch the PR gates.
- `--pr-url <url>`: URL of the company pull request.
- `--pr <n>`: Pull request number, read from the URL when omitted.
- `--push-remote <remote>`: Remote to push uplink/state to. Default: `origin`.

### `resolve`

Finish a conflict resolution and put the patch back in the queue.

Resolve re-runs the upstream assessment on the resolution and refuses an upstream-bound resolution that fails it, leaving the branch and staged files as they were. The gate check runs the same assessment on the conflict PR, so a failing resolution cannot merge. For internal-only patches the gate skips the assessment on conflict and amend PRs and runs only the preflight script (`git uplink preflight --command-only`).

A submitted patch whose resolution adds or removes other lines than the last approval covered becomes `amended` until IP approves the delta. That includes keeping the patch's line over an upstream change of the same line: the patch then removes upstream's new line. A resolution that only follows upstream changes next to the patch's lines changes nothing that was approved: the patch stays `submitted` (or `approved`) and needs no other approval. Resolve of a submitted patch dispatches a new submit for you either way, so the public PR gets the patch on the new upstream. A follow-on conflict prints `gh.prCreate` JSON for the next gated PR and exits 0. That includes a patch `preflight.sh` fails on in the rebuild, which can be the resolved patch again; see `rebuild`.

`readyToSubmit` in the output lists the resolved patch when it is `queued` again, and the queued patches whose last unmerged upstream dependency the rebuild marked merged.

- `<id>`: Patch whose conflict was resolved.
- `--preflight-result <path>`: Take the verdict of preflight.sh from this file instead of running it.

  The file is the output of this command with `--preflight-only`, from a job without credentials. It is used only when it is for the trees this command builds; otherwise the command fails and preflight has to run again.
- `--preflight-only`: Run preflight.sh on what this command would test, print the result as JSON, change nothing.

  For a job without credentials; pass the output to the same command with `--preflight-result`. The checkout is left on the squashed resolution, so use a clone made for it.

## Tools

### `web-ui`

Start the embedded operator dashboard on 127.0.0.1 and open a browser.

Serves the local operator UI for this checkout. It reads `.uplink/queue.json` from `uplink/state` in the directory you started in, and can also inspect `origin/uplink/state` after a fetch. The UI is embedded in the binary.

- `--port <port>`: Port to listen on. Default: `43721`.
- `--no-open`: Do not launch a browser.

### `man`

Write the man pages into a directory.

Writes `git-uplink.1` and one page per command, generated from this help text when the binary was built. Give it a `man1` directory on the man path, for example `git uplink man ~/.local/share/man/man1`. After that, `git uplink --help` shows the manual through Git, and `man git-uplink-<command>` the page of one command.

- `<dir>`: Directory to write the pages into; created when missing.

### `version`

Print the git uplink version and the commit it was built from.

Same as `git-uplink --version`, for example `git-uplink 0.1.0 (0c393c4)`. A `-dirty` suffix means tracked files other than `Cargo.toml` / `Cargo.lock` had local changes at build time. `unknown` means the build had no Git checkout. Packagers can set `GIT_UPLINK_COMMIT` to override it.

## Where state lives

Queue state lives on the orphan branch `uplink/state` as `.uplink/queue.json` and `.uplink/patches/*.patch`.

Company `main` is product-only: public upstream, then the tooling patch, then every active `upstream[]` and `internal[]` patch.

Reports live under `.uplink/reports/<id>/` on `uplink/state`, so a product rebuild never drops them.

## Settings in uplink.toml

The leak scan's keywords and internal email domains live in `uplink.toml` on `uplink/hooks`, not in environment or repository variables. Every command reads the file from the local `uplink/hooks`, else `origin/uplink/hooks`, so a developer's machine and CI agree.

    # Words that must not appear in a contribution.
    redact_keywords = ["AcmeCorp", "companyTelemetry"]

    # Email domains flagged in the export.
    internal_email_domains = ["acme.example"]

When `init` creates `uplink/hooks` it asks for each setting in a terminal. A flag answers its question. Without a terminal, a setting with no flag is written empty and init says which.

`init --upgrade` adds the file when it is missing and appends settings the file lacks, asking the same way. It offers values an older `queue.json` held. It never changes a value that is already there; edit the file on `uplink/hooks` for that.

With a forge recorded, a missing or unparseable `uplink.toml` fails upstream-bound `assess` instead of scanning for nothing.

Moving from repository variables: run `git uplink init --upgrade`, answer with the values of `UPLINK_PREFLIGHT`, `UPLINK_REDACT_KEYWORDS`, and `UPLINK_INTERNAL_DOMAINS`, run `git uplink push`, then delete the three variables. They are no longer read.

## Preflight script preflight.sh

What preflight runs is the script `preflight.sh` at the root of `uplink/hooks`, read from the local branch, else `origin/uplink/hooks`.

- How it runs: `sh preflight.sh`, with the root of the tree under test as the working directory: the export tree, or the current checkout for `--command-only`. A non-zero exit fails preflight.

- Output: what the script prints is shown as it runs, stdout and stderr together. `git uplink preflight` shows it on stdout. With `--json`, and in every other command that runs the script, it goes to stderr, because stdout is the result.

- Other files on the branch are checked out beside the script for the run; reach them with `"$(dirname "$0")"`.

- Environment: the caller's, without `GITHUB_TOKEN`, `GH_TOKEN`, `GH_ENTERPRISE_TOKEN`, `ACTIONS_RUNTIME_TOKEN`, `ACTIONS_ID_TOKEN_REQUEST_TOKEN`, `UPLINK_*_TOKEN` and `UPLINK_*_KEY`.

- Credentials: the script builds and runs product code, so it must not run where credentials are. An emptied environment does not hide them from a process on the same runner. In CI (`CI` or `GITHUB_ACTIONS` set) with a forge recorded, a command that holds a credential refuses to run the script. Run `git uplink preflight --json` (or `transfer` / `amend --complete` with `--preflight-only`) in a job without credentials, and give the output to `add`, `submit`, `transfer` or `amend --complete` with `--preflight-result <file>`. The result carries a token for the tree and the hooks it was tested with; a command accepts it only for the same tree.

- Known limitations: the job that runs the script still holds a read-only token and a clone of the company repository, so code the script runs can read company source. The verdict is the script's exit code, which code it runs could force to 0. Preflight checks that a change builds and passes its tests; it is not a defence against hostile code in the tree.

- Created by `init`, with the answer to its preflight question (or `--preflight <cmd>`) as the script's command. `init --upgrade` adds the script when the branch lacks it, offering the command an older `queue.json` held. An existing script is never rewritten; edit it on `uplink/hooks`.

- No script means preflight runs no command. With a forge recorded, a missing `uplink/hooks` fails preflight.

- Trying a change: commit it on a branch made from `uplink/hooks`, then run `git uplink preflight --command-only --hooks <branch>`. `--hooks` works with every form of `preflight`.

## Adopting an existing main

If company `main` already matches public upstream, init rebuilds `main` with the tooling patch.

If `main` is fast-forward ahead, init leaves `main` alone and records the unique first-parent commits as patches after tooling.

Group rebase-style history in the terminal UI, or pass `--adopt-groups` JSON:

    [{ "commits": ["abc123", "def456"], "title": "...", "intent": "upstream" }]

Adopt-group `intent` is `upstream` (the default) or `internal-only`; it is not stored on the patch. Merge commits are one row each (the merge SHA, not the hidden PR branch).

Then preview and publish:

    git uplink rebuild --branch uplink/preview/verify
    git diff main uplink/preview/verify
    git uplink rebuild --push

## Gated PRs

`sync`, `resolve`, `transfer`, and `amend` print JSON (`gh.prCreate`, `gh.prClose`) for the company PR that gates a conflict or a change, and exit 0. Callers use the JSON, not the process status, to open company PRs.

## Credentials and identity

git-uplink shells out to `git`, but it does not use the operator's commit signer, default SSH key, or `GITHUB_TOKEN`.

Bot identity and `commit.gpgsign=false` are process-scoped (`git -c`), so `git uplink init` does not rewrite `user.name` / `commit.gpgsign` in the clone. Your own `git commit` in that repo still follows global signing.

Network git picks credentials by remote:

- `origin`: `UPLINK_INTERNAL_KEY` or `UPLINK_INTERNAL_TOKEN`. Required for SSH.

- `contrib`: `UPLINK_CONTRIB_KEY` or `UPLINK_CONTRIB_TOKEN`. Required for SSH.

- `upstream`: `UPLINK_UPSTREAM_KEY` or `UPLINK_UPSTREAM_TOKEN`. An `https://` upstream may omit both and is fetched anonymously.

If both KEY and TOKEN are set, KEY wins.

A KEY is a path to a passwordless private key (`BatchMode=yes`); a passphrase-protected key fails closed.

A TOKEN rewrites SSH remotes to HTTPS for that invocation and is sent when present, including on an already-HTTPS upstream.

Local `file://` remotes need neither.

SSH upstream, origin, and contrib without the matching role's creds fail instead of opening ssh-agent / Touch ID.
