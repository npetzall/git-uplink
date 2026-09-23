# Story 1 — Asha starts a new fix

Asha changes token hashing. Nobody else is in the way. This is [way-of-working.md](../../../way-of-working.md) story 1: internal PR → import → to-upstream submit → upstream merge → sync drops the patch.

```bash
export KIT=/path/to/git-uplink/examples/github
```

Work in your **internal** clone unless a step says otherwise.

## Reset

On GitHub, **Actions → Reset example → Run workflow** on all three repos (any branch; the job checks out `example-reset`). Then:

```bash
git uplink reset
git fetch origin --prune
git uplink status
```

Queue: only the internal-only tooling patch.

## Apply the patch and push

```bash
git switch -C feat/sha256 main
git apply "$KIT/patches/asha-sha256.diff"
git add -A
git commit -m "Use SHA-256 for tokens"
git push -u origin feat/sha256
```

On GitHub, open **Compare & pull request** for `feat/sha256` into `main`. Title:

```text
Use SHA-256 for tokens
```

Body:

```text
Replace SHA-1 in the default hasher with SHA-256.

----- Uplink: internal below this line -----

Ticket: PROJ-1234
Uplink-Export-Author: Asha <asha@users.noreply.github.com>
```

Wait until **Uplink upstream assess** and **Uplink upstream preflight** are green. The first run compiles git-uplink and is slow.

## Import (product gate)

On the PR, merge after review. Watch **Uplink import**. Then:

```bash
git uplink reset
git uplink status
```

Asha’s patch is `queued` (`upl_` + 10 hex digits). `src/tokens.js` on `main` calls `sha256`. Copy the id.

## Submit (IP gate)

**Actions → Uplink submit → Run workflow** on internal `main`, input `patch_id` = that id.

The packet job writes `.uplink/reports/<id>/assessment.md` on `uplink/state`. The submit job waits on Environment **to-upstream**. Open the run → **Review deployments** → approve `to-upstream`.

After it finishes, `git uplink status` shows `submitted`. GitHub has a PR from `uplink-example-upstream-contrib` (`uplink/<id>`) into `uplink-example-upstream`.

## Merge upstream and flow back

On that **upstream** PR, squash-merge in the GitHub UI.

**Actions → Uplink sync → Run workflow** on internal `main`. The squash-merge is Asha’s trailer, so inspect applies immediately (no `from-upstream` wait).

```bash
git uplink reset
git uplink status
```

Asha’s patch is `merged` and is not applied internally anymore. Company `main` has SHA-256 because it is on upstream.

## Optional: maintainer follow-up

In the **upstream** clone:

```bash
git fetch origin --prune
git switch -C main origin/main
git apply "$KIT/patches/upstream-salt-hash.diff"
git commit -am "follow-up: salt the hash"
git push origin main
```

Run **Uplink sync** again. The salt commit is not a company patch, so the apply job waits on Environment **from-upstream**. Approve it. Company `main` then has `saltedSha256` and does **not** re-apply Asha’s old `return sha256(value)`.
