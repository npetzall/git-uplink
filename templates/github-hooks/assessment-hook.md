# Uplink assessment hook

The assessment hook runs the company checks whose results belong in the contribution packet. IP reads its markdown before approving.

Hooks are company-only. They live on the orphan branch `uplink/hooks`, never on `main`, so they are never queued, replayed, or contributed.

## When it runs

1. **On the PR, as advice.** Uplink PR checks run the hook with `pr` on every open, push, reopen and edit. The hook's markdown and the built-in assess report share one PR comment, which is updated in place instead of adding a new one each run. Nothing from this run is stored: the change is not a patch yet, and import does not copy the result.
2. **At submit, for the packet.** Uplink submit runs the hook with `patch`. This is the result IP reads. A successful run is stored with the patch on `uplink/state` under `.uplink/reports/<id>/extras/` and reused by later submit runs while the patch content is unchanged. The hook runs again when nothing is stored, the stored result is out of date (for example after a conflict was resolved), or the patch was amended.

A failed hook never fails the PR check or submit. It shows as a warning, and the comment or `assessment.md` starts with a note that links to the failed run. IP decides. A failed result is never stored, so the next submit runs the hook again.

If the patch changes while the hook runs at submit, the result is for other content. Submit compares the patch with the one at the `state` commit the hook was given, and stops when they differ; dispatch it again.

## How it is wired

- `git uplink init` creates `uplink/hooks` locally with this file, `toolchain-hook.md`, the toolchain hook stub, `preflight.sh`, a starter assessment hook, and `uplink.toml` (the CLI's settings). `git uplink push` publishes it with `uplink/state`. `git uplink init --upgrade` adds files a newer pack brings, without changing the ones you have. `git uplink doctor` reports when it is missing or not pushed.
- The forge pack installs a placeholder `.github/workflows/uplink-assessment-hook.yml` on `main`. GitHub only dispatches a workflow whose file exists on the default branch, so the placeholder has to be there. Running it by hand from the Actions tab prints this file from `uplink/hooks`.
- Your real hook is `.github/workflows/uplink-assessment-hook.yml` on `uplink/hooks`. Callers look for it there and run it with `--ref uplink/hooks`. If it is missing, they skip the hook.

## Contract

- Trigger on `workflow_dispatch` with these inputs:

  | Input | Set by | What it names | Where the result goes |
  | --- | --- | --- | --- |
  | `pr` | Uplink PR checks | The internal pull request number. The change is `refs/pull/<pr>/head`. | The PR comment only |
  | `patch` | Uplink submit | The patch id (`upl_…`). | The contribution packet, and stored with the patch |
  | `state` | Uplink submit, with `patch` | The commit of `uplink/state` to read the patch from: `.uplink/patches/<patch>.patch` at that commit. | — |
  | `caller_run_id` | both | The run that dispatched the hook. | — |

  Exactly one of `pr` and `patch` is set. It tells the hook what it is working with, and whether its result is advice or goes to IP.
- Put `caller_run_id` in `run-name`. Callers use it to find the run they dispatched.
- With `patch`, read the patch at `state`, not at the tip of `uplink/state`. The branch can move while the hook runs, and submit refuses a result when the patch is no longer what it was at that commit:

  ```bash
  git fetch origin "$STATE"
  git show "$STATE:.uplink/patches/$PATCH.patch"
  ```
- What the hook does with it is yours to decide: look up a ticket, scan the patch file, apply it on `uplink/upstream` and scan the tree. Uplink passes nothing else.
- Declare all four inputs. GitHub rejects a dispatch with an input the workflow does not declare, and a rejected dispatch shows as a failed hook.
- Upload `*.md` files as artifact `uplink-packet-extra`. Files are prepended in name order. An empty result is fine. uplink adds no headings or separators. Each file's markdown goes in as-is, so include your own heading and use a prefix such as `10-`, `20-` to set the order.
- Do not push `uplink/state`. Uplink stores the result itself.

**With `pr`, the checked-out code is not merged or reviewed yet.** Do not build or run it in a job that has secrets. Scan it as data.

## Add the hook

Copy `.github/workflows/uplink-assessment-hook-example.yml` to `.github/workflows/uplink-assessment-hook.yml` on `uplink/hooks`. The `-example` name keeps the starter from running. Change its "Write company extras" step to run your checks. A push that adds or changes a workflow file needs `workflows` write, so push with your own account or open a pull request against `uplink/hooks`.

## Protect the branch

Code on `uplink/hooks` runs on every PR check and submit. Import `.github/uplink-hooks-ruleset.json` from `main` as a repository ruleset. Changes to `uplink/hooks` then need a reviewed pull request.
