# Set up the GitHub example

The step-by-step setup lives on the site: **[Try it yourself → Setup](https://npetzall.github.io/git-uplink/examples?view=setup)**. It covers a new organization, the three repositories, fine-grained tokens, secrets, Environments, and required checks, in the web UI or with the gh CLI. Commands are for bash or zsh; on Windows use WSL or Git Bash.

The commands use the same variables as the stories:

```bash
export ORG=<your-org>
export WORK=~/src
export KIT=/path/to/git-uplink/examples/github
```

The setup copies these kit files into the repositories:

| Kit file | Goes to |
| --- | --- |
| [`upstream/`](upstream/) | The first commit of `uplink-example-upstream` |
| [`example-reset.yml`](example-reset.yml) | `.github/workflows/example-reset.yml` on upstream `main`, and on each repo's `example-reset` branch |
| [`reset/upstream.sh`](reset/upstream.sh), [`reset/contrib.sh`](reset/contrib.sh), [`reset/internal.sh`](reset/internal.sh) | `scripts/reset-example.sh` on that repo's orphan `example-reset` branch |

## Reset (Actions)

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

The repository variables the stories rely on (for example `UPLINK_REDACT_KEYWORDS`) are set in the setup's **Repository variables** step.
