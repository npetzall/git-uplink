# Uplink assessment hook

The assessment hook runs the company checks whose results belong in the contribution packet. IP reads its markdown before approving.

Hooks are company-only. They live on the orphan branch `uplink/hooks`, never on `main`, so they are never queued, replayed, or contributed.

## When it runs

1. **On the PR.** Uplink PR checks run the hook with `pr` on every open, push, reopen and edit. The hook's markdown and the built-in assess report share one PR comment, which is updated in place instead of adding a new one each run.
2. **At import.** When the PR merges, import keeps the hook result from the last PR check run if it was made for exactly what was merged: the same head commit, title and body. It is stored with the patch on `uplink/state` under `.uplink/reports/<id>/extras/`.
3. **At submit.** Submit uses the stored result while the patch content is unchanged. It runs the hook again with `patch` only when nothing is stored, the stored result is out of date (for example after a conflict was resolved), or the patch was amended. A successful run at submit is stored for later packets.

A failed hook never fails the PR check or submit. It shows as a warning, and the comment or `assessment.md` starts with a note that links to the failed run. IP decides. A failed result is never stored, so the next submit runs the hook again.

## How it is wired

- The forge pack installs a placeholder `.github/workflows/uplink-assessment-hook.yml` on `main`. GitHub only dispatches a workflow whose file exists on the default branch, so the placeholder has to be there. Run it by hand from the Actions tab. It creates `uplink/hooks` if the branch is missing, and writes this file there as `assessment-hook.md`.
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

Save this workflow to `.github/workflows/uplink-assessment-hook.yml` on `uplink/hooks`. Change the "Write company extras" step to run your checks. A push that adds or changes a workflow file needs `workflows` write, so push with your own account or open a pull request against `uplink/hooks`.

```yaml
name: Uplink assessment hook
run-name: Uplink assessment hook ${{ inputs.pr }} ${{ inputs.patch }} ${{ inputs.caller_run_id }}

on:
  workflow_dispatch:
    inputs:
      pr:
        description: Internal pull request number
        required: false
        type: string
      patch:
        description: Patch id (upl_…)
        required: false
        type: string
      caller_run_id:
        description: Caller run id (used to match this hook run)
        required: true
        type: string

permissions:
  contents: read

jobs:
  extra:
    if: github.ref == 'refs/heads/uplink/hooks'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0
          persist-credentials: false
          ref: ${{ inputs.pr && format('refs/pull/{0}/head', inputs.pr) || 'main' }}

      - name: Write company extras
        env:
          PR: ${{ inputs.pr }}
          PATCH: ${{ inputs.patch }}
        run: |
          set -euo pipefail
          mkdir -p extras
          subject=${PATCH:-PR #${PR}}
          cat > extras/10-company.md <<EOF
          ## Company review notes

          Extra details for ${subject}. Replace this step with Jira,
          license, or classification output.
          EOF

      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        with:
          name: uplink-packet-extra
          path: extras/*.md
          if-no-files-found: error
```

## Protect the branch

Code on `uplink/hooks` runs on every PR check and submit. Import `.github/uplink-hooks-ruleset.json` as a repository ruleset. Changes to `uplink/hooks` then need a reviewed pull request. GitHub Actions can still push, so the placeholder can keep this file current. The Actions token can never change a workflow file, so it cannot change the hook itself.
