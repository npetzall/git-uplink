# GitHub example

A walkthrough of the Uplink operating model on three GitHub repositories. You `git apply` the patches in [`patches/`](patches/), push a branch, and open the PR in the GitHub UI. Actions import, submit, and sync.

Rationale for each flow: [`way-of-working.md`](../../way-of-working.md). Setup, tokens, and the `oss` Environment: [`SETUP.md`](SETUP.md).

## Repositories

| Name | Role |
| --- | --- |
| `uplink-example-upstream` | Public tokenkit project |
| `uplink-example-upstream-contrib` | Fork used as the contrib remote |
| `uplink-example-internal` | Company product (Actions live here) |

Company `main` is bot-owned:

```text
public upstream/main  +  every patch that is not merged or dropped
```

After bootstrap, the queue already has one **internal-only** patch (the GitHub workflows). Product stories start from that baseline.

## How a story step works

**Git (your clones):** branch, `git apply`, commit, `git push`. After import or reset: `git fetch` and `git reset --hard origin/main`. `git uplink status` for the queue.

**GitHub UI (not the file editor):** Compare & pull request, paste the matching [`patches/*.pr.md`](patches/) into the body, add labels, Actions (**Reset example**, **Uplink submit**, **Uplink sync**), Review deployments for `oss`, merge the upstream PR.

## Stories

Reset all three repos at the start of each story: **Actions → Reset example** (see [`SETUP.md`](SETUP.md)). Then fetch in your clones.

| Story | What you exercise |
| --- | --- |
| [01 — Solo fix](stories/01-solo-fix.md) | Asha SHA-256: PR, import, oss submit, upstream merge, drop-on-merge |
| [02 — Parallel independent](stories/02-parallel-independent.md) | Asha hash + Ben TTL; merge Ben first; Asha stays queued |
| [03 — Stacked depends-on](stories/03-stacked-depends-on.md) | Ben log needs Asha; preflight without the trailer; submit order |
| [04 — Upstream conflict](stories/04-upstream-conflict.md) | Sync conflict issue, resolve branch, rebuild |
| [05 — Cam on two siblings](stories/05-cam-two-deps.md) | Cam depends on Asha and Ben; wait until both merge before submitting Cam |
| [06 — Internal-only](stories/06-internal-only.md) | Telemetry patch never goes through oss / submit |

## Layout

```text
examples/github/
  SETUP.md
  upstream/             tokenkit + upstream Reset example
  upstream-contrib/     contrib Reset example (delete extra branches)
  internal/             Uplink Actions + internal Reset example
  patches/*.diff        git apply these
  patches/*.pr.md       paste into the GitHub PR body
  scripts/bootstrap_*.sh  seed upstream, then contrib fork, then internal
  stories/
```

The first Actions run on a PR compiles git-uplink (Rust 1.98 + Node 22). Later runs hit the cargo cache.
