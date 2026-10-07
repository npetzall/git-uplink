# Try it on GitHub (`--forge try-it-on-github`)

`git uplink init --forge try-it-on-github` installs the [GitHub pack](../github/README.md) into `uplink-example-internal` as the tooling patch, adjusted for a walkthrough. For setting up the example step by step, see [Try it yourself](https://npetzall.github.io/git-uplink/examples?view=setup).

This folder is an overlay on [`../github/`](../github/): a file here with the same path replaces the one in the GitHub pack, a new path is added, and `Forge::omitted_paths` in `src/types.rs` lists the files left out. Today it only leaves one out:

- **No `uplink-sync-schedule.yml`.** Nothing dispatches **Uplink sync** hourly. You dispatch it when a story says so, so nothing races the walkthrough.

The workflows themselves are identical. The setup sets the repository variables `UPLINK_INTERNAL_AUTH`, `UPLINK_UPSTREAM_AUTH`, and `UPLINK_CONTRIB_AUTH` to `pat`, so each role is one fine-grained token (`UPLINK_*_TOKEN`) instead of a GitHub App, which is the pack's default. It leaves `UPLINK_AUTO_SUBMIT` unset, so you dispatch every **Uplink submit** yourself, as the stories say.

| Workflow | In the walkthrough | Needs |
| --- | --- | --- |
| `uplink-pr.yml` — Uplink PR checks | Runs on every story PR to `main`: **Uplink upstream assess** and **Uplink upstream preflight** | `uplink.toml` and `preflight.sh` on `uplink/hooks` (written by `init` in the setup); labels `uplink:internal-only`, `uplink:rebase` |
| `uplink-rebase.yml` — Uplink rebase | Add the label `uplink:rebase` to a story PR whose `main` a sync has replaced, or set `UPLINK_AUTO_REBASE` to `true`; the branch is rebased and pushed | `UPLINK_INTERNAL_TOKEN`; label `uplink:rebase` |
| `uplink-import.yml` — Uplink import | Runs when you merge a story PR; records the patch as `queued` | `UPLINK_INTERNAL_TOKEN` |
| `uplink-submit.yml` — Uplink submit | **Actions → Uplink submit** with a patch id; stops early if an upstream dependency is not merged; waits for your approval on `to-upstream`, then GitHub creates the signed commit on the fork (`.github/uplink/contrib_commit.py`) and the upstream PR is opened. With a PAT the commit is not shown as Verified | `UPLINK_CONTRIB_TOKEN` on Environment `to-upstream`; `UPLINK_UPSTREAM_TOKEN` |
| `uplink-sync.yml` — Uplink sync | **Actions → Uplink sync** after upstream moves; foreign commits wait for your approval on `from-upstream`; a conflict opens a gated PR | `UPLINK_INTERNAL_TOKEN`, `UPLINK_UPSTREAM_TOKEN`; Environment `from-upstream`; label `uplink:conflict` |
| `uplink-resolve.yml` — Uplink resolve | Runs when you merge a gated conflict PR (story 04) | `UPLINK_INTERNAL_TOKEN`, `UPLINK_UPSTREAM_TOKEN`; label `uplink:conflict` |
| `uplink-verify.yml` — Uplink verify | **Actions → Uplink verify** when `main` is broken; runs `preflight.sh` on `main` as rebuilt and opens a gated PR for the first patch it fails on | `UPLINK_INTERNAL_TOKEN`, `UPLINK_UPSTREAM_TOKEN`; label `uplink:conflict` |
| `uplink-transfer.yml` — Uplink transfer | **Actions → Uplink transfer** to move a patch between queues | `UPLINK_INTERNAL_TOKEN`, `UPLINK_UPSTREAM_TOKEN`; labels `uplink:transfer-to-upstream`, `uplink:transfer-to-internal` |
| `uplink-amend.yml` — Uplink amend | **Actions → Uplink amend** with a patch id opens a draft PR to revise it; merging updates the patch (and re-submits a submitted one) | `UPLINK_INTERNAL_TOKEN`, `UPLINK_UPSTREAM_TOKEN`; label `uplink:amend` |
| `uplink-abandon.yml` — Uplink abandon contrib | Dispatched by transfer after moving a submitted patch to internal; waits on `abandon-contrib` | `UPLINK_CONTRIB_TOKEN` on Environment `abandon-contrib`; `UPLINK_UPSTREAM_TOKEN` |
| `uplink-gate.yml` — Uplink gate | Required check on gated conflict, transfer, and amend PRs | `preflight.sh` on `uplink/hooks` |

The internal token opens the gated conflict, transfer, and amend PRs, so it needs **Pull requests: read and write** besides Contents and Workflows.

## Reset example

Not part of the pack. [`examples/github/example-reset.yml`](../../examples/github/example-reset.yml) is copied into the example repositories during setup, and each repository's reset script from [`examples/github/reset/`](../../examples/github/reset/) is published on the orphan branch `example-reset`.

- **Runs on:** manual dispatch (**Actions → Reset example**) at the start of every story.
- **Does:** checks out `example-reset` and runs `scripts/reset-example.sh`. That closes open PRs, deletes branches the story created, and moves `main` (and on internal, `uplink/state` and `uplink/upstream`) back to the seed refs. Force-push is expected.
- **Requires:** the Actions token (contents, pull requests, and issues write).
