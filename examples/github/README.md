# GitHub example

A walkthrough of the Uplink operating model on three GitHub repositories. You `git apply` the patches in [`patches/`](patches/), push a branch, and open the PR in the GitHub UI. Actions import, submit, and sync.

How it works: [How](https://npetzall.github.io/git-uplink/how). What developers do: [Day to day](https://npetzall.github.io/git-uplink/day-to-day). Set it up step by step: [Try it yourself → Setup](https://npetzall.github.io/git-uplink/examples?view=setup). Kit files and resets: [`SETUP.md`](SETUP.md).

## Repositories

| Name | Role |
| --- | --- |
| `uplink-example-upstream` | Public tokenkit project |
| `uplink-example-upstream-contrib` | Fork used as the contrib remote |
| `uplink-example-internal` | Company product (Actions live here) |

Company `main` is always:

```text
public upstream/main  +  tooling  +  active upstream[]  +  active internal[]
```

After setup, the tooling slot already holds **Uplink tooling** (GitHub workflows and the PR template). Product stories start from that baseline.

## How a story step works

**Git (your clones):** branch, `git apply`, commit, `git push`. After import or Actions reset: `git uplink reset`, `git fetch origin --prune`, then `git switch -C <branch> main` for feature branches. `git uplink status` for the queue.

**GitHub UI (not the file editor):** Compare & pull request, copy the title and body code blocks from the story, add labels (`uplink:internal-only` when needed), merge after checks, Actions (**Reset example**, **Uplink submit**, **Uplink sync**), Review deployments for `to-upstream` (export) and `from-upstream` (inbound foreign commits), merge the upstream PR.

## Stories

Reset all three repos at the start of each story: **Actions → Reset example** (see [`SETUP.md`](SETUP.md)). Then `git uplink reset`, `git fetch origin --prune`, and `git uplink status` in the internal clone.

| Story | What you exercise |
| --- | --- |
| [01 — Solo fix](stories/01-solo-fix.md) | Asha SHA-256: PR, import, to-upstream submit, upstream merge, drop-on-merge |
| [02 — Parallel independent](stories/02-parallel-independent.md) | Asha hash + Ben TTL; merge Ben first; Asha stays queued |
| [03 — Stacked depends-on](stories/03-stacked-depends-on.md) | Ben log needs Asha; preflight without the trailer; submit order |
| [04 — Upstream conflict](stories/04-upstream-conflict.md) | Sync conflict gated PR, work on `-work`, merge, rebuild |
| [05 — Cam on two siblings](stories/05-cam-two-deps.md) | Cam depends on Asha and Ben; wait until both merge before submitting Cam |
| [06 — Internal-only](stories/06-internal-only.md) | Telemetry patch never goes through to-upstream / submit |
| [07 — Assessment hook](stories/07-assessment-hook.md) | Hook on uplink/hooks; one updated PR comment; extras stored at import and reused at submit |

## Layout

```text
examples/github/
  SETUP.md              kit files and resets (setup steps are on the site)
  example-reset.yml     Reset example workflow, copied into each repo
  reset/                per-repo reset scripts, copied onto each repo's example-reset branch
  upstream/             tokenkit, the first commit of uplink-example-upstream
  patches/*.diff        git apply these
  patches/*.yml         copy into the internal clone (assessment hook)
  stories/
```

Actions jobs download the `git-uplink` release named by the `UPLINK_SRC` / `UPLINK_VERSION` repository variables; nothing is compiled.
