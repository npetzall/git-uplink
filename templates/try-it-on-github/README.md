# GitHub example workflows (`--forge example-github`)

`git uplink init --forge example-github` installs the workflows in [`.github/workflows/`](.github/workflows/) into `uplink-example-internal` as the tooling patch, with the shared [pull request template](../github/pull_request_template.md) and [`install-git-uplink`](../github/actions/install-git-uplink/action.yml) action. For setting up the example step by step, see [Try it yourself](https://npetzall.github.io/git-uplink/examples?view=setup).

They are the [GitHub Enterprise Cloud workflows](../ghec/README.md) with two differences for a walkthrough:

- `UPLINK_*_AUTH` defaults to `pat`, so each role is one fine-grained token (`UPLINK_*_TOKEN`) instead of a GitHub App.
- **Uplink sync** has no hourly schedule. You dispatch it when a story says so, so nothing races the walkthrough.

| Workflow | In the walkthrough | Needs |
| --- | --- | --- |
| `uplink-pr.yml` — Uplink PR checks | Runs on every story PR to `main`: **Uplink upstream assess** and **Uplink upstream preflight** | Variables `UPLINK_REDACT_KEYWORDS`, `UPLINK_INTERNAL_DOMAINS`, `UPLINK_PREFLIGHT`; label `uplink:internal-only` |
| `uplink-import.yml` — Uplink import | Runs when you merge a story PR; records the patch as `queued` | `UPLINK_INTERNAL_TOKEN` |
| `uplink-submit.yml` — Uplink submit | **Actions → Uplink submit** with a patch id; stops early if an upstream dependency is not merged; waits for your approval on `to-upstream`, then GitHub creates the signed commit on the fork (`.github/uplink/contrib_commit.py`) and the upstream PR is opened. With a PAT the commit is not shown as Verified | `UPLINK_CONTRIB_TOKEN` on Environment `to-upstream`; `UPLINK_UPSTREAM_TOKEN` |
| `uplink-sync.yml` — Uplink sync | **Actions → Uplink sync** after upstream moves; foreign commits wait for your approval on `from-upstream`; a conflict opens a gated PR | `UPLINK_INTERNAL_TOKEN`, `UPLINK_UPSTREAM_TOKEN`; Environment `from-upstream`; label `uplink:conflict` |
| `uplink-resolve.yml` — Uplink resolve | Runs when you merge a gated conflict PR (story 04) | `UPLINK_INTERNAL_TOKEN`, `UPLINK_UPSTREAM_TOKEN`; label `uplink:conflict` |
| `uplink-transfer.yml` — Uplink transfer | **Actions → Uplink transfer** to move a patch between queues | `UPLINK_INTERNAL_TOKEN`, `UPLINK_UPSTREAM_TOKEN`; labels `uplink:transfer-to-upstream`, `uplink:transfer-to-internal` |
| `uplink-amend.yml` — Uplink amend | **Actions → Uplink amend** with a patch id opens a draft PR to revise it; merging updates the patch (and re-submits a submitted one) | `UPLINK_INTERNAL_TOKEN`, `UPLINK_UPSTREAM_TOKEN`; label `uplink:amend` |
| `uplink-abandon.yml` — Uplink abandon contrib | Dispatched by transfer after moving a submitted patch to internal; waits on `abandon-contrib` | `UPLINK_CONTRIB_TOKEN` on Environment `abandon-contrib`; `UPLINK_UPSTREAM_TOKEN` |
| `uplink-gate.yml` — Uplink gate | Required check on gated conflict, transfer, and amend PRs | Variable `UPLINK_PREFLIGHT` |

The internal token opens the gated conflict, transfer, and amend PRs, so it needs **Pull requests: read and write** besides Contents and Workflows.

## Reset example

Not part of the pack. [`examples/github/example-reset.yml`](../../examples/github/example-reset.yml) is copied into the example repositories during setup, and each repository's reset script from [`examples/github/reset/`](../../examples/github/reset/) is published on the orphan branch `example-reset`.

- **Runs on:** manual dispatch (**Actions → Reset example**) at the start of every story.
- **Does:** checks out `example-reset` and runs `scripts/reset-example.sh`. That closes open PRs, deletes branches the story created, and moves `main` (and on internal, `uplink/state` and `uplink/upstream`) back to the seed refs. Force-push is expected.
- **Requires:** the Actions token (contents, pull requests, and issues write).
