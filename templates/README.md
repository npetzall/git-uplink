# EMU workflow templates

Copy these files into the **company product repository** on GitHub Enterprise Cloud.

- `templates/emu-workflows/*.yml` → `.github/workflows/`
- `templates/github/pull_request_template.md` → `.github/pull_request_template.md`

The PR template is the commit message. Title + body become one stored message (HTML comments stripped). Keep the cutoff line; company `main` includes it, contrib export does not.

That repository also needs the `git-uplink` binary on `PATH`. Install this crate on the runner (`cargo install --path vendor/git-uplink` or a release binary).

`GITHUB_TOKEN` in these workflows is the **EMU** token and only pushes to the company repo. It cannot open the public pull request.

## Two gates

- **Prepare** (`uplink-prepare.yml`) — on every PR to `main`. Uses the PR title and body as the single commit message, strips HTML comments, keeps the cutoff on company main, rewrites author, scans for company keywords / internal emails, writes `GITHUB_STEP_SUMMARY`, and the workflow posts the report with `gh pr comment`. Required check.
- **Preflight** (`uplink-preflight.yml`) — required check on every PR to `main`. Applies the PR onto public upstream plus `Uplink-Depends-On` lines from the body, then runs `UPLINK_PREFLIGHT`. Failure prints a comment body; the workflow posts it with `gh pr comment`. Do not import until it is green.
- **Import** (`uplink-import.yml`) — internal product approval. Label `uplink:import` after engineering review (or merge the PR). The change is recorded on `uplink/state` and applied onto company `main` as status `queued` only if export preflight still passes.
- **Sync** (`uplink-sync.yml`) — hourly / manual. Fetches public upstream, drops merged patches, and rebuilds `main` only when upstream moved. Queue commits are fast-forwards on `uplink/state`. If a patch does not apply, it records `conflict` on the queue (without moving product files), pushes `uplink/conflict/<id>`, and emits issue JSON. The workflow runs `gh issue create` then `git uplink conflicted`. Do not open a PR; resolve the patch on that branch instead.
- **Resolve** (`uplink-resolve.yml`) — on human pushes to `uplink/conflict/<id>` (skips `github-actions[bot]`). Runs `git uplink resolve`, rebuilds `main`, emits `gh issue close` JSON (or a new issue create if rebuild stops later), and deletes the conflict branch. If the resolved patch was already submitted, status becomes `amended` and this workflow dispatches **Uplink submit** so IP can approve the delta. It does not itself push the contribution fork.
- **Submit** (`uplink-submit.yml`) — IP / contribution approval via the **`oss` GitHub Environment**. Dispatch with a patch id (operators, or automatically after resolve of a submitted patch). The packet job commits the report (full contribution, or a delta-first packet when status is `amended`); environment reviewers approve; the same run then `git uplink approve` + `git uplink submit` (contrib git push) + `gh pr create` + `git uplink submitted` (records the PR and pushes `uplink/state`). If `upstream.pr_number` is already stored, the workflow reuses that URL and does not open a second PR. Preflight runs again; a failing build/test means no fork push and no public PR.

Repo variables:

| Variable | Purpose |
| --- | --- |
| `UPLINK_PREFLIGHT` | Product build/test command on the export tree (example: `npm test`) |
| `UPLINK_REDACT_KEYWORDS` | Comma-separated words that must not appear in a contribution (company name, product aliases) |
| `UPLINK_INTERNAL_DOMAINS` | Comma-separated email domains flagged in the export diff (example: `acme.com`) |
| `UPLINK_EXPORT_AUTHOR` | Default public identity `Name <email>` for contribution commits (machine user). Override per change with `Uplink-Export-Author` below the cutoff. |

Import and sync share the Actions concurrency group `uplink-mutate` at workflow level. Resolve uses that group too. Submit uses it **per job** (packet, then submit) so IP’s environment wait does not freeze imports. The CLI retries a rejected fast-forward of `uplink/state` or `main` if another import landed first.

---

## OSS environment (contribution / IP gate)

Yes: on GitHub Enterprise Cloud, contribution approval should be a **GitHub Environment**, not a second homegrown checkbox. Name it `oss`. Required reviewers are IP/legal. After they approve the waiting deployment, the same workflow records the receipt and submits to the upstream-owned private fork.

This is the documented option. Use it instead of asking an operator to run `git uplink approve` by hand.

### What the reviewer sees

1. **Job summary** — the packet job appends the contribution packet to `GITHUB_STEP_SUMMARY` (company and upstream commit messages, export author, leak checks, depends-on). For an `amended` patch the packet **leads with the delta** since the last approval and includes historical packets marked already approved. Open the workflow run; the summary is on the completed packet job.
2. **Committed report** — `.uplink/reports/<id>/prepare.md` on `uplink/state`. The `oss` deployment URL points at that file. Reports live on the orphan branch, so a later product rebuild does not drop them.
3. **Environment review UI** — GitHub pauses the submit job until a required reviewer approves the `oss` deployment. That click is the IP gate.

After approval, the submit job writes `.uplink/reports/<id>/approval.md` (in-repo receipt), runs `git uplink approve`, `git uplink submit` (contrib git push), `gh pr create`, then `git uplink submitted` (records the PR and pushes `uplink/state`). The public GitHub App token is minted in this job only.

### Why the GitHub audit log is the source of truth

The dispatcher (`workflow_dispatch` actor) is **not** the IP approver. GitHub records the environment reviewer on:

- the repository **Deployments** tab for environment `oss`
- the **GitHub Enterprise Cloud audit log** (deployment review events)

`approval.md` is a human-readable receipt that points at the run URL. It does not replace the audit log. Enable **Prevent self-review** on the environment so the person who dispatched cannot approve their own export.

### Create the environment

In the company product repo (EMU):

1. Settings → Environments → New environment → name **`oss`** (exact name; the workflow references `environment: oss`).
2. **Required reviewers** — add the IP/legal team (or named reviewers). Turn on **Prevent self-review**.
3. **Deployment branches** — restrict to `main` so a dispatch from another ref cannot export.
4. Optional wait timer if policy wants a cooling-off period.
5. **Environment secrets** (not repository secrets):

| Secret | Purpose |
| --- | --- |
| `UPLINK_APP_ID` | GitHub App id (registered on public github.com) |
| `UPLINK_APP_PRIVATE_KEY` | App private key |
| `UPLINK_UPSTREAM_OWNER` | Public GitHub org that owns the project and the private fork |
| `UPLINK_CONTRIB_REPO` | Repository name of the private fork |
| `UPLINK_UPSTREAM_REPO` | Optional. Public parent repository name, if different |

The App must be installed on the private fork (contents: write) and on the public parent (pull requests: write, contents: read). Do not register that App from an EMU account if the App would then be enterprise-scoped and unable to see public github.com repositories.

**Do not also store those App secrets at repo or org level.** If they exist there, a workflow job without the `oss` environment can still mint a token. The environment gate only protects credentials that live on the environment.

`uplink-sync.yml` fetches public `upstream` over git and detects merge via trailers / patch-id / empty apply. It does **not** mint the write App. If you want hourly sync to treat the GitHub PR as merged via the API, add a **read-only** repo secret (`UPLINK_SYNC_TOKEN`) and wire it later — do not reuse the fork-write App key.

Repository `GITHUB_TOKEN` stays a repo permission: it can commit reports to `uplink/state`. It still cannot open the public pull request.

### Ruleset so reports can be committed

The packet job **fast-forwards** a commit of `.uplink/reports/<id>/prepare.md` on `uplink/state` (not a force-push). Import fast-forwards `main` when it applies a new patch. Sync force-updates `main` only for an upstream rebuild. Allow GitHub Actions (or the Uplink bot) to push:

- Humans still require a pull request to `main`.
- Actions may bypass to fast-forward `uplink/state`, and to force-push `main` when upstream (or drop/resolve) requires a replay.

If the ruleset blocks `GITHUB_TOKEN` from pushing `uplink/state`, the packet job fails before anyone is asked to approve `oss`.

### Operator flow

```text
queued patch on uplink/state (and applied on company main)
        │
        ▼
workflow_dispatch Uplink submit (patch_id)
        │
        ▼
packet job: git uplink report → STEP_SUMMARY + commit prepare.md
        │
        ▼
submit job waits on environment oss   ← IP/legal reviews packet
        │  (Deployments + enterprise audit log)
        ▼
approval.md committed; git uplink approve; git uplink submit
        │
        ▼
gh pr create (or reuse existing URL); git uplink submitted
        │
        ▼
fork branch + public PR (first bytes leaving EMU)

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
