# GitHub Enterprise Cloud workflows (`--forge ghec`)

`git uplink init --forge ghec` installs the workflows in [`.github/workflows/`](.github/workflows/) as the tooling patch on company `main`, together with the shared [pull request template](../github/pull_request_template.md) and the [`install-git-uplink`](../github/actions/install-git-uplink/action.yml) action. `git uplink init --upgrade` refreshes them from the binary. Do not edit them by hand.

For setting up a repository step by step, see [Production setup](https://npetzall.github.io/git-uplink/setup?forge=ghec&view=steps).

Every job installs the `git-uplink` release named by `UPLINK_SRC` / `UPLINK_VERSION`, then runs `git uplink init` to hydrate its checkout from `origin`. Gated pull requests are opened with the internal App or PAT, not `GITHUB_TOKEN`, so the repository setting "Allow GitHub Actions to create and approve pull requests" can stay off, and the Uplink gate check runs as soon as the PR opens. `UPLINK_*_AUTH` selects `app` (mint an installation token from `UPLINK_*_APP_ID` + `UPLINK_*_APP_PRIVATE_KEY`, the default) or `pat` (use `UPLINK_*_TOKEN`). Workflows that write the queue share the concurrency group `uplink-mutate` with `queue: max`, so they run one at a time and none is dropped. Jobs that wait on an Environment never hold that group.

## `uplink-pr.yml` — Uplink PR checks

- **Runs on:** pull requests to `main` (opened, synchronize, reopened, edited). Skipped for `uplink:internal-only`.
- **Does:** two parallel jobs, both required checks.
  - **Uplink upstream assess:** turns the PR title and body into the commit message, strips everything below the cutoff, turns `Uplink-Export-Author` into a `Co-Authored-By` trailer, and scans for company keywords and internal email domains. It also runs the optional assessment hook (below) with `pr`. Both results go into one PR comment that is updated in place on every run, and into artifact `uplink-assessment` for import. A failed hook is noted in the comment; it does not fail the check.
  - **Uplink upstream preflight:** applies the change onto public upstream plus declared `Uplink-Depends-On`, then runs `UPLINK_PREFLIGHT`. A failure is commented on the PR; later runs update that comment.
- **Requires:**
  - variables `UPLINK_REDACT_KEYWORDS`, `UPLINK_INTERNAL_DOMAINS`, `UPLINK_PREFLIGHT`;
  - the Actions token (contents read, pull requests write, actions write to run the hook);
  - label `uplink:internal-only`.

## `uplink-import.yml` — Uplink import

- **Runs on:** a merged pull request.
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

- **Runs on:** hourly schedule and manual dispatch.
- **Does:**
  - **Inspect:** `git uplink sync` fetches public upstream without moving `uplink/upstream`. Commits that are our own merged patches apply at once: promote, mark merged, rebuild. Anything else writes `.uplink/reports/from-upstream/incoming.md`.
  - **Wait:** holds on Environment `from-upstream`.
  - **Apply:** after approval, `git uplink accept-upstream` promotes and rebuilds.
  - **Conflict:** a conflict pushes `uplink/conflict/<id>` plus `-work` and opens a gated PR labelled `uplink:conflict`. The run stays green.
- **Requires:**
  - the internal App or PAT (contents, workflows, and pull requests write), which also opens the gated PR;
  - the upstream App or PAT, optional for an `https://` upstream;
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
  - variable `UPLINK_PREFLIGHT`;
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
  - variable `UPLINK_PREFLIGHT`;
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
- **Does:** job **Uplink gate** fails if conflict markers remain or if the PR changes pack files (`uplink-*.yml`, `install-git-uplink`). For conflict PRs it also runs the upstream assessment on the resolution with the patch's stored message, so a resolution that would leak company text cannot merge. For transfer-to-upstream PRs it runs export preflight. For amend PRs it assesses the whole amended patch with the PR title and description as its message.
- **Requires:**
  - variable `UPLINK_PREFLIGHT`;
  - the Actions token (contents read);
  - make **Uplink gate** a required check on the gated bases. Before this job had a name its check was called `validate`; update existing rulesets or branch protection to the new name.

## Assessment hook

- **Optional and company-owned.** The real hook lives on the orphan branch `uplink/hooks`, which is never queued, replayed, or contributed.
- **Placeholder in the pack:** `.github/workflows/uplink-assessment-hook.yml` on `main`. GitHub only dispatches a workflow whose file exists on the default branch, so submit needs this file there. Run it by hand: it prints the setup guide, creates `uplink/hooks` if it is missing, and keeps `assessment-hook.md` there current.
- **Runs on:** dispatch with `--ref uplink/hooks` by the PR checks (input `pr`) and by submit's extras job (input `patch`) when no current result is stored. `caller_run_id` must appear in `run-name`. If `uplink/hooks` has no `.github/workflows/uplink-assessment-hook.yml`, callers skip the hook.
- **Does:** whatever company scans must reach the IP packet. It uploads `*.md` files as artifact `uplink-packet-extra`. The PR comment shows them, import stores them with the patch, and the packet prepends them to `assessment.md` before IP is asked. A failed hook fails neither the PR check nor submit: the run shows a warning, and the comment or `assessment.md` starts with a note that links to the failed hook run. A failed result is never stored.
- **Requires:**
  - `.github/workflows/uplink-assessment-hook.yml` on `uplink/hooks`. Adding it needs workflows write;
  - it must not push `uplink/state`;
  - with `pr`, the checked-out code is unmerged. Do not run it with secrets;
  - recommended: import [`uplink-hooks-ruleset.json`](../github/uplink-hooks-ruleset.json), so changes to `uplink/hooks` need a reviewed pull request. Its code runs during every submit. GitHub Actions bypasses, so the placeholder can update `assessment-hook.md`.
- Guide and starter YAML: [`uplink-assessment-hook.md`](../github/uplink-assessment-hook.md). Walkthrough: [story 07](../../examples/github/stories/07-assessment-hook.md).
