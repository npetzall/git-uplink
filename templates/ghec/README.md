# GitHub Enterprise Cloud workflows (`--forge ghec`)

`git uplink init --forge ghec` installs the workflows in [`.github/workflows/`](.github/workflows/) as the tooling patch on company `main`, together with the shared [pull request template](../github/pull_request_template.md) and the [`install-git-uplink`](../github/actions/install-git-uplink/action.yml) action. `git uplink init --upgrade` refreshes them from the binary. Do not edit them by hand.

For setting up a repository step by step, see [Production setup](https://npetzall.github.io/git-uplink/setup?forge=ghec&view=steps).

Every job installs the `git-uplink` release named by `UPLINK_SRC` / `UPLINK_VERSION`, then runs `git uplink init` to hydrate its checkout from `origin`. Gated pull requests are opened with the internal App or PAT, not `GITHUB_TOKEN`, so the repository setting "Allow GitHub Actions to create and approve pull requests" can stay off, and the Uplink gate check runs as soon as the PR opens. `UPLINK_*_AUTH` selects `app` (mint an installation token from `UPLINK_*_APP_ID` + `UPLINK_*_APP_PRIVATE_KEY`, the default) or `pat` (use `UPLINK_*_TOKEN`). Workflows that write the queue share the concurrency group `uplink-mutate` with `queue: max`, so they run one at a time and none is dropped. Jobs that wait on an Environment never hold that group.

## `uplink-pr.yml` — Uplink PR checks

- **Runs on:** pull requests to `main` (opened, synchronize, reopened, edited). Skipped for `uplink:internal-only`.
- **Does:** two parallel jobs, both required checks.
  - **Uplink upstream assess:** turns the PR title and body into the commit message, strips everything below the cutoff, rewrites the author, scans for company keywords and internal email domains, and comments the report on the PR.
  - **Uplink upstream preflight:** applies the change onto public upstream plus declared `Uplink-Depends-On`, then runs `UPLINK_PREFLIGHT`.
- **Requires:**
  - variables `UPLINK_REDACT_KEYWORDS`, `UPLINK_INTERNAL_DOMAINS`, `UPLINK_EXPORT_AUTHOR`, `UPLINK_PREFLIGHT`;
  - the Actions token (contents read, pull requests write);
  - label `uplink:internal-only`.

## `uplink-import.yml` — Uplink import

- **Runs on:** a merged pull request.
- **Does:** `git uplink add` records the merged change on `uplink/state` as `queued`, in `internal[]` when labelled `uplink:internal-only`, otherwise in `upstream[]`. An upstream import rebuilds `main` so the patch sits under `internal[]`. Then `git uplink push`.
- **Requires:**
  - the internal App or PAT (contents and workflows write), because the rebuild force-pushes `main`, which contains workflow files;
  - concurrency group `uplink-mutate`.

## `uplink-submit.yml` — Uplink submit

- **Runs on:** manual dispatch from `main` with a `patch_id`, and dispatch by resolve for an `amended` patch.
- **Does:**
  1. **Packet:** `git uplink report` writes `.uplink/reports/<id>/assessment.md` on `uplink/state` and to the job summary.
  2. **Finalize:** runs the optional assessment hook (below) and prepends its extras.
  3. **Submit:** waits on Environment `to-upstream`. After approval it records `approval.md`, runs `git uplink approve` and `git uplink submit` (pushes `uplink/<id>` to the fork), opens the public PR (`maintainer_can_modify` false), and runs `git uplink submitted`. If the patch already has a PR number, no second PR is opened.
- **Requires:**
  - Environment `to-upstream` with the IP reviewers and the contrib App or PAT (fork contents write) as environment secrets;
  - repository secrets for the upstream App or PAT (contents read, pull requests write on upstream and the fork);
  - the Actions token (contents write, actions write) for the packet and finalize jobs;
  - `uplink-mutate` on packet and finalize only.

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
  - `git uplink resolve <id>` refreshes the patch, rebuilds `main`, and deletes the base and `-work` branches.
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

## `uplink-abandon.yml` — Uplink abandon contrib

- **Runs on:** dispatch by transfer after `--to-internal` of a submitted patch.
- **Does:** waits on Environment `abandon-contrib`, then closes the public PR and deletes `uplink/<id>` on the fork. Until it is approved, the queue already says internal-only while the public PR remains.
- **Requires:**
  - Environment `abandon-contrib` with engineering reviewers and a copy of the contrib secrets (Environments do not share secrets);
  - the upstream App or PAT (pull requests write) to close the PR.
  - Does not take `uplink-mutate`.

## `uplink-gate.yml` — Uplink gate

- **Runs on:** pull requests into `uplink/conflict/**`, `uplink/transfer-to-upstream/**`, and `uplink/transfer-to-internal/**` (`pull_request_target`).
- **Does:** job **Uplink gate** fails if conflict markers remain or if the PR changes pack files (`uplink-*.yml`, `install-git-uplink`). For transfer-to-upstream PRs it also runs export preflight.
- **Requires:**
  - variable `UPLINK_PREFLIGHT`;
  - the Actions token (contents read);
  - make **Uplink gate** a required check on the gated bases. Before this job had a name its check was called `validate`; update existing rulesets or branch protection to the new name.

## Assessment hook

- **Optional and company-owned.** It is not part of the pack, and `--upgrade` never touches it.
- **Runs on:** dispatch by Uplink submit's finalize job, with inputs `patch_id` and `caller_run_id`. Both must appear in `run-name`.
- **Does:** whatever company scans must reach the IP packet. It uploads `*.md` files as artifact `uplink-packet-extra`, which finalize prepends to `assessment.md` before IP is asked. A failed hook fails submit, so IP is never asked.
- **Requires:**
  - `.github/workflows/uplink-assessment-hook.yml`, added as an `uplink:internal-only` change;
  - it must not push `uplink/state`.
- Starter: [`uplink-assessment-hook.yml`](../../examples/github/patches/uplink-assessment-hook.yml). Walkthrough: [story 07](../../examples/github/stories/07-assessment-hook.md).
