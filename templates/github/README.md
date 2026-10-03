# GitHub workflows (`--forge github`)

`git uplink init --forge github` installs everything under [`.github/`](.github/) as the tooling patch on company `main`: the workflows in [`.github/workflows/`](.github/workflows/), the [pull request template](.github/pull_request_template.md), the [`install-git-uplink`](.github/actions/install-git-uplink/action.yml) action, the actions that call the company hooks, and sample rulesets. `git uplink init --upgrade` refreshes them from the binary. Do not edit them by hand. The pack is the same for github.com and GitHub Enterprise Cloud.

For setting up a repository step by step, see [Production setup](https://npetzall.github.io/git-uplink/setup?forge=github&view=steps).

Every job installs the `git-uplink` release named by `UPLINK_SRC` / `UPLINK_VERSION`, then runs `git uplink init` to hydrate its checkout from `origin`. Gated pull requests are opened with the internal App or PAT, not `GITHUB_TOKEN`, so the repository setting "Allow GitHub Actions to create and approve pull requests" can stay off, and the Uplink gate check runs as soon as the PR opens. `UPLINK_*_AUTH` selects `app` (mint an installation token from `UPLINK_*_APP_ID` + `UPLINK_*_APP_PRIVATE_KEY`, the default) or `pat` (use `UPLINK_*_TOKEN`). Workflows that write the queue share the concurrency group `uplink-mutate` with `queue: max`, so they run one at a time and none is dropped. Jobs that wait on an Environment never hold that group.

## `uplink-pr.yml` — Uplink PR checks

- **Runs on:** pull requests to `main` (opened, synchronize, reopened, edited). Skipped for `uplink:internal-only`.
- **Does:** two parallel jobs, both required checks.
  - **Uplink upstream assess:** turns the PR title and body into the commit message, strips everything below the cutoff, turns `Uplink-Export-Author` into a `Co-Authored-By` trailer, and scans for company keywords and internal email domains. It also runs the optional assessment hook (below) with `pr`. Both results go into one PR comment that is updated in place on every run, and into artifact `uplink-assessment` for import. A failed hook is noted in the comment; it does not fail the check.
  - **Uplink upstream preflight:** applies the change onto public upstream plus declared `Uplink-Depends-On`, then runs `preflight.sh` from `uplink/hooks`. A failure is commented on the PR; later runs update that comment.
- **Requires:**
  - `uplink.toml` (`redact_keywords`, `internal_email_domains`) and `preflight.sh` on `uplink/hooks` (see [Settings](#settings));
  - the Actions token (contents read, pull requests write, actions write to run the hook);
  - label `uplink:internal-only`.

## `uplink-import.yml` — Uplink import

- **Runs on:** a pull request merged into `main`. A merge into any other branch is not imported; `git uplink add --base-branch` refuses it as well.
- **Does:** `git uplink add` records the merged change on `uplink/state` as `queued`, in `internal[]` when labelled `uplink:internal-only`, otherwise in `upstream[]`. An upstream import rebuilds `main` so the patch sits under `internal[]`. Then `git uplink push`.
  - For an upstream patch, import keeps the assessment-hook result from the PR checks when it was made for exactly what was merged (head commit, title and body). It is stored under `.uplink/reports/<id>/extras/`.
- **Requires:**
  - the Actions token (actions read) to download the PR check's `uplink-assessment` artifact;
  - the internal App or PAT (contents and workflows write), because the rebuild force-pushes `main`, which contains workflow files;
  - concurrency group `uplink-mutate`.

## `uplink-submit.yml` — Uplink submit

- **Runs on:** manual dispatch from `main` with a `patch_id`, and dispatch by resolve for an `amended` patch.
- **Does:**
  1. **Extras:** stops if an upstream-bound `Uplink-Depends-On` patch is not merged upstream yet. Otherwise it uses the company extras stored at import while the patch is unchanged. Otherwise it runs the optional assessment hook (below) with `patch`. It reads `uplink/state` but does not take `uplink-mutate`, so a slow hook never blocks the queue.
  2. **Packet:** `git uplink report` writes `.uplink/reports/<id>/assessment.md` on `uplink/state`, with the extras first, and to the job summary. A successful hook result is stored for later packets.
  3. **Submit:** waits on Environment `to-upstream`. After approval it records `approval.md`, runs `git uplink approve` and `git uplink submit`, which builds the export commit on `uplink/upstream`. `.github/uplink/contrib_commit.py` then recreates that commit on the fork through the Git Database API and moves `uplink/<id>` to it. GitHub signs the commit, and it is Verified only with the contrib App (a PAT commit is unverified). The job then opens the public PR (`maintainer_can_modify` false), and runs `git uplink submitted`. If the patch already has a PR number, no second PR is opened.
- **Requires:**
  - Environment `to-upstream` with the IP reviewers and the contrib App (or PAT, without Verified commits) with fork contents write as environment secrets;
  - repository secrets for the upstream App or PAT (contents read, pull requests write on upstream and the fork);
  - the Actions token: contents read and actions write for the extras job, contents write for the packet job;
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
  - If the patch was already submitted, it marks it `amended`, cancels any running `Uplink submit <id>`, and dispatches a new one for the delta.
- **Requires:**
  - the internal App or PAT (contents, workflows, and pull requests write), which opens the next conflict PR;
  - the upstream App or PAT for fetch;
  - the Actions token (actions write to dispatch submit);
  - label `uplink:conflict`;
  - concurrency group `uplink-mutate`.

## `uplink-transfer.yml` — Uplink transfer

- **Runs on:** manual dispatch with `patch_id` and `direction` (`to-upstream` / `to-internal`), and closed pull requests into `uplink/transfer-to-*/**`.
- **Does:**
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
  - **Complete:** merging runs `--complete` with the PR title and description, rebuilds `main`, and deletes both branches. A submitted patch becomes `amended`, and the job dispatches **Uplink submit** for the delta. If the rebuild stops on a later patch, it opens that conflict PR the same way sync does.
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
- **Does:** job **Uplink gate** fails if conflict markers remain or if the PR changes pack files (`uplink-*.yml`, `install-git-uplink`). For conflict PRs it also runs the upstream assessment on the resolution with the patch's stored message, so a resolution that would leak company text cannot merge. For transfer-to-upstream PRs it runs export preflight. For amend PRs it assesses the whole amended patch with the PR title and description as its message. Conflict and amend PRs for internal-only patches skip the assessment and run only the preflight script (`git uplink preflight --command-only`), as transfer-to-internal PRs do.
- **Requires:**
  - `preflight.sh` on `uplink/hooks`;
  - the Actions token (contents read);
  - make **Uplink gate** a required check on the gated bases. Before this job had a name its check was called `validate`; update existing rulesets or branch protection to the new name.

## Settings

- **`uplink.toml` on `uplink/hooks`** holds what the CLI needs and the workflows do not: `redact_keywords` (words that must not appear in a contribution) and `internal_email_domains`. They are not repository variables, so `git uplink assess` gives the same result on a developer's machine and in CI.
- **`preflight.sh` on `uplink/hooks`** is the script preflight runs, with `sh`, from the root of the export tree. The rest of the branch is checked out beside it for the run. `init` creates it with the command given to `--preflight` (or to its question); change it by editing the file on `uplink/hooks`. Try a change with `git uplink preflight --command-only --hooks <branch>`.
- **Written by `git uplink init`,** which asks for each setting in a terminal, or takes `--redact-keyword` and `--internal-domain`. `init --upgrade` adds the file, or settings a newer binary introduces, and never changes a value that is there. Change values by editing the file on `uplink/hooks`.
- **Read from** the local `uplink/hooks`, else `origin/uplink/hooks`. Every job runs `git uplink init`, which fetches the branch. Gated jobs read the branch, never the pull request, so a PR cannot change the command they run.
- **Missing or broken `uplink.toml`:** upstream-bound assess fails rather than scan for nothing. `git uplink doctor` reports it.
- **Protect it:** the file decides what CI runs and what counts as a leak. Import [`uplink-hooks-ruleset.json`](.github/uplink-hooks-ruleset.json) so changes need a reviewed pull request.

## Assessment hook

- **Optional and company-owned.** The real hook lives on the orphan branch `uplink/hooks`, which is never queued, replayed, or contributed.
- **Created by `git uplink init`:** the local orphan branch `uplink/hooks` with `assessment-hook.md`, `toolchain-hook.md`, a toolchain hook stub, and a starter `.github/workflows/uplink-assessment-hook-example.yml`. `git uplink push` publishes it with `uplink/state`. `init` never changes an existing file on it; `init --upgrade` creates the branch when it is missing and adds files a newer pack brings. `git uplink doctor` reports when it is missing, lacks the toolchain hook, or is not pushed.
- **Placeholder in the pack:** `.github/workflows/uplink-assessment-hook.yml` on `main`. GitHub only dispatches a workflow whose file exists on the default branch, so submit needs this file there. Run it by hand to print `assessment-hook.md` from `uplink/hooks`. It writes nothing.
- **Runs on:** dispatch with `--ref uplink/hooks` by the PR checks (input `pr`) and by submit's extras job (input `patch`) when no current result is stored. `caller_run_id` must appear in `run-name`. If `uplink/hooks` has no `.github/workflows/uplink-assessment-hook.yml`, callers skip the hook.
- **Does:** whatever company scans must reach the IP packet. It uploads `*.md` files as artifact `uplink-packet-extra`. The PR comment shows them, import stores them with the patch, and the packet prepends them to `assessment.md` before IP is asked. A failed hook fails neither the PR check nor submit: the run shows a warning, and the comment or `assessment.md` starts with a note that links to the failed hook run. A failed result is never stored.
- **Requires:**
  - `.github/workflows/uplink-assessment-hook.yml` on `uplink/hooks` (copy the `-example` starter). Adding it needs workflows write;
  - it must not push `uplink/state`;
  - with `pr`, the checked-out code is unmerged. Do not run it with secrets;
  - recommended: import [`uplink-hooks-ruleset.json`](.github/uplink-hooks-ruleset.json), so changes to `uplink/hooks` need a reviewed pull request. Its code runs on every PR check and submit.
- Guide: [`assessment-hook.md`](../github-hooks/assessment-hook.md), starter: [`uplink-assessment-hook-example.yml`](../github-hooks/.github/workflows/uplink-assessment-hook-example.yml). Walkthrough: [story 07](../../examples/github/stories/07-assessment-hook.md).

## Toolchain hook

- **Company-owned.** A composite action at `.github/actions/uplink-toolchain-hook/action.yml` on `uplink/hooks` that installs what `preflight.sh` needs (runtimes, package managers, system packages). `init` creates a stub that only prints `toolchain-hook.md` to the job summary.
- **Runs in:** every job that runs preflight, right before its Uplink step: PR checks (**Uplink upstream preflight**), gate, import, submit, amend, and transfer (start and complete). The pack's `.github/actions/uplink-toolchain-hook` on `main` checks out `uplink/hooks` into `.uplink-hooks/` and runs the hook from there.
- **Missing hook:** the job shows a notice and continues. **A failed hook fails the job**, since preflight without its toolchain would fail anyway.
- **Requires:** keep it to installing pinned tools; import, submit, amend, and transfer hold write tokens and Environment secrets. Do not build or run product code in it.
- Guide and examples: [`toolchain-hook.md`](../github-hooks/toolchain-hook.md).
