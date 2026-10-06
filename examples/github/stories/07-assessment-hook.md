# Story 7 — Assessment hook extras on the IP packet

Company scans that belong in the contribution packet run in **Uplink submit**, on the patch, **before** the packet is written and Environment **to-upstream** is asked. The same hook also runs on the internal PR, where its result is advice in a comment and is not kept. Hooks are company-only, so they live on the orphan branch `uplink/hooks`, not on `main`. The forge pack ships a placeholder `.github/workflows/uplink-assessment-hook.yml` on `main`, because GitHub only dispatches workflows whose file is on the default branch. Submit's extras job runs the real hook with `--ref uplink/hooks` and prepends artifact `uplink-packet-extra` onto `assessment.md`.

This is the walkthrough for [templates/github/README.md](../../../templates/github/README.md) **Assessment hook**. The guide is [`assessment-hook.md`](../../../templates/github-hooks/assessment-hook.md) and the starter is [`uplink-assessment-hook-example.yml`](../../../templates/github-hooks/.github/workflows/uplink-assessment-hook-example.yml). `git uplink init` put both on the orphan branch `uplink/hooks` during setup.

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

## Look at `uplink/hooks`

`git uplink init` created `uplink/hooks` during setup, and you pushed it with `main`. **Actions → Uplink assessment hook → Run workflow** on internal `main` prints `assessment-hook.md` from that branch to the run summary. `git uplink doctor` reports the branch as pushed.

## Add the hook

Pushing a workflow file needs **workflows** write (example org owner).

```bash
git switch uplink/hooks
git pull --ff-only origin uplink/hooks
cp .github/workflows/uplink-assessment-hook-example.yml .github/workflows/uplink-assessment-hook.yml
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

Merge. The PR comment was advice: import records the patch and stores nothing from the hook.

```bash
git uplink reset
git uplink status
```

Copy Asha’s patch id (`upl_` + 10 hex digits).

## Submit: extras first, then to-upstream

**Actions → Uplink submit → Run workflow** on internal `main`, input `patch_id` = Asha’s id.

The extras job runs **Uplink assessment hook** with `patch` set to Asha’s id and `state` set to the `uplink/state` commit to read it from. The packet job then assesses the patch file and writes the packet. Open its `GITHUB_STEP_SUMMARY` (or `.uplink/reports/<id>/assessment.md` on `uplink/state`): `## Company review notes` sits **above** `# Contribution packet`.

The result is now stored with the patch:

```bash
git fetch origin uplink/state
git show "origin/uplink/state:.uplink/reports/<id>/extras/10-company.md"
```

Dispatch **Uplink submit** again for the same patch and the hook does not run: the extras job’s summary says the company extras are stored for the unchanged patch. If the patch changes (for example a conflict is resolved), the stored extras no longer match and the hook runs again.

Then **Review deployments** → approve `to-upstream`. Full upstream merge and flow-back are [story 01](01-solo-fix.md).

## When the hook fails

Add `exit 1` to the hook's "Write company extras" step on `uplink/hooks`, then open any upstream-bound PR (or push to one). **Uplink upstream assess** keeps its own result: it shows a warning annotation, and the PR comment starts with **Uplink assessment hook failed** and a link to the hook run.

Dispatch **Uplink submit** while the hook still fails: submit continues and `assessment.md` starts with the same note. IP decides whether to approve. A failed result is never stored, so the next submit runs the hook again.

## Remove the hook

**Reset example** does not touch `uplink/hooks`, so the hook keeps running in later stories until you remove it. Revert every commit made on `uplink/hooks` since `git uplink init` created it. That brings back the stubs and removes the hook workflow:

```bash
git switch uplink/hooks
git pull --ff-only origin uplink/hooks
git revert --no-edit "$(git rev-list --max-parents=0 HEAD)..HEAD"
git push origin uplink/hooks
git switch main
```

`git rev-list --max-parents=0 HEAD` is the commit `init` made, and the range lists your commits newest first, which is the order `revert` needs. If you imported `uplink-hooks-ruleset.json`, push the reverts to a topic branch and open a pull request against `uplink/hooks` instead. Afterwards, `git ls-tree -r --name-only uplink/hooks` lists no `.github/workflows/uplink-assessment-hook.yml`, and PR checks skip the hook again.
