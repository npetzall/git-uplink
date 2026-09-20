# Forge packs

`git uplink init --upstream <url> --contrib <url> --forge <forge>` installs the selected pack as the dedicated tooling patch on company `main`. `--upgrade` refreshes that patch from the templates embedded in the binary.

| `--forge` | Workflows | Notes |
| --- | --- | --- |
| `ghec` | `templates/ghec/.github/workflows/` | GitHub Enterprise Cloud. `git-uplink` on `PATH`. Empty `UPLINK_*_AUTH` defaults to `app`. |
| `example-github` | `templates/example-github/.github/` | Worked example. Includes `install-git-uplink`. Empty `UPLINK_*_AUTH` defaults to `pat`. |

Both are GitHub-family forges. They share one pull request template: `templates/github/pull_request_template.md` → `.github/pull_request_template.md`. Do not copy YAML by hand.

The PR template is the commit message. Title + body become one stored message (HTML comments stripped). Keep the cutoff line; company `main` includes it, contrib export does not.

The `ghec` pack needs the `git-uplink` binary on `PATH`. Install this crate on the runner (`cargo install --path vendor/git-uplink` or a release binary). `example-github` builds it on the runner from `UPLINK_SRC` / `UPLINK_REV`.

Each job’s first `git uplink` command is `git uplink init`, which fetches `origin` `uplink/state` and `uplink/upstream` and adds the `upstream` and `contrib` remotes from URLs stored in `.uplink/queue.json`. That hydrate path does not rewrite workflows. First-time setup is `git uplink init --upstream <url> --contrib <url> --forge ghec` in the product clone (then push `main` and `uplink/state`). Pushing `.github/workflows` needs **workflows** write, not `GITHUB_TOKEN`.

Sync and resolve mint `UPLINK_INTERNAL_TOKEN` first and pass it to `actions/checkout` with `persist-credentials: true`, so shell `git fetch` / `git push` of **origin** can include `.github/workflows` (GitHub rejects `GITHUB_TOKEN` for those files). The submit packet job also persists credentials (`GITHUB_TOKEN`) for its shell `git push` of `uplink/state`. Prepare, preflight, import, and the submit job set `persist-credentials: false`; `git uplink` blanks the checkout extraheader and authenticates by remote (`UPLINK_INTERNAL_*` for origin, `UPLINK_UPSTREAM_*` for public upstream fetch, `UPLINK_CONTRIB_*` for contrib force-push). `GITHUB_TOKEN` is `GH_TOKEN` for `gh` on the company repo (issues, PR comments, dispatching submit). It cannot open the public pull request. Import/sync/resolve/submit mint an App token when the matching `UPLINK_*_AUTH` is empty or `app`. The internal App or PAT needs **contents** and **workflows** write.

## Workflows

- **Prepare** (`uplink-prepare.yml`) — on every PR to `main`. Uses the PR title and body as the single commit message, strips HTML comments, keeps the cutoff on company main, rewrites author, scans for company keywords / internal emails, writes `GITHUB_STEP_SUMMARY`, and the workflow posts the report with `gh pr comment`. Required check.
- **Preflight** (`uplink-preflight.yml`) — required check on PRs to `main` except `uplink:internal-only`. Applies the PR onto public upstream plus `Uplink-Depends-On` lines, then onto tooling + queued upstream (internal omitted), then runs `UPLINK_PREFLIGHT`. Failure prints a comment body; the workflow posts it with `gh pr comment`. Do not merge until it is green.
- **Import** (`uplink-import.yml`) — internal product approval. Merge the PR after engineering review. Internal-only import records the patch on `uplink/state` and leaves `main` at the merge tree. Upstream import records the patch, rebuilds `tooling → upstream[] → internal[]`, and publishes rewritten `main` plus `uplink/state`. Export preflight must still pass for upstream-bound PRs.
- **Sync** (`uplink-sync.yml`) — hourly / manual. Fetches public upstream without moving `uplink/upstream` until inbound review. Commits that match a company patch (trailer / `patch-id`) apply immediately. Any unmatched commit writes `.uplink/reports/from-upstream/incoming.md` and waits on Environment **`from-upstream`**; after approval, `git uplink accept-upstream` promotes `uplink/upstream` and rebuilds `main`. Queue commits are fast-forwards on `uplink/state`. If a patch does not apply, it records `conflict` on the queue (without moving product files), pushes `uplink/conflict/<id>`, and emits issue JSON. The workflow runs `gh issue create` then `git uplink conflicted`. Do not open a PR; resolve the patch on that branch instead.
- **Resolve** (`uplink-resolve.yml`) — on human pushes to `uplink/conflict/<id>` (skips `Uplink Bot` authors and `github-actions[bot]`). Runs `git uplink resolve`, rebuilds `main`, emits `gh issue close` JSON (or a new issue create if rebuild stops later), and deletes the conflict branch. If the resolved patch was already submitted, status becomes `amended` and this workflow dispatches **Uplink submit** so IP can approve the delta. It does not itself push the contribution fork.
- **Submit** (`uplink-submit.yml`) — IP / contribution approval via the **`to-upstream` GitHub Environment**. Dispatch with a patch id (operators, or automatically after resolve of a submitted patch). The packet job commits the report (full contribution, or a delta-first packet when status is `amended`); environment reviewers approve; the same run then `git uplink approve` + `git uplink submit` (contrib git push) + `gh pr create` + `git uplink submitted` (records the PR and pushes `uplink/state`). If `upstream.pr_number` is already stored, the workflow reuses that URL and does not open a second PR. Preflight runs again; a failing build/test means no fork push and no public PR.

Repo variables:

| Variable | Purpose |
| --- | --- |
| `UPLINK_PREFLIGHT` | Product build/test command on the export tree (example: `npm test`) |
| `UPLINK_REDACT_KEYWORDS` | Comma-separated words that must not appear in a contribution (company name, product aliases) |
| `UPLINK_INTERNAL_DOMAINS` | Comma-separated email domains flagged in the export diff (example: `acme.com`) |
| `UPLINK_EXPORT_AUTHOR` | Default public identity `Name <email>` for contribution commits (machine user). Override per change with `Uplink-Export-Author` below the cutoff. |
| `UPLINK_INTERNAL_AUTH` | `pat` (token) or `app` (mint installation token). Empty defaults to `app`. |
| `UPLINK_UPSTREAM_AUTH` | Same models for public upstream fetch. Empty defaults to `app`. |
| `UPLINK_CONTRIB_AUTH` | Same models for contrib force-push. Empty defaults to `app`. |

Import and resolve share the Actions concurrency group `uplink-mutate` at workflow level. Sync uses workflow group **`uplink-sync`** so a waiting `from-upstream` review does not stack hourly runs, and job-level `uplink-mutate` on inspect/apply so that wait does not freeze imports. Submit uses `uplink-mutate` **per job** (packet, then submit) so IP’s environment wait does not freeze imports. The CLI retries a rejected fast-forward of `uplink/state` if another import landed first.

---

## to-upstream environment (contribution / IP gate)

Yes: on GitHub Enterprise Cloud, contribution approval should be a **GitHub Environment**, not a second homegrown checkbox. Name it `to-upstream`. Required reviewers are IP/legal. After they approve the waiting deployment, the same workflow records the receipt and submits to the public contribution fork.

This is the documented option. Use it instead of asking an operator to run `git uplink approve` by hand.

### What the reviewer sees

1. **Job summary** — the packet job appends the contribution packet to `GITHUB_STEP_SUMMARY` (company and upstream commit messages, export author, leak checks, depends-on). For an `amended` patch the packet **leads with the delta** since the last approval and includes historical packets marked already approved. Open the workflow run; the summary is on the completed packet job.
2. **Committed report** — `.uplink/reports/<id>/prepare.md` on `uplink/state`. The `to-upstream` deployment URL points at that file. Reports live on the orphan branch, so a later product rebuild does not drop them.
3. **Environment review UI** — GitHub pauses the submit job until a required reviewer approves the `to-upstream` deployment. That click is the IP gate.

After approval, the submit job writes `.uplink/reports/<id>/approval.md` (in-repo receipt), runs `git uplink approve`, `git uplink submit` (contrib git push), `gh pr create`, then `git uplink submitted` (records the PR and pushes `uplink/state`). The public contrib App token is minted in this job only (`UPLINK_CONTRIB_AUTH=app`).

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
5. **Credentials** — three isolated roles. Workflows use a PAT (`UPLINK_*_TOKEN`) or mint an App installation token (`UPLINK_*_AUTH=app`). `gh pr create` uses the contrib TOKEN.

**Repository secrets** (sync/import/resolve must not wait on `to-upstream` or `from-upstream`):

| Secret | Purpose |
| --- | --- |
| `UPLINK_INTERNAL_TOKEN` | Origin fetch/push (force-push `main`, `uplink/state`, workflow files) when `UPLINK_INTERNAL_AUTH=pat`. Needs contents + workflows write |
| `UPLINK_INTERNAL_APP_ID` / `UPLINK_INTERNAL_APP_PRIVATE_KEY` | When `UPLINK_INTERNAL_AUTH=app`. Install with contents + workflows write |
| `UPLINK_UPSTREAM_TOKEN` | Authenticated `git fetch` of public upstream (rate limits) when `UPLINK_UPSTREAM_AUTH=pat` |
| `UPLINK_UPSTREAM_APP_ID` / `UPLINK_UPSTREAM_APP_PRIVATE_KEY` | When `UPLINK_UPSTREAM_AUTH=app` |
| `UPLINK_UPSTREAM_OWNER` | Public GitHub org that owns the parent (and usually the contribution fork) |
| `UPLINK_UPSTREAM_REPO` | Public parent repository name (required for upstream `app` mint) |

Scope upstream as **read-only** on the public parent (contents: read). Do not give this role contrib write.

**Environment `to-upstream` secrets** (fork write + public PR). Do not also store these at repo or org level:

| Secret | Purpose |
| --- | --- |
| `UPLINK_CONTRIB_TOKEN` | Contrib force-push and `GH_TOKEN` for `gh pr create` when `UPLINK_CONTRIB_AUTH=pat` |
| `UPLINK_CONTRIB_APP_ID` | GitHub App id (registered on public github.com) |
| `UPLINK_CONTRIB_APP_PRIVATE_KEY` | App private key |
| `UPLINK_CONTRIB_REPO` | Repository name of the public contribution fork |

The contrib App must be installed on the contribution fork (contents: write) and on the public parent (pull requests: write, contents: read). Do not register that App from an EMU account if the App would then be enterprise-scoped and unable to see public github.com repositories.

`uplink-sync.yml` fetches public `upstream` over git with `UPLINK_UPSTREAM_*` and classifies new commits against the queue (trailer / patch-id). Flow-back of our patches updates `uplink/upstream` immediately. Foreign commits wait on Environment **`from-upstream`** (review gate only; do not put `UPLINK_INTERNAL_*` there). It does **not** mint the contrib write App. If hourly sync later treats the GitHub PR as merged via the API, that call uses `UPLINK_UPSTREAM_TOKEN` (read on the public parent; same token as authenticated `git fetch`).

The company-repo `GITHUB_TOKEN` (on GHEC EMU, the enterprise token) is still used for `gh issue create` / `gh pr comment` on the company repo. Sync and resolve shell origin git uses the persisted internal token; the submit packet job persists `GITHUB_TOKEN` for its shell `git push` of `uplink/state`. Prepare, preflight, import, and the submit job do not persist checkout credentials. It cannot open the public pull request. `git uplink` does not use it as git transport.

### Ruleset so reports can be committed

The packet job **fast-forwards** a commit of `.uplink/reports/<id>/prepare.md` on `uplink/state` (not a force-push). Import records the patch on `uplink/state` after the PR merge. Internal-only import leaves `main` at the merge tree. Upstream import rebuilds `tooling → upstream[] → internal[]` and publishes rewritten `main`. Sync force-updates `main` when an upstream rebuild is required. Allow **GitHub Actions** (`GITHUB_TOKEN` on the submit packet job) and the **internal** Uplink bot (`UPLINK_INTERNAL_*` for `git uplink` origin transport, and for sync/resolve shell origin push) to push:

- Humans still require a pull request to `main`.
- Actions may bypass to fast-forward `uplink/state`, and to force-push `main` when upstream (or drop/resolve) requires a replay.

If the ruleset blocks those credentials from pushing `uplink/state`, the packet job fails before anyone is asked to approve `to-upstream`.

### Operator flow

```text
queued patch on uplink/state (PR already merged to company main)
        │
        ▼
workflow_dispatch Uplink submit (patch_id)
        │
        ▼
packet job: git uplink report → STEP_SUMMARY + commit prepare.md
        │
        ▼
submit job waits on environment to-upstream   ← IP/legal reviews packet
        │  (Deployments + enterprise audit log)
        ▼
approval.md committed; git uplink approve; git uplink submit
        │
        ▼
gh pr create (or reuse existing URL); git uplink submitted
        │
        ▼
fork branch + public PR (first bytes leaving the private forge)

If a submitted patch later conflicts, resolve sets amended and
dispatches this workflow again. The packet leads with the delta;
historical prepare.md is read from the prior approval SHA on
uplink/state. The workflow skips creating a PR when pr_number is stored.
```

Local equivalent when you are not on Actions (engine tests, a break-glass operator):

```bash
git uplink report upl_…
git uplink approve upl_…
git uplink submit upl_…
gh pr create …   # from submit JSON
git uplink submitted upl_… --pr-url <url>
```

That still writes the same markdown under `.uplink/reports/`. It does **not** create a GitHub Environment review. On GHEC, use the workflow.

---

## from-upstream environment (inbound public main)

Hourly sync must not silently take unrelated upstream commits onto company `main`. Create a second repository Environment named **`from-upstream`**. Required reviewers are whoever should review inbound public changes (security / engineering). Do **not** put `UPLINK_INTERNAL_*` or contrib secrets on this environment: inspect and import must not wait, and this gate is review-only. The contrib write App stays on **`to-upstream`**.

The **Uplink sync** workflow:

1. **Inspect job** (no environment). Runs `git uplink sync`, which fetches public `main` without moving `uplink/upstream`. Commits that match a company patch (`Uplink-Patch-Id` trailer or `git patch-id --stable`) apply immediately: promote `uplink/upstream`, mark those patches `merged`, rebuild company `main`. If every new commit is ours (or nothing moved), that is the whole run.
2. If any commit does not match a company patch, inspect writes `.uplink/reports/from-upstream/incoming.md` (foreign `git show`, plus which patches flowed back), appends `GITHUB_STEP_SUMMARY`, and fast-forwards `uplink/state` only. It does not push `uplink/upstream` or `main`.
3. **Apply job** (`environment: from-upstream`). GitHub holds the job until a required reviewer approves the deployment. The deployment URL points at `incoming.md` on `uplink/state`. After approval the same run writes `approval.md` and runs `git uplink accept-upstream`, then pushes `uplink/upstream` and rebuilds `main`. Patch apply conflicts are recorded the same way as an auto-apply (conflict branch + issue).

Workflow concurrency group `uplink-sync` (`cancel-in-progress: false`) keeps one inbound review at a time so hourly cron does not stack deployments. Inspect and apply still take `uplink-mutate` **per job**, so the environment wait does not freeze imports.

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

