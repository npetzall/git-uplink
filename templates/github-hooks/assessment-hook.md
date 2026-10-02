# Uplink assessment hook

The assessment hook runs the company checks whose results belong in the contribution packet. IP reads its markdown before approving.

Hooks are company-only. They live on the orphan branch `uplink/hooks`, never on `main`, so they are never queued, replayed, or contributed.

## When it runs

1. **On the PR.** Uplink PR checks run the hook with `pr` on every open, push, reopen and edit. The hook's markdown and the built-in assess report share one PR comment, which is updated in place instead of adding a new one each run.
2. **At import.** When the PR merges, import keeps the hook result from the last PR check run if it was made for exactly what was merged: the same head commit, title and body. It is stored with the patch on `uplink/state` under `.uplink/reports/<id>/extras/`.
3. **At submit.** Submit uses the stored result while the patch content is unchanged. It runs the hook again with `patch` only when nothing is stored, the stored result is out of date (for example after a conflict was resolved), or the patch was amended. A successful run at submit is stored for later packets.

A failed hook never fails the PR check or submit. It shows as a warning, and the comment or `assessment.md` starts with a note that links to the failed run. IP decides. A failed result is never stored, so the next submit runs the hook again.

## How it is wired

- `git uplink init` creates `uplink/hooks` locally with this file, `toolchain-hook.md`, the toolchain hook stub, and a starter assessment hook. `git uplink push` publishes it with `uplink/state`. `git uplink init --upgrade` adds files a newer pack brings, without changing the ones you have. `git uplink doctor` reports when it is missing or not pushed.
- The forge pack installs a placeholder `.github/workflows/uplink-assessment-hook.yml` on `main`. GitHub only dispatches a workflow whose file exists on the default branch, so the placeholder has to be there. Running it by hand from the Actions tab prints this file from `uplink/hooks`.
- Your real hook is `.github/workflows/uplink-assessment-hook.yml` on `uplink/hooks`. Callers look for it there and run it with `--ref uplink/hooks`. If it is missing, they skip the hook.

## Contract

- Trigger on `workflow_dispatch` with these inputs:
  - `pr`: the internal pull request number, or empty;
  - `patch`: the patch id (`upl_…`), or empty;
  - `caller_run_id`: always set.

  At least one of `pr` and `patch` is set. A PR check sets `pr`. Submit sets `patch`. An amendment or conflict-resolution PR for a patch that already exists may set both.
- Put `caller_run_id` in `run-name`. Callers use it to find the run they dispatched.
- With `pr`, assess `refs/pull/<pr>/head`. Otherwise assess `main`.
- Upload `*.md` files as artifact `uplink-packet-extra`. Files are prepended in name order. An empty result is fine. uplink adds no headings or separators. Each file's markdown goes in as-is, so include your own heading and use a prefix such as `10-`, `20-` to set the order.
- Do not push `uplink/state`. Uplink stores the result itself.

**With `pr`, the checked-out code is not merged or reviewed yet.** Do not build or run it in a job that has secrets. Scan it as data.

## Add the hook

Copy `.github/workflows/uplink-assessment-hook-example.yml` to `.github/workflows/uplink-assessment-hook.yml` on `uplink/hooks`. The `-example` name keeps the starter from running. Change its "Write company extras" step to run your checks. A push that adds or changes a workflow file needs `workflows` write, so push with your own account or open a pull request against `uplink/hooks`.

## Protect the branch

Code on `uplink/hooks` runs on every PR check and submit. Import `.github/uplink-hooks-ruleset.json` from `main` as a repository ruleset. Changes to `uplink/hooks` then need a reviewed pull request.
