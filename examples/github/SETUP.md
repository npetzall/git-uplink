# Set up the GitHub example

Bootstrap upstream first, then fork it (GitHub cannot fork an empty repo), then bootstrap contrib and internal. After that, walk the stories with `git apply` / push and the GitHub UI for PRs and gates.

You need a **GitHub organization** plus your user account. GitHub will not let one account fork its own repository, and `git uplink submit` opens a real pull request from the contrib fork to upstream.

| Repository | Owner | Visibility | Role |
| --- | --- | --- | --- |
| `uplink-example-upstream` | org | public | Canonical tokenkit project |
| `uplink-example-upstream-contrib` | your user (fork of upstream) | public on github.com | Contrib remote; `uplink/<id>` branches |
| `uplink-example-internal` | org | private | Company product; humans merge PRs to `main`; bot owns `uplink/state` |

A true private fork of a public parent needs GitHub Enterprise. On github.com the fork is public; that is enough for this example.

## Prerequisites

- `git`
- `git-uplink` on `PATH` (`cargo install --path .` from this crate)
- Node.js 22 only if you run `npm test` locally (Actions uses Node 22)
- `gh` is optional (`bootstrap_internal.sh` can set labels and variables if you are logged in)

## 1. Upstream

In the org, create public `uplink-example-upstream` (empty is fine). Clone it. `origin` must point at that GitHub repo.

```bash
git clone https://github.com/YOUR_ORG/uplink-example-upstream.git
export UPSTREAM_DIR=/path/to/uplink-example-upstream
./examples/github/scripts/bootstrap_upstream.sh
```

The script replaces the clone with [`upstream/`](upstream/) (tokenkit), installs the shared **Reset example** stub workflow, force-pushes `main`, creates `seed`, and publishes orphan `example-reset` (the per-repo reset script).

## 2. Contrib fork

Fork the bootstrapped upstream to your user as `uplink-example-upstream-contrib`. Clone it.

```bash
git clone https://github.com/YOUR_USER/uplink-example-upstream-contrib.git
export CONTRIB_DIR=/path/to/uplink-example-upstream-contrib
./examples/github/scripts/bootstrap_upstream-contrib.sh
```

The script does **not** change contrib `main` (it must stay a fork of upstream `main` so submit does not rewrite workflows). It publishes orphan `example-reset` with the contrib reset script. If `UPSTREAM_DIR` is still set, it checks that the fork owner differs from upstream.

If you already bootstrapped an older layout that committed reset files on contrib `main`:

```bash
git remote add upstream https://github.com/YOUR_ORG/uplink-example-upstream.git  # if missing
git fetch upstream
git checkout main
git reset --hard upstream/main
git push origin main --force
```

Then re-run `bootstrap_upstream-contrib.sh`. Do not run the old contrib **Reset example** first: it deleted every branch except `main`.

## 3. Internal

In the org, create private `uplink-example-internal` (empty is fine). Clone it.

```bash
git clone https://github.com/YOUR_ORG/uplink-example-internal.git
export INTERNAL_DIR=/path/to/uplink-example-internal
# optional:
# export UPLINK_SRC=npetzall/git-uplink
# export UPLINK_REV=main
# export UPLINK_INTERNAL_AUTH=pat
# export UPLINK_CONTRIB_AUTH=pat
# export UPLINK_UPSTREAM_AUTH=pat

./examples/github/scripts/bootstrap_internal.sh
```

The script needs all three clones. It sets remotes, runs **`git uplink init --forge example-github`** (installs the internal-only Uplink Actions pack), and pushes `main`, `uplink/state`, `uplink/upstream`, `seed`, `seed-state`, `seed-upstream`, and orphan `example-reset`. If `gh` is authenticated: labels, repo variables, Actions write permission, Environments `to-upstream` and `from-upstream`.

`git uplink status` in the internal clone should show the tooling patch (`Uplink tooling`) in the tooling slot.

Keep the internal and upstream clones for the stories:

```bash
export KIT=/path/to/git-uplink/examples/github
```

## 4. GitHub settings (UI)

If `bootstrap_internal.sh` did not run `gh`, do this in **uplink-example-internal**:

**Labels:** `uplink:internal-only`, `uplink:conflict`

**Variables** (Settings → Secrets and variables → Actions → Variables):

| Variable | Example |
| --- | --- |
| `UPLINK_PREFLIGHT` | `npm test` |
| `UPLINK_REDACT_KEYWORDS` | `companyTelemetry,AcmeCorp` |
| `UPLINK_INTERNAL_DOMAINS` | `acme.example` |
| `UPLINK_EXPORT_AUTHOR` | `Uplink Example <uplink@users.noreply.github.com>` |
| `UPLINK_SRC` | `npetzall/git-uplink` |
| `UPLINK_REV` | `main` |
| `UPLINK_INTERNAL_AUTH` | `pat` |
| `UPLINK_CONTRIB_AUTH` | `pat` |
| `UPLINK_UPSTREAM_AUTH` | `pat` |

Each `UPLINK_*_AUTH` is `pat` or `app`. Empty defaults to `pat` in this example (production templates default to `app`).

**Actions:** workflow permissions **Read and write**.

**Environment `to-upstream`:** Settings → Environments → New environment → `to-upstream`.

1. **Required reviewers** — add yourself. For a solo walkthrough leave **Prevent self-review** off.
2. **Deployment branches** — restrict to `main` if the UI offers it.
3. **Secrets** — contrib write credentials live on **to-upstream**. Internal force-push and upstream fetch credentials are **repository** secrets (sync cannot wait on to-upstream).

**Environment `from-upstream`:** Settings → Environments → New environment → `from-upstream`.

1. **Required reviewers** — add yourself (solo walkthrough: leave **Prevent self-review** off).
2. **Deployment branches** — restrict to `main` if the UI offers it.
3. **Secrets** — none. This is the inbound review gate only. Sync inspect must not wait on it.

### Internal (repo secrets; origin force-push)

Used by import, sync, resolve, and the submit packet job.

#### PAT (default, `UPLINK_INTERNAL_AUTH=pat`)

Fine-grained or classic PAT with contents read/write **and workflows write** on `uplink-example-internal` (sync/resolve push `main` and conflict branches that include `.github/workflows`). Store as repo secret `UPLINK_INTERNAL_TOKEN`.

#### GitHub App (`UPLINK_INTERNAL_AUTH=app`)

Install an App on the internal repo (contents: write, workflows: write). Repo secrets: `UPLINK_INTERNAL_APP_ID`, `UPLINK_INTERNAL_APP_PRIVATE_KEY`.

### Upstream (repo secrets; authenticated `git fetch` of public parent)

Used by sync and resolve so github.com applies authenticated rate limits. Read-only. Do not reuse contrib write creds.

#### PAT (default, `UPLINK_UPSTREAM_AUTH=pat`)

Fine-grained PAT with contents **read** on `uplink-example-upstream`. Store as repo secret `UPLINK_UPSTREAM_TOKEN`. The same token is what a future GitHub PR merge check would use (no separate `UPLINK_SYNC_TOKEN`).

#### GitHub App (`UPLINK_UPSTREAM_AUTH=app`)

Install an App on the public parent (contents: read). Repo secrets: `UPLINK_UPSTREAM_APP_ID`, `UPLINK_UPSTREAM_APP_PRIVATE_KEY`, plus `UPLINK_UPSTREAM_OWNER` and `UPLINK_UPSTREAM_REPO` (`uplink-example-upstream`).

### Contrib (`to-upstream` environment secrets; fork write + public PR)

#### PAT (default, `UPLINK_CONTRIB_AUTH=pat`)

The contrib fork is on a **different owner** than upstream, so a personal access token can both push the fork and open the upstream PR.

Fine-grained or classic PAT with:

- `uplink-example-upstream-contrib`: Contents read/write
- `uplink-example-upstream`: Contents read, Pull requests read/write

Store it on the **to-upstream** environment as `UPLINK_CONTRIB_TOKEN`. `gh pr create` uses this token (`GH_TOKEN`).

#### GitHub App (`UPLINK_CONTRIB_AUTH=app`)

Production-shaped; see [`templates/README.md`](../../templates/README.md). An installation token is one owner. On the **to-upstream** environment:

| Secret | Purpose |
| --- | --- |
| `UPLINK_CONTRIB_APP_ID` | GitHub App id |
| `UPLINK_CONTRIB_APP_PRIVATE_KEY` | App private key |
| `UPLINK_UPSTREAM_OWNER` | Owner of the contrib repository |
| `UPLINK_CONTRIB_REPO` | `uplink-example-upstream-contrib` |

Install the App on the contrib fork (contents: write) and on upstream (contents: read, pull requests: write). Set variable `UPLINK_CONTRIB_AUTH` to `app`.

## 5. Reset (Actions)

Each story starts by restoring the three repositories. Force-push is expected.

On **each** repo: **Actions → Reset example → Run workflow**. The stub YAML always checks out orphan `example-reset`, so the branch you dispatch from does not matter. If the workflow is missing on a wrecked default branch, use **Use workflow from** `example-reset`.

| Repo | What it does |
| --- | --- |
| `uplink-example-upstream` | Close PRs; delete extra branches (keeps `main`, `seed`, `example-reset`); `main` ← `seed` |
| `uplink-example-upstream-contrib` | Delete extra branches (`uplink/<id>`). Keeps `main`, `seed`, `example-reset`. Does not move `main`. No PRs on this repo |
| `uplink-example-internal` | Close PRs and `uplink:conflict` issues; keeps `example-reset` and seed refs; `main` ← `seed`; `uplink/state` ← `seed-state`; `uplink/upstream` ← `seed-upstream` |

Then in the clones:

```bash
# internal
git uplink reset

# upstream (when the story touches it)
git fetch origin
git checkout main
git reset --hard origin/main
```

## Optional: branch protection

Not required. Sync and resolve persist `UPLINK_INTERNAL_TOKEN` on checkout so shell origin git can push workflow files. `git uplink` origin transport uses the same token. Production rulesets are in [`templates/README.md`](../../templates/README.md).

## Next

[`README.md`](README.md) lists the stories. Rationale: [`way-of-working.md`](../../way-of-working.md).
