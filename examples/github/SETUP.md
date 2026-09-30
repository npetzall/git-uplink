# Set up the GitHub example

This walkthrough runs the whole Uplink model on three repositories in a **new GitHub organization**, using **fine-grained personal access tokens**. It is the shortest path to seeing it work. GitHub Apps, EMU, and forks under another owner are production concerns; see [Production setup](https://npetzall.github.io/git-uplink/setup).

Order matters: bootstrap upstream first, then fork it (GitHub cannot fork an empty repository), then bootstrap contrib and internal. Then create tokens, secrets, and environments. After that, walk the [stories](README.md).

| Repository | Visibility | Role |
| --- | --- | --- |
| `uplink-example-upstream` | public | Canonical tokenkit project |
| `uplink-example-upstream-contrib` | public (fork of upstream) | Contrib remote; `uplink/<id>` branches |
| `uplink-example-internal` | private | Company product; humans merge PRs to `main`; bot owns `uplink/state` |

A true private fork of a public parent needs GitHub Enterprise. On github.com the fork is public; that is enough for this example.

## Prerequisites

- `git`
- `git-uplink` on `PATH` — [install a release binary](https://npetzall.github.io/git-uplink/install)
- A clone of this repository (for `examples/github/scripts/`)
- Node.js 22 only if you run tokenkit's `npm test` locally (Actions brings its own)
- `gh`, logged in (optional but recommended: the bootstrap sets labels, variables, and environments, and you can set secrets from the terminal)

```bash
git clone https://github.com/npetzall/git-uplink.git
cd git-uplink
```

## 0. Create an organization

Create a free organization on github.com (**Your organizations → New organization**). Call it `YOUR_ORG` below. A fresh organization keeps the example isolated and lets fine-grained tokens cover all three repositories.

In **Organization settings → Personal access tokens → Settings**, allow fine-grained personal access tokens. If you require approval, approve your own tokens under **Pending requests** after you create them in step 4.

## 1. Upstream

Create public `YOUR_ORG/uplink-example-upstream` (empty is fine) and clone it.

```bash
git clone https://github.com/YOUR_ORG/uplink-example-upstream.git
export UPSTREAM_DIR=/path/to/uplink-example-upstream
./examples/github/scripts/bootstrap_upstream.sh
```

The script replaces the clone with [`upstream/`](upstream/) (tokenkit), installs the shared **Reset example** stub workflow, force-pushes `main`, creates `seed`, and publishes orphan `example-reset` (the per-repo reset script).

## 2. Contrib fork

Fork `uplink-example-upstream` **into the same organization** as `uplink-example-upstream-contrib`, and clone it.

```bash
git clone https://github.com/YOUR_ORG/uplink-example-upstream-contrib.git
export CONTRIB_DIR=/path/to/uplink-example-upstream-contrib
./examples/github/scripts/bootstrap_upstream-contrib.sh
```

The script does **not** change contrib `main` (it must stay a fork of upstream `main` so submit does not rewrite workflows). It publishes orphan `example-reset` with the contrib reset script. If `UPSTREAM_DIR` is set, it rejects the case where contrib is the same repository as upstream.

<details>
<summary>Upgrading from an older layout that committed reset files on contrib <code>main</code></summary>

```bash
git remote add upstream https://github.com/YOUR_ORG/uplink-example-upstream.git  # if missing
git fetch upstream
git checkout main
git reset --hard upstream/main
git push origin main --force
```

Then re-run `bootstrap_upstream-contrib.sh`. Do not run the old contrib **Reset example** first: it deleted every branch except `main`.

</details>

## 3. Internal

Create private `YOUR_ORG/uplink-example-internal` (empty is fine) and clone it.

```bash
git clone https://github.com/YOUR_ORG/uplink-example-internal.git
export INTERNAL_DIR=/path/to/uplink-example-internal
./examples/github/scripts/bootstrap_internal.sh
```

The script needs all three clones. It sets remotes, runs **`git uplink init --forge example-github`** (installs the Uplink Actions pack as the tooling patch), and pushes `main`, `uplink/state`, `uplink/upstream`, `seed`, `seed-state`, `seed-upstream`, and orphan `example-reset`. If `gh` is authenticated it also creates labels, repository variables, Actions write permission, and empty Environments `to-upstream`, `from-upstream`, and `abandon-contrib`. It does not create tokens.

`git uplink status` in the internal clone should show `Uplink tooling` in the tooling slot.

Keep the internal and upstream clones for the stories:

```bash
export KIT=/path/to/git-uplink/examples/github
```

## 4. Tokens

Create three fine-grained tokens under **Settings → Developer settings → Personal access tokens → Fine-grained tokens → Generate new token**. For each: **Resource owner** is `YOUR_ORG`, **Repository access** is *Only select repositories*. Metadata read is added automatically.

| Token | Repositories | Permissions | Used by |
| --- | --- | --- | --- |
| **internal** | `uplink-example-internal` | Contents: read and write · Workflows: read and write · Pull requests: read and write | Import, sync, resolve, transfer push company `main` and gated branches (including `.github/workflows`) and open the gated PRs |
| **upstream** | `uplink-example-upstream`, `uplink-example-upstream-contrib` | Contents: read · Pull requests: read and write | Fetch public upstream; open (submit) and close (abandon) the public PR |
| **contrib** | `uplink-example-upstream-contrib` | Contents: read and write | Push and delete `uplink/<id>` on the fork |

The upstream token has no contents write on the fork, so submit opens the public PR with `maintainer_can_modify` false. A maintainer commit on the fork branch would sit outside the queue with no path back onto company `main`.

## 5. Secrets and environments

`bootstrap_internal.sh` created the three environments with no reviewers and no secrets (if `gh` was not logged in, create them under **Settings → Environments**). Store each token where only the jobs that need it can read it. `gh secret set` prompts for the value, so it stays out of your shell history:

```bash
gh secret set UPLINK_INTERNAL_TOKEN --repo YOUR_ORG/uplink-example-internal
gh secret set UPLINK_UPSTREAM_TOKEN --repo YOUR_ORG/uplink-example-internal
gh secret set UPLINK_CONTRIB_TOKEN  --repo YOUR_ORG/uplink-example-internal --env to-upstream
gh secret set UPLINK_CONTRIB_TOKEN  --repo YOUR_ORG/uplink-example-internal --env abandon-contrib
```

| Secret | Where | Why |
| --- | --- | --- |
| `UPLINK_INTERNAL_TOKEN` | Repository | Sync runs hourly and cannot wait on an environment |
| `UPLINK_UPSTREAM_TOKEN` | Repository | Submit and abandon read repository secrets too; do not copy it onto environments |
| `UPLINK_CONTRIB_TOKEN` | `to-upstream` **and** `abandon-contrib` | The fork write credential exists only behind a review. Environments do not share secrets, so store it twice. Never at repository or organization level |

Then, in **Settings → Environments**, for each of `to-upstream`, `from-upstream`, and `abandon-contrib`:

1. **Required reviewers** — add yourself. For a solo walkthrough leave **Prevent self-review** off.
2. **Deployment branches** — restrict to `main` if the UI offers it.

What each gate is for:

- **`to-upstream`** — IP approval before a contribution leaves. In production, IP/legal.
- **`from-upstream`** — review of foreign public commits before they land on company `main`. No secrets.
- **`abandon-contrib`** — withdrawing a submitted contribution after `--to-internal`. Until approved, the public PR and `uplink/<id>` branch remain.

### Repository variables

**Settings → Secrets and variables → Actions → Variables.** `bootstrap_internal.sh` sets these when `gh` is authenticated.

| Variable | Example | Purpose |
| --- | --- | --- |
| `UPLINK_PREFLIGHT` | `npm test` | Build/test command on the export tree |
| `UPLINK_REDACT_KEYWORDS` | `companyTelemetry,AcmeCorp` | Words that must not appear in a contribution |
| `UPLINK_INTERNAL_DOMAINS` | `acme.example` | Email domains flagged in the export diff |
| `UPLINK_EXPORT_AUTHOR` | `Uplink Example <uplink@example.com>` | Public identity for contribution commits |
| `UPLINK_SRC` | `npetzall/git-uplink` | Repository that publishes `git-uplink` releases |
| `UPLINK_VERSION` | `latest` | Release to download (`latest`, or a tag `vX.Y.Z` / `X.Y.Z`) |
| `UPLINK_INTERNAL_AUTH` / `UPLINK_UPSTREAM_AUTH` / `UPLINK_CONTRIB_AUTH` | `pat` | Token model per role. The example pack defaults to `pat` |

## 6. Labels, Actions, required checks

If `bootstrap_internal.sh` did not run `gh`, also do this in **uplink-example-internal**:

- **Labels:** `uplink:internal-only`, `uplink:conflict`, `uplink:transfer-to-upstream`, `uplink:transfer-to-internal`
- **Actions:** workflow permissions **Read and write**.
- **Required checks** (branch protection / ruleset on `main`): **Uplink upstream assess**, **Uplink upstream preflight**.

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
git fetch origin --prune
git uplink status

# upstream (when the story touches it)
git fetch origin --prune
git switch -C main origin/main
```

Actions clears remotes; `git fetch origin --prune` drops stale remote-tracking branches, and `git switch -C <branch> main` recreates a clean local feature branch from company `main` (even if that branch name already exists from a previous story).

## Optional: branch protection

Not required for the walkthrough. Production rulesets, with ready-to-run JSON, are in [Production setup → rulesets](https://npetzall.github.io/git-uplink/setup?forge=ghec&view=steps#rulesets):

- Protect `main` as usual (PR required).
- Protect the three gated **bases**: `uplink/conflict/*`, `uplink/transfer-to-upstream/*`, `uplink/transfer-to-internal/*`. **Exclude** `*-work`. Require a pull request; no direct pushes. Require the **Uplink gate** check.
- Bypass for the internal token owner and GitHub Actions is only to **create and delete** those protected bases. Humans push `-work` only; merge is the only update to a base.
- A separate ruleset **including** `*-work` must not freeze product CI. Restrict only pack paths (`.github/workflows/uplink-*.yml`, `.github/actions/install-git-uplink/**`) so a gated PR can still fix `ci.yml`. Sample JSON: [`templates/github/uplink-pack-files-ruleset.json`](../../templates/github/uplink-pack-files-ruleset.json).

## Next

[`README.md`](README.md) lists the stories. Background: [How](https://npetzall.github.io/git-uplink/how) and [Day to day](https://npetzall.github.io/git-uplink/day-to-day). Going to production (GitHub Apps, EMU, a fork under another owner): [Production setup](https://npetzall.github.io/git-uplink/setup).
