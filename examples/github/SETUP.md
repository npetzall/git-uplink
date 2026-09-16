# Set up the GitHub example

Bootstrap upstream first, then fork it (GitHub cannot fork an empty repo), then bootstrap contrib and internal. After that, walk the stories with `git apply` / push and the GitHub UI for PRs and gates.

You need a **GitHub organization** plus your user account. GitHub will not let one account fork its own repository, and `git uplink submit` opens a real pull request from the contrib fork to upstream.

| Repository | Owner | Visibility | Role |
| --- | --- | --- | --- |
| `uplink-example-upstream` | org | public | Canonical tokenkit project |
| `uplink-example-upstream-contrib` | your user (fork of upstream) | public on github.com | Contrib remote; `uplink/<id>` branches |
| `uplink-example-internal` | org | private | Company product; bot-owned `main` and `uplink/state` |

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

The script replaces the clone with [`upstream/`](upstream/) (tokenkit + **Reset example**), force-pushes `main`, and creates `seed`.

## 2. Contrib fork

Fork the bootstrapped upstream to your user as `uplink-example-upstream-contrib`. Clone it.

```bash
git clone https://github.com/YOUR_USER/uplink-example-upstream-contrib.git
export CONTRIB_DIR=/path/to/uplink-example-upstream-contrib
./examples/github/scripts/bootstrap_upstream-contrib.sh
```

The script adds the contrib **Reset example** workflow from [`upstream-contrib/`](upstream-contrib/) on contrib `main` (the fork already has upstream `main`). If `UPSTREAM_DIR` is still set, it checks that the fork owner differs from upstream.

## 3. Internal

In the org, create private `uplink-example-internal` (empty is fine). Clone it.

```bash
git clone https://github.com/YOUR_ORG/uplink-example-internal.git
export INTERNAL_DIR=/path/to/uplink-example-internal
# optional:
# export UPLINK_SRC=npetzall/git-uplink
# export UPLINK_REV=main
# export UPLINK_SUBMIT_AUTH=pat

./examples/github/scripts/bootstrap_internal.sh
```

The script needs all three clones. It sets remotes, runs **`git uplink init`**, overlays [`internal/`](internal/), **`git uplink add --internal-only`**, and pushes `main`, `uplink/state`, `uplink/upstream`, `seed`, `seed-state`, `seed-upstream`. If `gh` is authenticated: labels, repo variables, Actions write permission, Environment `oss`.

`git uplink status` in the internal clone should show one internal-only patch (`Example GitHub workflows`).

Keep the internal and upstream clones for the stories:

```bash
export KIT=/path/to/git-uplink/examples/github
```

## 4. GitHub settings (UI)

If `bootstrap_internal.sh` did not run `gh`, do this in **uplink-example-internal**:

**Labels:** `uplink:import`, `uplink:internal-only`, `uplink:conflict`

**Variables** (Settings → Secrets and variables → Actions → Variables):

| Variable | Example |
| --- | --- |
| `UPLINK_PREFLIGHT` | `npm test` |
| `UPLINK_REDACT_KEYWORDS` | `companyTelemetry,AcmeCorp` |
| `UPLINK_INTERNAL_DOMAINS` | `acme.example` |
| `UPLINK_EXPORT_AUTHOR` | `Uplink Example <uplink@users.noreply.github.com>` |
| `UPLINK_SRC` | `npetzall/git-uplink` |
| `UPLINK_REV` | `main` |
| `UPLINK_SUBMIT_AUTH` | `pat` |
| `UPLINK_UPSTREAM` | `YOUR_ORG/uplink-example-upstream` |
| `UPLINK_CONTRIB` | `YOUR_USER/uplink-example-upstream-contrib` |

**Actions:** workflow permissions **Read and write**.

**Environment `oss`:** Settings → Environments → New environment → `oss`.

1. **Required reviewers** — add yourself. For a solo walkthrough leave **Prevent self-review** off.
2. **Deployment branches** — restrict to `main` if the UI offers it.
3. **Environment secrets** (not repository secrets):

### PAT (default, `UPLINK_SUBMIT_AUTH=pat`)

The contrib fork is on a **different owner** than upstream, so a personal access token can both push the fork and open the upstream PR.

Fine-grained or classic PAT with:

- `uplink-example-upstream-contrib`: Contents read/write
- `uplink-example-upstream`: Contents read, Pull requests read/write

Store it on the **oss** environment as `UPLINK_GITHUB_TOKEN`.

### GitHub App (`UPLINK_SUBMIT_AUTH=app`)

Production-shaped; see [`templates/README.md`](../../templates/README.md). An installation token is one owner. On the **oss** environment:

| Secret | Purpose |
| --- | --- |
| `UPLINK_APP_ID` | GitHub App id |
| `UPLINK_APP_PRIVATE_KEY` | App private key |
| `UPLINK_UPSTREAM_OWNER` | Owner of the contrib repository |
| `UPLINK_CONTRIB_REPO` | `uplink-example-upstream-contrib` |

Install the App on the contrib fork (contents: write) and on upstream (contents: read, pull requests: write). Set variable `UPLINK_SUBMIT_AUTH` to `app`.

## 5. Reset (Actions)

Each story starts by restoring the three repositories. Force-push is expected.

On **each** repo: **Actions → Reset example → Run workflow**.

| Repo | Branch to run from | What it does |
| --- | --- | --- |
| `uplink-example-upstream` | `seed` | Close PRs; delete extra branches; `main` ← `seed` |
| `uplink-example-upstream-contrib` | `main` | Delete extra branches (`uplink/<id>`). Does not move `main`. No PRs on this repo |
| `uplink-example-internal` | `seed` | Close PRs and `uplink:conflict` issues; `main` ← `seed`; `uplink/state` ← `seed-state`; `uplink/upstream` ← `seed-upstream` |

Then in the clones:

```bash
# internal
git fetch origin
git checkout main
git reset --hard origin/main
git fetch origin '+refs/heads/uplink/state:refs/heads/uplink/state'
git fetch origin '+refs/heads/uplink/upstream:refs/heads/uplink/upstream'

# upstream (when the story touches it)
git fetch origin
git checkout main
git reset --hard origin/main
```

## Optional: branch protection

Not required. `GITHUB_TOKEN` with write permissions can push `main` and `uplink/state`. Production rulesets are in [`templates/README.md`](../../templates/README.md).

## Next

[`README.md`](README.md) lists the stories. Rationale: [`way-of-working.md`](../../way-of-working.md).
