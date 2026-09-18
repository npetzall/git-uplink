# Story 4 — Upstream conflicts with a queued patch

Ben’s TTL patch is queued. Upstream then changes the same lines. Sync stops; Ben is `conflict`; you resolve on `uplink/conflict/<id>`, not a PR. This is [way-of-working.md](../../../way-of-working.md) story 4.

```bash
export KIT=/path/to/git-uplink/examples/github
```

## Reset

**Actions → Reset example** on all three repos, then in **internal**:

```bash
git fetch origin
git checkout main
git reset --hard origin/main
git fetch origin '+refs/heads/uplink/state:refs/heads/uplink/state'
git fetch origin '+refs/heads/uplink/upstream:refs/heads/uplink/upstream'
```

In **upstream**:

```bash
git fetch origin && git checkout main && git reset --hard origin/main
```

## Import Ben’s TTL

In **internal**:

```bash
git checkout -b feat/ttl
git apply "$KIT/patches/ben-ttl.diff"
git add -A && git commit -m "Extend TTL"
git push -u origin feat/ttl
```

Open the PR in the GitHub UI; paste [`ben-ttl.pr.md`](../patches/ben-ttl.pr.md). Wait for checks. Merge.

```bash
git fetch origin && git reset --hard origin/main
git fetch origin '+refs/heads/uplink/state:refs/heads/uplink/state'
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

**Actions → Uplink sync** on internal. Inspect finds the shorten-ttl commit is not a company patch, so the apply job waits on Environment **from-upstream**. Approve that deployment. After apply, the run exits 2. Internal gets:

- Ben’s status `conflict` on `uplink/state`
- branch `uplink/conflict/<id>` with markers
- issue labeled `uplink:conflict`

**Do not open a pull request for the conflict.** Company `main` stays at the last successful rebuild (still 7200, no markers).

```bash
git fetch origin
git fetch origin '+refs/heads/uplink/state:refs/heads/uplink/state'
git fetch origin '+refs/heads/uplink/conflict/*:refs/heads/uplink/conflict/*'
git uplink status
```

## Resolve on the conflict branch

In **internal**:

```bash
id=upl_YOUR_ID
git fetch origin
git checkout "uplink/conflict/${id}"
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
git push origin "uplink/conflict/${id}"
```

**Uplink resolve** runs on that push. It refreshes the same patch id, rebuilds `main`, closes the issue, and deletes the conflict branch.

```bash
git fetch origin && git checkout main && git reset --hard origin/main
git fetch origin '+refs/heads/uplink/state:refs/heads/uplink/state'
git uplink status
```

Ben is `queued` again. `src/tokens.js` has `return 7200` and not `1800`.
