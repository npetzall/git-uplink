# Set up the GitHub example

Bootstrap upstream first, then fork it (GitHub cannot fork an empty repo), then bootstrap contrib and internal. Create the PATs or Apps next, then put secrets on the environments. After that, walk the stories with `git apply` / push and the GitHub UI for PRs and gates.

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

The script needs all three clones. It sets remotes, runs **`git uplink init --forge example-github`** (installs the internal-only Uplink Actions pack), and pushes `main`, `uplink/state`, `uplink/upstream`, `seed`, `seed-state`, `seed-upstream`, and orphan `example-reset`. If `gh` is authenticated: labels, repo variables, Actions write permission, and empty Environments `to-upstream`, `from-upstream`, and `abandon-contrib`. It does not create tokens.

`git uplink status` in the internal clone should show the tooling patch (`Uplink tooling`) in the tooling slot.

Keep the internal and upstream clones for the stories:

```bash
export KIT=/path/to/git-uplink/examples/github
```

## 4. Credentials

Create these before you fill environment secrets. Three bots, each a fine-grained PAT or a GitHub App. This example defaults each `UPLINK_*_AUTH` to `pat`. Production templates default to `app`.

A PAT is one token stored as `UPLINK_*_TOKEN`. An App is installed on **one owner**; the workflow mints an installation token from the App id and private key. Here the fork owner is your user and the parent owner is the org, so the contrib App and the upstream App are different installations. Use PATs for the walkthrough unless you are rehearsing that App layout.

### Repository variables

Settings → Secrets and variables → Actions → Variables. `bootstrap_internal.sh` sets these when `gh` is authenticated.

| Variable | Example | Purpose |
| --- | --- | --- |
| `UPLINK_PREFLIGHT` | `npm test` | Build/test command on the export tree |
| `UPLINK_REDACT_KEYWORDS` | `companyTelemetry,AcmeCorp` | Words that must not appear in a contribution |
| `UPLINK_INTERNAL_DOMAINS` | `acme.example` | Email domains flagged in the export diff |
| `UPLINK_EXPORT_AUTHOR` | `Uplink Example <uplink@users.noreply.github.com>` | Public identity for contribution commits |
| `UPLINK_SRC` | `npetzall/git-uplink` | Repo the example runner builds `git-uplink` from |
| `UPLINK_REV` | `main` | Git ref of `UPLINK_SRC` |
| `UPLINK_INTERNAL_AUTH` | `pat` | `pat` or `app` for the internal bot |
| `UPLINK_UPSTREAM_AUTH` | `pat` | `pat` or `app` for the upstream bot |
| `UPLINK_CONTRIB_AUTH` | `pat` | `pat` or `app` for the contrib bot |

### Internal

Pushes company `main` and the gated conflict and transfer branches, including `.github/workflows`. Used by import, sync, resolve, and transfer. PR checks, gate, and submit copy the Actions `GITHUB_TOKEN` into `UPLINK_INTERNAL_TOKEN` for origin reads and `uplink/state` fast-forwards.

Repository secrets. Sync cannot wait on an environment.

#### PAT (`UPLINK_INTERNAL_AUTH=pat`)

Fine-grained or classic PAT on `uplink-example-internal`.

| Permission | Access |
| --- | --- |
| Contents | Read and write |
| Workflows | Read and write |

#### App (`UPLINK_INTERNAL_AUTH=app`)

Install on `uplink-example-internal` only.

| Permission | Access |
| --- | --- |
| Contents | Read and write |
| Workflows | Read and write |

#### Secrets

| Secret | When | Value |
| --- | --- | --- |
| `UPLINK_INTERNAL_TOKEN` | `pat` | The PAT |
| `UPLINK_INTERNAL_APP_ID` | `app` | App id |
| `UPLINK_INTERNAL_APP_PRIVATE_KEY` | `app` | App private key (PEM) |

### Upstream

Authenticated `git fetch` of the public parent (sync, resolve, transfer) and the public pull request (submit `POST /repos/{parent}/pulls`, abandon `gh pr close`). Contents **read** and pull requests **write** on `uplink-example-upstream` only. No access to the fork.

Submit opens that PR with `maintainer_can_modify` false. `head` is `<contrib_org>:<branch>`. This token cannot read the fork, so GitHub cannot grant maintainers push access to the head branch. A maintainer commit on that branch would also sit outside the queue: there is no path to bring it back onto company `main`.

Repository secrets. Do not copy them onto `to-upstream` or `abandon-contrib`. Those jobs already read repository secrets. An environment copy of `UPLINK_UPSTREAM_OWNER` would hide the org and mint the App against the fork owner.

#### PAT (`UPLINK_UPSTREAM_AUTH=pat`)

Fine-grained PAT scoped to `uplink-example-upstream` only.

| Permission | Access |
| --- | --- |
| Contents | Read |
| Pull requests | Read and write |

#### App (`UPLINK_UPSTREAM_AUTH=app`)

Install on the org that owns `uplink-example-upstream`. Not on the fork.

| Permission | Access |
| --- | --- |
| Contents | Read |
| Pull requests | Read and write |

Sync, resolve, and transfer mint this App with contents read only. Submit and abandon also request pull requests write.

#### Secrets

| Secret | When | Value |
| --- | --- | --- |
| `UPLINK_UPSTREAM_TOKEN` | `pat` | The PAT |
| `UPLINK_UPSTREAM_APP_ID` | `app` | App id |
| `UPLINK_UPSTREAM_APP_PRIVATE_KEY` | `app` | App private key (PEM) |
| `UPLINK_UPSTREAM_OWNER` | `app` | Org that owns `uplink-example-upstream` |
| `UPLINK_UPSTREAM_REPO` | `app` | `uplink-example-upstream` |

### Contrib

Force-pushes `uplink/<id>` on the fork during submit, and deletes that branch during abandon. Contents write on `uplink-example-upstream-contrib` only. No pull-request permission, and no install on the parent. `git uplink submit` uses `UPLINK_CONTRIB_TOKEN`. The public pull request uses the upstream token.

Environment secrets on **`to-upstream`** and a copy on **`abandon-contrib`**. GitHub environments do not share secrets. Do not also store these at repository or organization level (sync and import must not see the fork write credential).

#### PAT (`UPLINK_CONTRIB_AUTH=pat`)

Fine-grained PAT scoped to `uplink-example-upstream-contrib` only. The fork is a different owner than upstream, so this token does not open the upstream PR.

| Permission | Access |
| --- | --- |
| Contents | Read and write |

#### App (`UPLINK_CONTRIB_AUTH=app`)

Install on the fork owner (your user), not the upstream org. One installation token is one owner.

| Permission | Access |
| --- | --- |
| Contents | Read and write |

#### Secrets

| Secret | When | Value |
| --- | --- | --- |
| `UPLINK_CONTRIB_TOKEN` | `pat` | The PAT |
| `UPLINK_CONTRIB_APP_ID` | `app` | App id |
| `UPLINK_CONTRIB_APP_PRIVATE_KEY` | `app` | App private key (PEM) |
| `UPLINK_CONTRIB_OWNER` | `app` | Your user (fork owner). If the fork lived in the same account as upstream, omit this and the workflow falls back to `UPLINK_UPSTREAM_OWNER` |
| `UPLINK_CONTRIB_REPO` | `app` | `uplink-example-upstream-contrib` |

### Where each secret lives

| Secret | Repository | `to-upstream` | `abandon-contrib` |
| --- | --- | --- | --- |
| `UPLINK_INTERNAL_TOKEN` | PAT | | |
| `UPLINK_INTERNAL_APP_ID` | App | | |
| `UPLINK_INTERNAL_APP_PRIVATE_KEY` | App | | |
| `UPLINK_UPSTREAM_TOKEN` | PAT | | |
| `UPLINK_UPSTREAM_APP_ID` | App | | |
| `UPLINK_UPSTREAM_APP_PRIVATE_KEY` | App | | |
| `UPLINK_UPSTREAM_OWNER` | App | | |
| `UPLINK_UPSTREAM_REPO` | App | | |
| `UPLINK_CONTRIB_TOKEN` | | PAT | same PAT |
| `UPLINK_CONTRIB_APP_ID` | | App | same value |
| `UPLINK_CONTRIB_APP_PRIVATE_KEY` | | App | same value |
| `UPLINK_CONTRIB_OWNER` | | App | same value |
| `UPLINK_CONTRIB_REPO` | | App | same value |

## 5. Environments

`bootstrap_internal.sh` creates the three environments with no reviewers and no secrets when `gh` is authenticated. Add reviewers and the contrib secrets below. If `gh` did not run, create each environment first (Settings → Environments → New environment), then the same settings.

**Environment `to-upstream`:**

1. **Required reviewers** — add yourself. For a solo walkthrough leave **Prevent self-review** off.
2. **Deployment branches** — restrict to `main` if the UI offers it.
3. **Secrets** — the contrib table above. Upstream credentials stay repository secrets; this job reads them anyway.

**Environment `from-upstream`:**

1. **Required reviewers** — add yourself (solo walkthrough: leave **Prevent self-review** off).
2. **Deployment branches** — restrict to `main` if the UI offers it.
3. **Secrets** — none. This is the inbound review gate only. Sync inspect must not wait on it.

**Environment `abandon-contrib`:**

1. **Required reviewers** — add yourself (solo walkthrough: leave **Prevent self-review** off). Different audience than `to-upstream` in production (engineering vs IP/legal).
2. **Deployment branches** — restrict to `main` if the UI offers it.
3. **Secrets** — copy the **same** contrib secrets as `to-upstream`. After `--to-internal` of a submitted patch, the public PR and `uplink/<id>` fork branch remain until this environment is approved. Closing the PR uses the repository upstream token; deleting the branch uses these contrib secrets.

## 6. Labels, Actions, required checks

If `bootstrap_internal.sh` did not run `gh`, also do this in **uplink-example-internal**:

**Labels:** `uplink:internal-only`, `uplink:conflict`, `uplink:transfer-to-upstream`, `uplink:transfer-to-internal`

**Actions:** workflow permissions **Read and write**.

**Required checks** (branch protection / ruleset on `main`): **Uplink upstream assess**, **Uplink upstream preflight**.

## 7. Reset (Actions)

Each story starts by restoring the three repositories. Force-push is expected.

On **each** repo: **Actions → Reset example → Run workflow**. The stub YAML always checks out orphan `example-reset`, so the branch you dispatch from does not matter. If the workflow is missing on a wrecked default branch, use **Use workflow from** `example-reset`.

| Repo | What it does |
| --- | --- |
| `uplink-example-upstream` | Close PRs; delete extra branches (keeps `main`, `seed`, `example-reset`); `main` ← `seed` |
| `uplink-example-upstream-contrib` | Delete extra branches (`uplink/<id>`). Keeps `main`, `seed`, `example-reset`. Does not move `main`. No PRs on this repo |
| `uplink-example-internal` | Close PRs (including gated conflict/transfer PRs); keeps `example-reset` and seed refs; `main` ← `seed`; `uplink/state` ← `seed-state`; `uplink/upstream` ← `seed-upstream` |

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

Not required for the walkthrough. Production rulesets are in [`templates/README.md`](../../templates/README.md):

- Protect `main` as usual (PR required).
- Protect the three gated **bases**: `uplink/conflict/*`, `uplink/transfer-to-upstream/*`, `uplink/transfer-to-internal/*`. **Exclude** `*-work`. Require a pull request; no direct pushes. Require the **Uplink gate** check.
- Bypass for the internal Uplink App/PAT and GitHub Actions is only to **create and delete** those protected bases. Humans push `-work` only; merge is the only update to a base.
- A separate ruleset **including** `*-work` must not freeze product CI. Restrict only pack paths (`.github/workflows/uplink-*.yml`, `.github/actions/install-git-uplink/**`) so a gated PR can still fix `ci.yml`. Sample JSON: [`templates/github/uplink-pack-files-ruleset.json`](../../templates/github/uplink-pack-files-ruleset.json). Bypass the internal App/PAT and GitHub Actions so conflict/transfer branches can still be created.

Sync, resolve, and transfer persist the internal PAT or App on checkout so shell origin git can push `main` and gated branches. `git uplink` origin transport on those jobs uses the same token. PR checks, gate, and submit set `UPLINK_INTERNAL_TOKEN` to the Actions `GITHUB_TOKEN` for origin reads and fast-forwards of `uplink/state`.

## Next

[`README.md`](README.md) lists the stories. Rationale: [`way-of-working.md`](../../way-of-working.md).
