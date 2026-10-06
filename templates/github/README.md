# GitHub workflows (`--forge github`)

`git uplink init --forge github` installs everything under [`.github/`](.github/) as the tooling patch on company `main`: the workflows in [`.github/workflows/`](.github/workflows/), the [pull request template](.github/pull_request_template.md), the [`install-git-uplink`](.github/actions/install-git-uplink/action.yml) action, the actions that call the company hooks, and sample rulesets. `git uplink init --upgrade` refreshes them from the binary. Do not edit them by hand. The pack is the same for github.com and GitHub Enterprise Cloud.

For setting up a repository step by step, see [Production setup](https://npetzall.github.io/git-uplink/setup?forge=github&view=steps).

Every job installs the `git-uplink` release named by `UPLINK_SRC` / `UPLINK_VERSION`, then runs `git uplink init` to hydrate its checkout from `origin`. Init stops if `origin` has `uplink/state` but no `uplink/upstream`: that branch is the accepted public base and is never seeded from public upstream again. Gated pull requests are opened with the internal App or PAT, not `GITHUB_TOKEN`, so the repository setting "Allow GitHub Actions to create and approve pull requests" can stay off, and the Uplink gate check runs as soon as the PR opens. `UPLINK_*_AUTH` selects `app` (mint an installation token from `UPLINK_*_APP_ID` + `UPLINK_*_APP_PRIVATE_KEY`, the default) or `pat` (use `UPLINK_*_TOKEN`). Workflows that write the queue share the concurrency group `uplink-mutate` with `queue: max`, so they run one at a time and none is dropped. Jobs that wait on an Environment never hold that group.

**The pack and the binary are one version.** The workflows call the CLI with the flags of the release they shipped with, and the CLI expects the jobs of its own pack. Upgrade them together: set `UPLINK_VERSION` to the new release and run `git uplink init --upgrade` with that same release, then `git uplink push`. A new binary under an old pack, or the reverse, is not supported and fails in the workflows.

## `uplink-pr.yml` — Uplink PR checks

- **Runs on:** pull requests to `main` (opened, synchronize, reopened, edited, labeled, unlabeled). Skipped for `uplink:internal-only`. Adding or removing any label runs the checks again, so removing `uplink:internal-only` cannot leave the skipped checks standing as passed.
- **Does:** two parallel jobs, both required checks, and a third that posts the preflight comment.
  - **Uplink upstream assess:** turns the PR title and body into the commit message, strips everything below the cutoff, turns `Uplink-Export-Author` into a `Co-Authored-By` trailer, and scans for company keywords and internal email domains. It uploads the assessment package (`git uplink assess --package`: the result, the public message, the `uplink.toml` settings and the diff) as artifact `uplink-assessment`, and runs the optional assessment hook (below), which reads it. Both results go into one PR comment that is updated in place on every run. A failed hook is noted in the comment; it does not fail the check. Both are advice: the change is not a patch yet, and nothing from this run is stored. **Uplink submit** assesses the patch.
  - **Uplink upstream preflight:** applies the change onto public upstream plus declared `Uplink-Depends-On`, then runs `preflight.sh` from `uplink/hooks`, in a step with no token. Job **Uplink preflight comment** posts a failure on the PR; later runs update that comment.
- **Requires:**
  - `uplink.toml` (`redact_keywords`, `internal_email_domains`) and `preflight.sh` on `uplink/hooks` (see [Settings](#settings));
  - the Actions token (contents read, pull requests write for the comments, actions write to run the hook);
  - label `uplink:internal-only`.

## `uplink-import.yml` — Uplink import

- **Runs on:** a pull request merged into `main`. A merge into any other branch is not imported; `git uplink add --base-branch` refuses it as well.
- **Does:** job **Preflight (no credentials)** runs `preflight.sh` on the export tree with a read-only token. Job **import** takes its result: `git uplink add` records the merged change on `uplink/state` as `queued`, in `internal[]` when labelled `uplink:internal-only`, otherwise in `upstream[]`. An upstream import rebuilds `main` so the patch sits under `internal[]`. Then `git uplink push`.
  - `git uplink add` runs the upstream assessment on the merged change and refuses an upstream-bound change that fails it. Import stores no assessment-hook result; submit runs the hook on the patch.
- **Requires:**
  - `preflight.sh` on `uplink/hooks`;
  - the Actions token (contents read for the preflight job, actions read to download the PR check's `uplink-assessment` artifact);
  - the internal App or PAT (contents and workflows write), because the rebuild force-pushes `main`, which contains workflow files;
  - concurrency group `uplink-mutate`.

## `uplink-submit.yml` — Uplink submit

- **Runs on:** manual dispatch from `main` with a `patch_id`, and dispatch by resolve or amend for a patch that has a public PR.
- **Does:**
  1. **Assess:** stops if an upstream-bound `Uplink-Depends-On` patch is not merged upstream yet. Otherwise it runs `git uplink assess --patch <id> --package` on the patch file, with today's `uplink.toml`, and uploads the assessment package as artifact `uplink-assessment`. It applies nothing; the preflight job checks that the patch still applies. It then runs the optional assessment hook (below), which reads that package, unless an earlier submit run stored the hook's result and the patch is unchanged since. It reads `uplink/state` but does not take `uplink-mutate`, so a slow hook never blocks the queue.
  2. **Packet:** `git uplink report --assess-result` stores the assessment of that package with the patch. It refuses a package that is not for the patch as it is now: the patch file, title, message or `uplink.toml` changed since, or the result is not what assessing the patch gives. It then writes `.uplink/reports/<id>/assessment.md` on `uplink/state`, with the extras first, and to the job summary. A successful hook result is stored for later packets. With assessment findings the packet is still written, and the run stops before the Environment. It also writes the packet's review token to `.uplink/reports/<id>/review-token`. The token names what the patch changes (the lines it adds and removes per file, and files it creates, deletes, renames or replaces), the public title and the public message. The unchanged lines around them are not part of it, so a replay onto a moved upstream keeps the token.
  3. **Preflight:** in parallel with the first two, runs `preflight.sh` on the export tree with a read-only token and no Environment.
  4. **Submit:** waits on Environment `to-upstream`, which links the packet at the commit this run wrote. After approval it runs `git uplink approve --reviewed <token>` with this run's token: if the lines the patch adds or removes, its title or its message changed while the run waited, it stops and nothing is exported; dispatch submit again for a new packet. When the last approval already covers the patch (the packet says so at the top), the click only releases the credentials and no new approval is recorded. Otherwise it records `approval.md` and runs `git uplink submit` with the preflight job's result, which builds the export commit on `uplink/upstream`. If `uplink/upstream` or `uplink/hooks` moved since the preflight job ran, submit stops; dispatch it again. `.github/uplink/contrib_commit.py` then recreates that commit on the fork through the Git Database API and moves `uplink/<id>` to it. GitHub signs the commit, and it is Verified only with the contrib App (a PAT commit is unverified). The job then opens the public PR (`maintainer_can_modify` false), and runs `git uplink submitted`. If the patch already has a PR number, no second PR is opened.
- **Requires:**
  - Environment `to-upstream` with the IP reviewers and the contrib App (or PAT, without Verified commits) with fork contents write as environment secrets;
  - repository secrets for the upstream App or PAT (contents read, pull requests write on upstream and the fork);
  - `preflight.sh` on `uplink/hooks`;
  - the Actions token: contents read and actions write for the assess job, contents write for the packet job, contents read for the preflight job;
  - `uplink-mutate` on the packet job only.

## `uplink-sync.yml` — Uplink sync

- **Runs on:** manual dispatch. `uplink-sync-schedule.yml` (**Uplink sync schedule**) dispatches it every hour with the Actions token (actions write); that is all it does.
- **Does:**
  - **Merged PRs:** before sync, the job asks the upstream repository whether the public PR recorded for each active upstream patch is merged, and passes each merge commit as `git uplink sync --merged-pr <id>=<sha>`.
  - **Inspect:** `git uplink sync` fetches public upstream without moving `uplink/upstream`. A patch is merged when a commit has its stable patch id or its public PR is merged; an `Uplink-Patch-Id` trailer alone is not enough. When the merged patches explain the whole range, sync promotes, marks them merged, and rebuilds at once. Otherwise it writes the remaining diff to `.uplink/reports/from-upstream/incoming.md`.
  - **Wait:** holds on Environment `from-upstream`. The packet lists the patches that approval marks merged, any that the maintainer changed, and commits whose trailer names a patch they do not match.
  - **Apply:** after approval, `git uplink accept-upstream --sha <reviewed sha>` promotes, marks those patches merged, and rebuilds. It stops if the pending upstream is no longer the reviewed one.
  - **Conflict:** a conflict pushes `uplink/conflict/<id>` plus `-work` and opens a gated PR labelled `uplink:conflict`. The run stays green.
- **Requires:**
  - the internal App or PAT (contents, workflows, and pull requests write), which also opens the gated PR;
  - the upstream App or PAT, optional for an `https://` upstream. It also reads the public PRs (pull requests read); without it a merged patch is still found by its patch id or an empty apply;
  - Environment `from-upstream` with inbound reviewers and no secrets;
  - label `uplink:conflict`;
  - concurrency group `uplink-sync` for the workflow and `uplink-mutate` for inspect and apply.

## `uplink-resolve.yml` — Uplink resolve

- **Runs on:** a merged pull request into `uplink/conflict/**` (`pull_request_target`, so the YAML comes from the default branch).
- **Does:**
  - `git uplink resolve <id>` refreshes the patch, re-runs the upstream assessment on the resolution (refusing an upstream-bound resolution that fails it), rebuilds `main`, and deletes the base and `-work` branches.
  - If the rebuild stops on a later patch, it opens that conflict PR the same way sync does.
  - If the patch was already submitted, it cancels any running `Uplink submit <id>` and dispatches a new one. A resolution that adds or removes other lines than the last approval covered marks the patch `amended`, and that run asks IP to approve the delta. Keeping the patch's line over an upstream change of the same line is such a case: the patch now removes upstream's new line. A resolution that leaves those lines as approved (upstream only changed lines next to them) leaves the patch `submitted`: the run exports it onto the new upstream, and no new approval is recorded.
- **Requires:**
  - the internal App or PAT (contents, workflows, and pull requests write), which opens the next conflict PR;
  - the upstream App or PAT for fetch;
  - the Actions token (actions write to dispatch submit);
  - label `uplink:conflict`;
  - concurrency group `uplink-mutate`.

## `uplink-transfer.yml` — Uplink transfer

- **Runs on:** manual dispatch with `patch_id` and `direction` (`to-upstream` / `to-internal`), and closed pull requests into `uplink/transfer-to-*/**`.
- **Does:**
  - **Preflight:** before start and before complete, a job with a read-only token runs `git uplink transfer --preflight-only`, which runs `preflight.sh` on the tree the transfer would test and changes nothing. The writing job takes its result.
  - **Start:** runs `git uplink transfer`. If apply, assess (for `to-upstream`), and preflight pass, the queue changes at once. Otherwise it pushes a protected base plus `-work` and opens a transfer PR.
  - **Complete:** merging the PR runs `--complete`.
  - **Abort:** closing the PR without merging deletes both branches.
  - A successful `--to-internal` of a submitted patch dispatches **Uplink abandon contrib**.
- **Requires:**
  - the internal App or PAT (contents, workflows, and pull requests write), which opens the transfer PR;
  - the upstream App or PAT for fetch;
  - `preflight.sh` on `uplink/hooks`;
  - the Actions token (actions write);
  - labels `uplink:transfer-to-upstream` and `uplink:transfer-to-internal`;
  - concurrency group `uplink-mutate`.

## `uplink-amend.yml` — Uplink amend

- **Runs on:** manual dispatch with `patch_id`, and closed pull requests into `uplink/amend/**`.
- **Does:**
  - **Start:** refuses if `uplink/amend/<id>` already exists. Runs `git uplink amend`, which replays the queue up to and including the patch. Pushes that as the protected base plus a `-work` branch, and opens a draft PR. Its title and description are the patch title and message.
  - Push the change to `-work` and edit the PR title and description as needed. Mark it ready and merge.
  - **Complete:** merging first runs `git uplink amend --complete --preflight-only` in a job with a read-only token, which runs `preflight.sh` on the amended patch and changes nothing. Then `--complete` runs with that result and the PR title and description, rebuilds `main`, and deletes both branches. For a submitted patch the job dispatches **Uplink submit**: for the delta when the amend made the patch `amended` (it changes the lines the patch adds or removes, or the public title or message), or only to export it again when the last approval still covers it. If the rebuild stops on a later patch, it opens that conflict PR the same way sync does.
  - **Abort:** closing the PR without merging deletes both branches. The queue is unchanged.
- **Requires:**
  - the internal App or PAT (contents, workflows, and pull requests write), which opens the amend PR;
  - the upstream App or PAT for fetch;
  - `preflight.sh` on `uplink/hooks`;
  - the Actions token (actions write to dispatch submit);
  - label `uplink:amend`;
  - concurrency group `uplink-mutate`.

## `uplink-abandon.yml` — Uplink abandon contrib

- **Runs on:** dispatch by transfer after `--to-internal` of a submitted patch.
- **Does:** waits on Environment `abandon-contrib`, then closes the public PR and deletes `uplink/<id>` on the fork. Until it is approved, the queue already says internal-only while the public PR remains.
- **Requires:**
  - Environment `abandon-contrib` with engineering reviewers and a copy of the contrib secrets (Environments do not share secrets);
  - the upstream App or PAT (pull requests write) to close the PR.
  - Does not take `uplink-mutate`.

## `uplink-gate.yml` — Uplink gate

- **Runs on:** pull requests into `uplink/conflict/**`, `uplink/transfer-to-upstream/**`, `uplink/transfer-to-internal/**`, and `uplink/amend/**` (`pull_request_target`), including title and description edits.
- **Does:** job **Uplink gate** fails if conflict markers remain or if the PR changes pack files (`uplink-*.yml`, `install-git-uplink`, `uplink-*` actions, `.github/uplink/`) or anything under `.uplink/`. For conflict PRs it also runs the upstream assessment on the resolution with the patch's stored message, so a resolution that would leak company text cannot merge. For transfer-to-upstream PRs it runs export preflight. For amend PRs it assesses the whole amended patch with the PR title and description as its message. Conflict and amend PRs for internal-only patches skip the assessment and run only the preflight script (`git uplink preflight --command-only`), as transfer-to-internal PRs do. The step that assesses and runs `preflight.sh` has no token.
- **Requires:**
  - `preflight.sh` on `uplink/hooks`;
  - the Actions token (contents read);
  - make **Uplink gate** a required check on the gated bases. Before this job had a name its check was called `validate`; update existing rulesets or branch protection to the new name.

## Settings

- **`uplink.toml` on `uplink/hooks`** holds what the CLI needs and the workflows do not: `redact_keywords` (words that must not appear in a contribution) and `internal_email_domains`. They are not repository variables, so `git uplink assess` gives the same result on a developer's machine and in CI.
- **`preflight.sh` on `uplink/hooks`** is the script preflight runs, with `sh`, from the root of the export tree. The rest of the branch is checked out beside it for the run. `init` creates it with the command given to `--preflight` (or to its question); change it by editing the file on `uplink/hooks`. Try a change with `git uplink preflight --command-only --hooks <branch>`.
- **`preflight.sh` never runs next to a write token.** It builds and runs product code, public upstream's included. Import, submit, amend and transfer each have a preflight job with a read-only token, no App token, no Environment and no credentials in the checkout; it prints a result (`git uplink preflight --json`, or `--preflight-only`) that the writing job takes with `--preflight-result`. The result names the tree and hooks it was tested with, so it is refused if either changed in between: run the workflow again.
- **Known limitations of preflight:** the preflight job holds a read-only Actions token and a clone of the company repository, so code the script runs (public upstream's included) can read company source; it cannot write or reach the App tokens and Environment secrets. The verdict is the script's exit code, which code it runs could force to 0: preflight checks that a change builds and passes its tests, it is not a defence against hostile code in the tree.
- **Written by `git uplink init`,** which asks for each setting in a terminal, or takes `--redact-keyword` and `--internal-domain`. `init --upgrade` adds the file, or settings a newer binary introduces, and never changes a value that is there. Change values by editing the file on `uplink/hooks`.
- **Read from** the local `uplink/hooks`, else `origin/uplink/hooks`. Every job runs `git uplink init`, which fetches the branch. Gated jobs read the branch, never the pull request, so a PR cannot change the command they run.
- **Missing or broken `uplink.toml`:** upstream-bound assess fails rather than scan for nothing. `git uplink doctor` reports it.
- **Protect it:** the file decides what CI runs and what counts as a leak. Import [`uplink-hooks-ruleset.json`](.github/uplink-hooks-ruleset.json) so changes need a reviewed pull request.

## Assessment hook

- **Optional and company-owned.** The real hook lives on the orphan branch `uplink/hooks`, which is never queued, replayed, or contributed.
- **Created by `git uplink init`:** the local orphan branch `uplink/hooks` with `assessment-hook.md`, `toolchain-hook.md`, a toolchain hook stub, and a starter `.github/workflows/uplink-assessment-hook-example.yml`. `git uplink push` publishes it with `uplink/state`. `init` never changes an existing file on it; `init --upgrade` creates the branch when it is missing and adds files a newer pack brings. `git uplink doctor` reports when it is missing, lacks the toolchain hook, or is not pushed.
- **Placeholder in the pack:** `.github/workflows/uplink-assessment-hook.yml` on `main`. GitHub only dispatches a workflow whose file exists on the default branch, so submit needs this file there. Run it by hand to print `assessment-hook.md` from `uplink/hooks`. It writes nothing.
- **Runs on:** dispatch with `--ref uplink/hooks` by the PR checks (advice) and by submit's assess job (for the packet) when no current result is stored. The only input is `caller_run_id`, which must appear in `run-name`. The hook downloads artifact `uplink-assessment` from that run: `assessment.json` (`kind` is `pr` or `patch`, the result, the public message, the `uplink.toml` settings, and for a patch its queue entry), `assessment.md` and `change.patch`. If `uplink/hooks` has no `.github/workflows/uplink-assessment-hook.yml`, callers skip the hook.
- **Does:** whatever company scans must reach the IP packet. It uploads `*.md` files as artifact `uplink-packet-extra`. The PR comment shows the result of a run for a pull request and keeps nothing. The result of a run for a patch is prepended to `assessment.md` before IP is asked and stored with the patch. A failed hook fails neither the PR check nor submit: the run shows a warning, and the comment or `assessment.md` starts with a note that links to the failed hook run. A failed result is never stored.
- **Requires:**
  - `.github/workflows/uplink-assessment-hook.yml` on `uplink/hooks` (copy the `-example` starter). Adding it needs workflows write;
  - it must not push `uplink/state`;
  - `actions: read` in the hook, to download the package;
  - a package of kind `pr` describes an unmerged change and was made by the pull request's own workflow copy. Read it as data, and do not build it with secrets;
  - recommended: import [`uplink-hooks-ruleset.json`](.github/uplink-hooks-ruleset.json), so changes to `uplink/hooks` need a reviewed pull request. Its code runs on every PR check and submit.
- Guide: [`assessment-hook.md`](../github-hooks/assessment-hook.md), starter: [`uplink-assessment-hook-example.yml`](../github-hooks/.github/workflows/uplink-assessment-hook-example.yml). Walkthrough: [story 07](../../examples/github/stories/07-assessment-hook.md).

## Toolchain hook

- **Company-owned.** A composite action at `.github/actions/uplink-toolchain-hook/action.yml` on `uplink/hooks` that installs what `preflight.sh` needs (runtimes, package managers, system packages). `init` creates a stub that only prints `toolchain-hook.md` to the job summary.
- **Runs in:** every job that runs `preflight.sh`, right before that step: PR checks (**Uplink upstream preflight**), gate, and the preflight job of import, submit, amend, and transfer (start and complete). The pack's `.github/actions/uplink-toolchain-hook` on `main` checks out `uplink/hooks` into `.uplink-hooks/` and runs the hook from there.
- **Missing hook:** the job shows a notice and continues. **A failed hook fails the job**, since preflight without its toolchain would fail anyway.
- **Requires:** keep it to installing pinned tools. Do not build or run product code in it. It never runs in a job that holds write tokens or Environment secrets.
- Guide and examples: [`toolchain-hook.md`](../github-hooks/toolchain-hook.md).
