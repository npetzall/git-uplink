# Story 7 — Assessment hook extras on the IP packet

Company scans that belong in the contribution packet run **after** `git uplink report` and **before** Environment **to-upstream**. Hooks are company-only, so they live on the orphan branch `uplink/hooks`, not on `main`. The forge pack ships a placeholder `.github/workflows/uplink-assessment-hook.yml` on `main`, because GitHub only dispatches workflows whose file is on the default branch. Submit's extras job runs the real hook with `--ref uplink/hooks` and prepends artifact `uplink-packet-extra` onto `assessment.md`.

This is the walkthrough for [templates/ghec/README.md](../../../templates/ghec/README.md) **Assessment hook**. The guide and starter YAML are in [`uplink-assessment-hook.md`](../../../templates/github/uplink-assessment-hook.md), which the pack installs as `.github/uplink-assessment-hook.md`.

```bash
export KIT=/path/to/git-uplink/examples/github
```

Work in the **internal** clone.

## Reset

**Actions → Reset example** on all three repos, then:

```bash
git uplink reset
git fetch origin --prune
git uplink status
```

Queue: only the internal-only tooling patch. Stories 01–06 do not use a hook.

## Create `uplink/hooks`

**Actions → Uplink assessment hook → Run workflow** on internal `main`. The run summary shows the setup guide. The run creates the orphan branch `uplink/hooks` with `assessment-hook.md`. Run it again later and it only updates that file when the guide changed.

## Add the hook

Pushing a workflow file needs **workflows** write (example org owner).

```bash
git fetch origin uplink/hooks
git switch uplink/hooks
mkdir -p .github/workflows
```

Copy the YAML block from `assessment-hook.md` to `.github/workflows/uplink-assessment-hook.yml`, then:

```bash
git add .github/workflows/uplink-assessment-hook.yml
git commit -m "Add Uplink assessment hook"
git push origin uplink/hooks
```

If you imported `uplink-hooks-ruleset.json`, push a topic branch and open a pull request against `uplink/hooks` instead.

```bash
git switch main
git uplink status
```

The queue does not change. The hook is not a patch, and company `main` only has the placeholder.

## Land Asha (upstream-bound)

```bash
git switch -C feat/sha256 main
git apply "$KIT/patches/asha-sha256.diff"
git add -A
git commit -m "Use SHA-256 for tokens"
git push -u origin feat/sha256
```

Open a PR. Title:

```text
Use SHA-256 for tokens
```

Body:

```text
Replace SHA-1 in the default hasher with SHA-256.

----- Uplink: internal below this line -----

Ticket: PROJ-1234
Uplink-Export-Author: Asha <asha@example.com>
```

Wait until **Uplink upstream assess** and **Uplink upstream preflight** are green. **Uplink upstream assess** also ran **Uplink assessment hook** with `pr`. The PR has one Uplink comment: `## Company review notes` sits above the assess report.

Edit the PR body (for example add a line above the cutoff). The checks run again and the same comment is updated; no second comment appears.

Merge. Import finds the last PR check run for the merged head commit, confirms it assessed the same title and body, and stores the hook's extras with the patch.

```bash
git uplink reset
git uplink status
```

Copy Asha’s patch id (`upl_` + 10 hex digits), then look at the stored extras:

```bash
git show "uplink/state:.uplink/reports/<id>/extras/10-company.md"
```

## Submit: extras first, then to-upstream

**Actions → Uplink submit → Run workflow** on internal `main`, input `patch_id` = Asha’s id.

The hook does **not** run again. The extras job’s summary says the company extras are stored for the unchanged patch. Open the packet job’s `GITHUB_STEP_SUMMARY` (or `.uplink/reports/<id>/assessment.md` on `uplink/state`): `## Company review notes` sits **above** `# Contribution packet`.

If the patch changes after import (for example a conflict is resolved), the stored extras no longer match and the extras job runs the hook with `patch` instead.

Then **Review deployments** → approve `to-upstream`. Full upstream merge and flow-back are [story 01](01-solo-fix.md).

## When the hook fails

Add `exit 1` to the hook's "Write company extras" step on `uplink/hooks`, then open any upstream-bound PR (or push to one). **Uplink upstream assess** keeps its own result: it shows a warning annotation, and the PR comment starts with **Uplink assessment hook failed** and a link to the hook run.

A failed result is not stored at import, so submit runs the hook for that patch. If it fails again, submit still continues and `assessment.md` starts with the same note. IP decides whether to approve.
