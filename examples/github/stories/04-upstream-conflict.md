# Story 4 — Upstream conflicts with a queued patch

Ben’s TTL patch is queued. Upstream then changes the same lines. Sync stops; Ben is `conflict`; you resolve on `uplink/conflict/<id>`, not a PR. This is [way-of-working.md](../../../way-of-working.md) story 4.

```bash
export KIT=/path/to/git-uplink/examples/github
```

## Reset

**Actions → Reset example** on all three repos, then in **internal**:

```bash
git uplink reset
git fetch origin --prune
git uplink status
```

In **upstream**:

```bash
git fetch origin --prune
git switch -C main origin/main
```

## Import Ben’s TTL

In **internal**:

```bash
git switch -C feat/ttl main
git apply "$KIT/patches/ben-ttl.diff"
git add -A && git commit -m "Extend TTL"
git push -u origin feat/ttl
```

Open the PR in the GitHub UI. Title:

```text
Extend TTL
```

Body:

```text
Extend the default token TTL from one hour to two.

----- Uplink: internal below this line -----

Ticket: PROJ-2001
Uplink-Export-Author: Ben <ben@example.com>
```

Wait for checks. Merge.

```bash
git uplink reset
git uplink status
```

Note Ben’s `upl_…` id. Company `main` still has `ttl() == 7200` and no conflict markers.

## Move upstream so the patch no longer applies

In the **upstream** clone:

```bash
git apply "$KIT/patches/upstream-shorten-ttl.diff"
git commit -am "shorten default ttl"
git push origin main
```

**Actions → Uplink sync** on internal. Inspect finds the shorten-ttl commit is not a company patch, so the apply job waits on Environment **from-upstream**. Approve that deployment. Apply succeeds and opens a gated PR (the run stays green). Internal gets:

- Ben’s status `conflict` on `uplink/state`
- protected base `uplink/conflict/<id>` (apply prefix) and `uplink/conflict/<id>-work` (conflict markers)
- a gated PR from `-work` into the base, labeled `uplink:conflict`

Company `main` stays at the last successful rebuild (still 7200, no markers). Work on `-work` only; merge is the only update to the base.

```bash
git uplink reset
git fetch origin '+refs/heads/uplink/conflict/*:refs/heads/uplink/conflict/*'
git uplink status
```

## Resolve on the work branch

In **internal**:

```bash
id=upl_YOUR_ID
git fetch origin
git checkout "uplink/conflict/${id}-work"
```

If the file still has conflict markers, keep Ben’s 7200 on the new upstream:

```bash
python3 - <<'PY'
from pathlib import Path
p = Path("src/tokens.js")
text = p.read_text()
if "<<<<<<" in text:
    text = "".join(
        line for line in text.splitlines(True)
        if not line.startswith(("<<<<<<<", "=======", ">>>>>>>"))
    )
text = text.replace("return 1800;", "return 7200;").replace("return 3600;", "return 7200;")
p.write_text(text)
PY
git add src/tokens.js
git commit -m "Resolve ttl onto the new upstream"
git push origin "uplink/conflict/${id}-work"
```

Merge the gated PR into `uplink/conflict/${id}`. **Uplink resolve** runs on that merge. It refreshes the same patch id, rebuilds `main`, and deletes both conflict branches.

```bash
git uplink reset
git uplink status
```

Ben is `queued` again. `src/tokens.js` has `return 7200` and not `1800`.
