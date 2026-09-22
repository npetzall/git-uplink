# Forge packs

`git uplink init --upstream <url> --contrib <url> --forge <forge>` installs the selected pack as the dedicated tooling patch on company `main`. `--upgrade` refreshes that patch from the templates embedded in the binary.

| `--forge` | Workflows | Notes |
| --- | --- | --- |
| `ghec` | `templates/ghec/.github/workflows/` | GitHub Enterprise Cloud. `git-uplink` on `PATH`. Empty `UPLINK_*_AUTH` defaults to `app`. |
| `example-github` | `templates/example-github/.github/` | Worked example. Includes `install-git-uplink`. Empty `UPLINK_*_AUTH` defaults to `pat`. |

Both are GitHub-family forges. They share one pull request template: `templates/github/pull_request_template.md` → `.github/pull_request_template.md`. Do not copy YAML by hand.

The PR template is the commit message. Title + body become one stored message (HTML comments stripped). Keep the cutoff line; company `main` includes it, contrib export does not.

The `ghec` pack needs the `git-uplink` binary on `PATH`. Install this crate on the runner (`cargo install --path vendor/git-uplink` or a release binary). `example-github` builds it on the runner from `UPLINK_SRC` / `UPLINK_REV`.

Each job’s first `git uplink` command is `git uplink init`, which fetches `origin` `uplink/state`, `uplink/upstream`, and the configured company branch, materializes that local ref without checking it out, and adds the `upstream` and `contrib` remotes from URLs stored in `.uplink/queue.json`. That hydrate path does not rewrite workflows. First-time setup is `git uplink init --upstream <url> --contrib <url> --forge ghec` in the product clone (then push `main` and `uplink/state`). Pushing `.github/workflows` needs **workflows** write, not `GITHUB_TOKEN`.

Sync, resolve, and transfer mint the internal PAT or App and pass it to `actions/checkout` with `persist-credentials: true`, so shell `git fetch` / `git push` of **origin** can include `.github/workflows` (GitHub rejects `GITHUB_TOKEN` for those files). Those jobs also set `UPLINK_INTERNAL_TOKEN` to that same bot token, because `git uplink` blanks the checkout extraheader and authenticates by remote (`UPLINK_INTERNAL_*` for origin, `UPLINK_UPSTREAM_*` for public upstream fetch, `UPLINK_CONTRIB_*` for contrib force-push). Gate, resolve, and transfer **closed-PR** jobs use `pull_request_target` so GitHub loads those sidecars from the default branch; the gated tree is checked out as data (`head.sha` / `base.ref`), not as the workflow file.

PR checks (`uplink-pr.yml`), the gate job, and submit copy `secrets.GITHUB_TOKEN` into `UPLINK_INTERNAL_TOKEN`. That covers origin reads (init, assess, preflight) and fast-forwards of `uplink/state` (submit packet, finalize, and `git uplink submitted`). The binary does not read `GITHUB_TOKEN`; the workflow assigns it. The gate shell `git fetch` of the protected base uses that token as an `http.<server>/.extraheader` for one command. Checkout on PR checks, gate, import, and the submit job stays `persist-credentials: false`. Packet and finalize keep `persist-credentials: true` with the default Actions token so their shell `git push` of `uplink/state` can authenticate.

Import keeps the internal PAT or App: `git uplink rebuild --push` force-pushes `main`, which can include workflow files. The submit job uses `UPLINK_CONTRIB_*` for the contrib force-push and `UPLINK_UPSTREAM_*` as `GH_TOKEN` for `gh pr create --no-maintainer-edit`. `GITHUB_TOKEN` is also `GH_TOKEN` for `gh` on the company repo (PR comments, dispatching the assessment hook). Company-repo `gh` that runs after `git uplink init` sets `GH_REPO` to the company repository, because init adds the `upstream` remote and `gh` would otherwise query that public parent. It cannot open the public pull request. Import, sync, resolve, and transfer mint an App token when the matching `UPLINK_*_AUTH` is empty or `app`. The internal App or PAT needs **contents** and **workflows** write. Submit and abandon mint the contrib App (fork contents write) and the upstream App (contents read and pull requests write on the parent).

## Workflows

- **PR checks** (`uplink-pr.yml`) — on every PR to `main` except `uplink:internal-only`. Two parallel jobs, neither on `uplink-mutate`:
  - **Uplink upstream assess** — PR title and body as the commit message, HTML comments stripped, cutoff kept on company main, author rewritten, scan for company keywords / internal emails, `GITHUB_STEP_SUMMARY`, and `gh pr comment`. Required check.
  - **Uplink upstream preflight** — apply onto public upstream plus `Uplink-Depends-On` lines, then tooling + queued upstream (internal omitted), then `UPLINK_PREFLIGHT`. Failure prints a comment body; the workflow posts it with `gh pr comment`. Required check. Do not merge until both jobs are green.
- **Import** (`uplink-import.yml`) — internal product approval. Merge the PR after engineering review. Internal-only import records the patch on `uplink/state` and leaves `main` at the merge tree. Upstream import records the patch, rebuilds `tooling → upstream[] → internal[]`, and publishes rewritten `main` plus `uplink/state`. Export preflight must still pass for upstream-bound PRs.
- **Sync** (`uplink-sync.yml`) — hourly / manual. Fetches public upstream without moving `uplink/upstream` until inbound review. Commits that match a company patch (trailer / `patch-id`) apply immediately. Any unmatched commit writes `.uplink/reports/from-upstream/incoming.md` and waits on Environment **`from-upstream`**; after approval, `git uplink accept-upstream` promotes `uplink/upstream` and rebuilds `main`. Queue commits are fast-forwards on `uplink/state`. If a patch does not apply, it records `conflict` on the queue (without moving product files), pushes `uplink/conflict/<id>` (protected base) and `<id>-work`, and emits `gh.prCreate` JSON. The workflow opens the gated PR then `git uplink gated`. That run stays **green**; company `main` is frozen until the PR is merged (`git uplink status` / open `uplink:conflict` PRs).
- **Resolve** (`uplink-resolve.yml`) — when the gated conflict PR is **merged** into `uplink/conflict/<id>`. `pull_request_target` loads this sidecar from the default branch (not `*-work`). Runs `git uplink resolve`, rebuilds `main`, deletes the base and `-work` branches, and if rebuild stops later publishes the next conflict PR (job stays green). If the resolved patch was already submitted, status becomes `amended`; this workflow cancels any waiting or in-progress **Uplink submit** run whose run-name is exactly `Uplink submit <id>` and dispatches a new one for the delta packet.
- **Transfer** (`uplink-transfer.yml`) — dispatch from `main` with a patch id and `to-upstream` / `to-internal`. `git uplink transfer` moves the patch immediately when apply, assess (for `--to-upstream`), and preflight pass. `--to-internal` refuses while another **active** patch still on `upstream[]` lists this id in `dependsOn` (move those dependents `--to-internal` first). If bytes must change, it pushes `uplink/transfer-to-*/<id>` plus `-work` and the workflow opens a PR (queue unchanged). Merging runs `--complete` (`pull_request_target`, YAML from the default branch). Closing the PR without merging deletes both branches. Successful `--to-internal` of a submitted patch dispatches **Uplink abandon contrib** and does not wait.
- **Abandon contrib** (`uplink-abandon.yml`) — `workflow_dispatch` from `main`, Environment **`abandon-contrib`**. Closes the public PR with `UPLINK_UPSTREAM_*` and deletes `uplink/<id>` on the contrib fork with `UPLINK_CONTRIB_*`. No `uplink-mutate`. Until this environment is approved, company state is already internal-only while the public leftover remains.
- **Gate** (`uplink-gate.yml`) — required check on PRs into the three protected bases (`pull_request_target`, YAML from the default branch). Fails if conflict markers remain, or if the PR changes pack files (`uplink-*.yml`, `install-git-uplink`). Product workflows (`ci.yml`, and so on) may still change. Transfer-to-upstream PRs also run export preflight / `UPLINK_PREFLIGHT`. These PRs are not imports to `main`.
- **Submit** (`uplink-submit.yml`) — IP / contribution approval via the **`to-upstream` GitHub Environment**. Dispatch with a patch id (operators, or automatically after resolve of a submitted patch). The packet job commits `assessment.md` (full contribution, or a delta-first packet when status is `amended`). Finalize optionally dispatches company `.github/workflows/uplink-assessment-hook.yml`, downloads artifact `uplink-packet-extra` from that run, and prepends those markdown files onto the packet. Environment reviewers then approve; the same run `git uplink approve` + `git uplink submit` (contrib git push) + `gh pr create --no-maintainer-edit` + `git uplink submitted` (records the PR and pushes `uplink/state`). If `upstream.pr_number` is already stored, the workflow reuses that URL and does not open a second PR. Preflight runs again; a failing build/test means no fork push and no public PR. Do not edit this workflow to add company scans — add the assessment hook instead.

Repo variables:

| Variable | Purpose |
| --- | --- |
| `UPLINK_PREFLIGHT` | Product build/test command on the export tree (example: `npm test`) |
| `UPLINK_REDACT_KEYWORDS` | Comma-separated words that must not appear in a contribution (company name, product aliases) |
| `UPLINK_INTERNAL_DOMAINS` | Comma-separated email domains flagged in the export diff (example: `acme.com`) |
| `UPLINK_EXPORT_AUTHOR` | Default public identity `Name <email>` for contribution commits (machine user). Override per change with `Uplink-Export-Author` below the cutoff. |
| `UPLINK_INTERNAL_AUTH` | `pat` (token) or `app` (mint installation token). Empty defaults to `app`. |
| `UPLINK_UPSTREAM_AUTH` | Same models for public upstream fetch and for opening or closing the public PR. Empty defaults to `app`. |
| `UPLINK_CONTRIB_AUTH` | Same models for contrib force-push and fork branch delete. Empty defaults to `app`. |

Import, resolve, and transfer share the Actions concurrency group `uplink-mutate` at workflow or job level. A GitHub Environment job that is **Waiting** for required reviewers still occupies its concurrency group, so wait jobs must not sit on `uplink-mutate`. Sync uses workflow group **`uplink-sync`** so a waiting `from-upstream` review does not stack hourly runs. Inspect takes `uplink-mutate`; a separate wait job holds `from-upstream` with no mutate slot; apply takes `uplink-mutate` only after approval. Submit takes `uplink-mutate` on packet and finalize only; the `to-upstream` export job keeps the environment (contrib secrets) and does **not** take the group, so IP review does not freeze import/resolve. `git uplink submitted` retries a rejected `uplink/state` push by replaying the PR fields onto the latest origin queue (not the import restack, which would drop updates to an existing id). The CLI also retries a rejected fast-forward of `uplink/state` if another import landed first.

---

## to-upstream environment (contribution / IP gate)

Yes: on GitHub Enterprise Cloud, contribution approval should be a **GitHub Environment**, not a second homegrown checkbox. Name it `to-upstream`. Required reviewers are IP/legal. After they approve the waiting deployment, the same workflow records the receipt and submits to the public contribution fork.

This is the documented option. Use it instead of asking an operator to run `git uplink approve` by hand.

### What the reviewer sees

1. **Job summary** — the packet job appends the contribution packet to `GITHUB_STEP_SUMMARY` (company and upstream commit messages, export author, leak checks, depends-on). For an `amended` patch the packet **leads with the delta** since the last approval and includes historical packets marked already approved. Open the workflow run; the summary is on the completed packet job.
2. **Committed report** — `.uplink/reports/<id>/assessment.md` on `uplink/state`. The `to-upstream` deployment URL points at that file. Reports live on the orphan branch, so a later product rebuild does not drop them.
3. **Environment review UI** — GitHub pauses the submit job until a required reviewer approves the `to-upstream` deployment. That click is the IP gate.

After approval, the submit job writes `.uplink/reports/<id>/approval.md` (in-repo receipt), runs `git uplink approve`, `git uplink submit` (contrib git push), `gh pr create --no-maintainer-edit`, then `git uplink submitted` (records the PR and pushes `uplink/state` with `UPLINK_INTERNAL_TOKEN` set to `secrets.GITHUB_TOKEN`). This job mints the contrib App (fork push) and the upstream App (public PR). Maintainer edits stay off: the upstream credential has no read on the fork, and a maintainer commit on the fork branch would not come back onto the queue.

### Why the GitHub audit log is the source of truth

The dispatcher (`workflow_dispatch` actor) is **not** the IP approver. GitHub records the environment reviewer on:

- the repository **Deployments** tab for environment `to-upstream`
- the **GitHub Enterprise Cloud audit log** (deployment review events)

`approval.md` is a human-readable receipt that points at the run URL. It does not replace the audit log. Enable **Prevent self-review** on the environment so the person who dispatched cannot approve their own export.

### Create the environment

In the company product repo (private forge; GHEC EMU is one implementation):

1. Settings → Environments → New environment → name **`to-upstream`** (exact name; the workflow references `environment: to-upstream`).
2. **Required reviewers** — add the IP/legal team (or named reviewers). Turn on **Prevent self-review**.
3. **Deployment branches** — restrict to `main` so a dispatch from another ref cannot export.
4. Optional wait timer if policy wants a cooling-off period.
5. **Credentials** — three isolated roles. Workflows use a PAT (`UPLINK_*_TOKEN`) or mint an App installation token (`UPLINK_*_AUTH=app`). `gh pr create --no-maintainer-edit` uses the upstream TOKEN (repository secret). The contrib TOKEN only force-pushes the fork.

**Repository secrets** (sync/import/resolve must not wait on `to-upstream` or `from-upstream`):

| Secret | Purpose |
| --- | --- |
| `UPLINK_INTERNAL_TOKEN` | Product origin push (force-push `main`, conflict and transfer branches, workflow files) on import, sync, resolve, and transfer when `UPLINK_INTERNAL_AUTH=pat`. Needs contents + workflows write. PR checks, gate, and submit state writes use the Actions `GITHUB_TOKEN` instead |
| `UPLINK_INTERNAL_APP_ID` / `UPLINK_INTERNAL_APP_PRIVATE_KEY` | When `UPLINK_INTERNAL_AUTH=app`. Install with contents + workflows write |
| `UPLINK_UPSTREAM_TOKEN` | Authenticated `git fetch` of public upstream, and `GH_TOKEN` for `gh pr create --no-maintainer-edit` / `gh pr close`, when `UPLINK_UPSTREAM_AUTH=pat`. Contents read and pull requests write on the parent. No access to the fork |
| `UPLINK_UPSTREAM_APP_ID` / `UPLINK_UPSTREAM_APP_PRIVATE_KEY` | When `UPLINK_UPSTREAM_AUTH=app`. Install on the parent with contents read and pull requests write. Fetch jobs mint contents read only; submit and abandon also request pull requests write |
| `UPLINK_UPSTREAM_OWNER` | Account that owns the public parent (required for the upstream App mint). Do not copy this onto an environment: an environment value would hide the parent owner inside submit and abandon |
| `UPLINK_UPSTREAM_REPO` | Public parent repository name (required for upstream `app` mint) |

Do not give the upstream role contents write, and do not give it access to the contribution fork.

**Environment `to-upstream` secrets** (fork write only). Do not also store these at repo or org level:

| Secret | Purpose |
| --- | --- |
| `UPLINK_CONTRIB_TOKEN` | Contrib force-push when `UPLINK_CONTRIB_AUTH=pat`. Not used for `gh pr create` |
| `UPLINK_CONTRIB_APP_ID` | GitHub App id (registered on public github.com) |
| `UPLINK_CONTRIB_APP_PRIVATE_KEY` | App private key |
| `UPLINK_CONTRIB_OWNER` | Account that owns the contribution fork. Omit when that account is `UPLINK_UPSTREAM_OWNER`; the workflow falls back |
| `UPLINK_CONTRIB_REPO` | Repository name of the public contribution fork |

The contrib App is installed on the contribution fork only (contents: write). Do not register that App from an EMU account if the App would then be enterprise-scoped and unable to see public github.com repositories.

`uplink-sync.yml` fetches public `upstream` over git with `UPLINK_UPSTREAM_*` and classifies new commits against the queue (trailer / patch-id). Flow-back of our patches updates `uplink/upstream` immediately. Foreign commits wait on Environment **`from-upstream`** (review gate only; do not put `UPLINK_INTERNAL_*` there). It does **not** mint the contrib write App. If hourly sync later treats the GitHub PR as merged via the API, that call uses `UPLINK_UPSTREAM_TOKEN` (contents read and pull requests write on the public parent; the same token as authenticated `git fetch` and `gh pr create`).

The company-repo `GITHUB_TOKEN` (on GHEC EMU, the enterprise token) is `GH_TOKEN` for `gh pr comment` on the company repo and for dispatching the assessment hook. PR checks, gate, and submit copy it into `UPLINK_INTERNAL_TOKEN` for origin reads and fast-forwards of `uplink/state`. Sync, resolve, and transfer shell origin git uses the persisted internal PAT or App. Packet and finalize persist `GITHUB_TOKEN` for their shell `git push` of `uplink/state`. Import and the submit job do not persist checkout credentials. The Actions token cannot open the public pull request. `git uplink` does not read `GITHUB_TOKEN` unless a workflow assigns it to `UPLINK_INTERNAL_TOKEN`.

### Ruleset so reports can be committed

The packet job **fast-forwards** a commit of `.uplink/reports/<id>/assessment.md` on `uplink/state` (not a force-push). Import records the patch on `uplink/state` after the PR merge. Internal-only import leaves `main` at the merge tree. Upstream import rebuilds `tooling → upstream[] → internal[]` and publishes rewritten `main`. Sync force-updates `main` when an upstream rebuild is required. Allow **GitHub Actions** (`GITHUB_TOKEN` on submit packet, finalize, and `git uplink submitted`) to fast-forward `uplink/state`. Allow the **internal** Uplink bot (`UPLINK_INTERNAL_*` on import, sync, resolve, and transfer, including shell origin git) to force-push `main` and the gated branches:

- Humans still require a pull request to `main`.
- Actions may bypass to fast-forward `uplink/state`. The internal bot force-pushes `main` when import rebuild, sync, resolve, or transfer requires a replay.
- A second ruleset (or the same one with extra patterns) protects **`uplink/conflict/*`**, **`uplink/transfer-to-upstream/*`**, and **`uplink/transfer-to-internal/*`**, excluding `*-work`. Require a pull request before merging; do not allow direct pushes. Require the **Uplink gate** check. Bypass for the **internal** Uplink App/PAT and GitHub Actions is only to **create and delete** those protected bases, not to land human commits on them. Work branches (`*-work`) stay unprotected so authors can push.
- A third ruleset, **including** `*-work`, restricts only Uplink pack paths: `.github/workflows/uplink-*.yml` and `.github/actions/install-git-uplink/**`. Product workflows stay editable on gated PRs. Bypass for the **internal** Uplink App/PAT and GitHub Actions so sync/transfer/resolve can still create those branches (the snapshot contains the pack). Sample: [`templates/github/uplink-pack-files-ruleset.json`](github/uplink-pack-files-ruleset.json) (`gh api repos/OWNER/REPO/rulesets --method POST --input .github/uplink-pack-files-ruleset.json`). Add a `bypass_actors` entry for the internal Uplink GitHub App (`actor_type: Integration`, `actor_id` = App ID). If `UPLINK_INTERNAL_AUTH=pat`, bypass the bot account that owns the PAT (or a RepositoryRole that may push). Do **not** restrict `.github/workflows/**` as a whole.

If the ruleset blocks those credentials from pushing `uplink/state`, the packet job fails before anyone is asked to approve `to-upstream`.

### Operator flow

```text
queued patch on uplink/state (PR already merged to company main)
        │
        ▼
workflow_dispatch Uplink submit (patch_id)
        │
        ▼
packet job: git uplink report → STEP_SUMMARY + commit assessment.md
        │
        ▼
finalize: optional uplink-assessment-hook.yml → artifact uplink-packet-extra
        │  extras prepended; skip if the company file is absent
        ▼
submit job waits on environment to-upstream   ← IP/legal reviews packet
        │  (Deployments + enterprise audit log)
        ▼
approval.md committed; git uplink approve; git uplink submit
        │
        ▼
gh pr create --no-maintainer-edit (or reuse existing URL); git uplink submitted
        │
        ▼
fork branch + public PR (first bytes leaving the private forge)

If a submitted patch later conflicts, resolve sets amended and
dispatches this workflow again. The packet leads with the delta;
historical assessment.md is read from the prior approval SHA on
uplink/state. The workflow skips creating a PR when pr_number is stored.
```

Local equivalent when you are not on Actions (engine tests, a break-glass operator):

```bash
git uplink report upl_…
git uplink approve upl_…
git uplink submit upl_…
gh pr create --no-maintainer-edit …   # from submit JSON
git uplink submitted upl_… --pr-url <url>
```

That still writes the same markdown under `.uplink/reports/`. It does **not** create a GitHub Environment review. On GHEC, use the workflow.

---

## abandon-contrib environment (withdraw a public contribution)

`--to-internal` of a submitted patch must close the public PR and delete `uplink/<id>` on the contrib fork. Closing the PR uses the repository upstream credential (pull requests write on the parent). Deleting the branch needs the **same contrib write credentials** as submit. It is not an IP export: reviewers are whoever decided the patch is internal-only (engineering), not legal.

Transfer mutates company `main` / `uplink/state` on `uplink-mutate`, then `gh workflow run "Uplink abandon contrib" --ref main` and **does not wait**. The abandon job waits on Environment **`abandon-contrib`** with no mutate slot.

**Gap:** after transfer succeeds, the queue already says internal-only. The public PR and fork branch stay until `abandon-contrib` is approved. If nobody approves, they stay. That is the control. Do not hide a failed close behind `|| true` on the transfer job.

### Create the environment

In the company product repo:

1. Settings → Environments → New environment → name **`abandon-contrib`** (exact name; the workflow references `environment: abandon-contrib`).
2. **Required reviewers** — engineering / the people who chose internal-only. Different from `to-upstream` (IP/legal). Turn on **Prevent self-review** in production.
3. **Deployment branches** — restrict to `main`.
4. **Secrets** — copy the **same** contrib values as `to-upstream`. GitHub environments do not share secrets. Still **do not** store these at repo or org level:

| Secret | Purpose |
| --- | --- |
| `UPLINK_CONTRIB_TOKEN` | When `UPLINK_CONTRIB_AUTH=pat`. Delete the fork branch. Closing the public PR uses repository `UPLINK_UPSTREAM_TOKEN` |
| `UPLINK_CONTRIB_APP_ID` / `UPLINK_CONTRIB_APP_PRIVATE_KEY` | When `UPLINK_CONTRIB_AUTH=app` |
| `UPLINK_CONTRIB_OWNER` | Fork owner. Omit when it matches repository `UPLINK_UPSTREAM_OWNER` |
| `UPLINK_CONTRIB_REPO` | Contribution fork repository name |

The contrib App is the same installation as submit (fork contents: write only).

`--to-internal` of a patch that was never submitted does not dispatch this workflow (`gh.prClose` is omitted). `--to-upstream` never abandons a public PR.

---

## Assessment hook

Company scans that must enter the IP packet run **after** the packet job and **before** `to-upstream`. Do not edit `uplink-submit.yml`. Add `.github/workflows/uplink-assessment-hook.yml` as an internal-only product patch (`uplink:internal-only` so it never goes upstream). `--upgrade` never touches a file that is not in the forge pack.

Copy-paste starter: [`examples/github/patches/uplink-assessment-hook.yml`](../examples/github/patches/uplink-assessment-hook.yml). Walkthrough: [`examples/github/stories/07-assessment-hook.md`](../examples/github/stories/07-assessment-hook.md). Do not put that file in `.github/workflows/` of the pack — GitHub treats every workflow YAML there as live.

**Contract**

1. Inputs: `patch_id`, `caller_run_id` (submit’s `github.run_id`). Include both in `run-name` so submit can find this run (`workflow_dispatch` does not return a run id).
2. Produce one or more `*.md` files. Upload them as artifact **`uplink-packet-extra`**.
3. Do **not** push `uplink/state`. Submit is the only writer of the committed packet.
4. Failure fails the assessment-hook run. Submit treats that as a failed gate (`gh run watch --exit-status`), so IP is never asked.

Submit’s **finalize** job:

- Skips if `HEAD:.github/workflows/uplink-assessment-hook.yml` is missing.
- Otherwise `gh workflow run uplink-assessment-hook.yml` with those inputs, waits until `displayTitle` contains both ids, then downloads the artifact from **that run id** (`actions/download-artifact` + `GITHUB_TOKEN`; `actions:read` is enough on the same repo). A missing artifact is ignored (the hook ran but uploaded nothing).
- If markdown files exist, `git uplink report <id> --extra-dir <download>` prepends them (name order, skip hidden) **before** `# Contribution packet` / `# Delta packet`, then pushes `uplink/state`.
- Appends the assembled packet to finalize’s `GITHUB_STEP_SUMMARY` (what IP should read). The `to-upstream` deployment URL still points at `assessment.md`.

Assess on the internal PR has no patch id yet. Extra required scans belong in sibling `pull_request` workflows. The assessment hook copies those findings into the artifact (look up the internal PR via `source.internalPrNumber` on the queued patch).

**Why not job outputs.** Outputs exist only inside one run (`needs.*.outputs`), cap at 1 MB per job, and do not survive `workflow_dispatch`. Artifacts do, and they are downloadable from another run by `run-id`.

**Why not `on: workflow_run` for Uplink submit.** That event fires when the *whole* submit run completes — after IP already approved. Too late to inject extras. Resolve’s cancel + re-dispatch is for an *amended* patch, not for assessment-hook extras.

---

## from-upstream environment (inbound public main)


Hourly sync must not silently take unrelated upstream commits onto company `main`. Create a second repository Environment named **`from-upstream`**. Required reviewers are whoever should review inbound public changes (security / engineering). Do **not** put `UPLINK_INTERNAL_*` or contrib secrets on this environment: inspect and import must not wait, and this gate is review-only. The contrib write App stays on **`to-upstream`** (export) and **`abandon-contrib`** (withdraw).

The **Uplink sync** workflow:

1. **Inspect job** (no environment). Runs `git uplink sync`, which fetches public `main` without moving `uplink/upstream`. Commits that match a company patch (`Uplink-Patch-Id` trailer or `git patch-id --stable`) apply immediately: promote `uplink/upstream`, mark those patches `merged`, rebuild company `main`. If every new commit is ours (or nothing moved), that is the whole run.
2. If any commit does not match a company patch, inspect writes `.uplink/reports/from-upstream/incoming.md` (foreign `git show`, plus which patches flowed back), appends `GITHUB_STEP_SUMMARY`, and fast-forwards `uplink/state` only. It does not push `uplink/upstream` or `main`.
3. **Wait job** (`environment: from-upstream`, no `uplink-mutate`). GitHub holds this job until a required reviewer approves the deployment. The deployment URL points at `incoming.md` on `uplink/state`.
4. **Apply job** (`uplink-mutate`, no environment). After approval the same run writes `approval.md` and runs `git uplink accept-upstream`, then pushes `uplink/upstream` and rebuilds `main`. Patch apply conflicts are recorded the same way as an auto-apply (conflict branch + issue).

Workflow concurrency group `uplink-sync` (`cancel-in-progress: false`) keeps one inbound review at a time so hourly cron does not stack deployments. Inspect and apply take `uplink-mutate`; the environment wait does not, so inbound review does not freeze import/resolve.

### Create the environment

1. Settings → Environments → New environment → name **`from-upstream`** (exact name; the workflow references `environment: from-upstream`).
2. **Required reviewers** — add the inbound review team. Turn on **Prevent self-review**.
3. **Deployment branches** — restrict to `main`.
4. No environment secrets.

Local equivalent:

```bash
git uplink sync                 # may print needsApproval and write incoming.md
git uplink accept-upstream      # after you have reviewed the packet
```
