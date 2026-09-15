# EMU workflow templates

Copy these files into the **company product repository** on GitHub Enterprise Cloud.

That repository also needs the `git-uplink` binary on `PATH`. Install this crate on the runner (`cargo install --path vendor/git-uplink` or a release binary).

`GITHUB_TOKEN` in these workflows is the **EMU** token and only pushes to the company repo. It cannot open the public pull request.

## Two gates

- **Prepare** (`uplink-prepare.yml`) — on every PR to `main`. Strips the internal commit-message section, rewrites author, scans for company keywords / internal emails, comments a report for approvers, and writes `GITHUB_STEP_SUMMARY`. Required check.
- **Preflight** (`uplink-preflight.yml`) — required check on every PR to `main`. Applies the PR onto public upstream plus `Uplink-Depends-On` lines from the body, then runs `UPLINK_PREFLIGHT`. Failure comments on the PR; do not import until it is green.
- **Import** (`uplink-import.yml`) — internal product approval. Label `uplink:import` after engineering review (or merge the PR). The change lands on company `main` as status `queued` only if export preflight still passes.
- **Sync** (`uplink-sync.yml`) — hourly / manual. Fetches public upstream, drops merged patches, rebuilds `main`. If a patch does not apply, it records `conflict` on the queue (committed on `main` without moving product files), pushes `uplink/conflict/<id>`, and opens an internal PR. Do not merge that PR; resolve the patch instead.
- **Resolve** (`uplink-resolve.yml`) — on human pushes to `uplink/conflict/<id>` (skips `github-actions[bot]`). Runs `git uplink resolve`, rebuilds `main`, and deletes the conflict branch. If rebuild stops on a later patch, it pushes that `uplink/conflict/<id>` and opens an internal PR, same as sync. Does not export to the contribution fork.
- **Submit** (`uplink-submit.yml`) — IP / contribution approval via the **`oss` GitHub Environment**. Dispatch with a patch id. The packet job commits the report; environment reviewers approve; the same run then `git uplink approve` + `git uplink submit`. Preflight runs again; a failing build/test means no fork push and no public PR.

Repo variables:

| Variable | Purpose |
| --- | --- |
| `UPLINK_PREFLIGHT` | Product build/test command on the export tree (example: `npm test`) |
| `UPLINK_REDACT_KEYWORDS` | Comma-separated words that must not appear in a contribution (company name, product aliases) |
| `UPLINK_INTERNAL_DOMAINS` | Comma-separated email domains flagged in the export diff (example: `acme.com`) |
| `UPLINK_EXPORT_AUTHOR` | Default public identity `Name <email>` for contribution commits (machine user). Override per change with `Uplink-Export-Author` below the cutoff. |

Import and sync share the Actions concurrency group `uplink-mutate` at workflow level. Resolve uses that group too. Submit uses it **per job** (packet, then submit) so IP’s environment wait does not freeze imports. The CLI also retries `git push --force-with-lease` if another import landed first.

---

## OSS environment (contribution / IP gate)

Yes: on GitHub Enterprise Cloud, contribution approval should be a **GitHub Environment**, not a second homegrown checkbox. Name it `oss`. Required reviewers are IP/legal. After they approve the waiting deployment, the same workflow records the receipt and submits to the upstream-owned private fork.

This is the documented option. Use it instead of asking an operator to run `git uplink approve` by hand.

### What the reviewer sees

1. **Job summary** — the packet job appends the full contribution packet to `GITHUB_STEP_SUMMARY` (public subject, export author, leak checks, depends-on). Open the workflow run; the summary is on the completed packet job.
2. **Committed report** — `.uplink/reports/<id>/prepare.md` on company `main`. The `oss` deployment URL points at that file. Reports live under `.uplink/`, so a later queue rebuild copies them rather than dropping them.
3. **Environment review UI** — GitHub pauses the submit job until a required reviewer approves the `oss` deployment. That click is the IP gate.

After approval, the submit job writes `.uplink/reports/<id>/approval.md` (in-repo receipt), runs `git uplink approve` then `git uplink submit`, and pushes. The public GitHub App token is minted in this job only.

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

Repository `GITHUB_TOKEN` stays a repo permission: it can commit reports to company `main`. It still cannot open the public pull request.

### Ruleset so reports can be committed

The packet job **fast-forwards** a commit of `.uplink/reports/<id>/prepare.md` (not a force-push). Import and sync still force-update `main`. Allow GitHub Actions (or the Uplink bot) to push to `main`:

- Humans still require a pull request.
- Actions may bypass to commit queue metadata and reports, and to force-push rebuilds.

If the ruleset blocks `GITHUB_TOKEN` from pushing `main`, the packet job fails before anyone is asked to approve `oss`.

### Operator flow

```text
queued patch on company main
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
fork branch + public PR (first bytes leaving EMU)
```

Local equivalent when you are not on Actions (engine tests, a break-glass operator):

```bash
git uplink report upl_…
git uplink approve upl_…
git uplink submit upl_…
```

That still writes the same markdown under `.uplink/reports/`. It does **not** create a GitHub Environment review. On GHEC, use the workflow.
