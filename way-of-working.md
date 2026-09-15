# Way of working

This is how developers use Uplink day to day. Company `main` is bot-owned. You open one internal PR per change. You never maintain a second branch for upstream. Operators can walk the model with `git uplink web-ui`.

Read this alongside `.uplink/queue.json` and `git uplink status`. The binary is `git-uplink` (a Git subcommand). The queue is the source of truth for what company `main` is made of. Git history on `main` is rebuilt and may be force-updated; do not treat it as a human commit log.

Write every change **as if it were the upstream submission**. Company-only details (issue ids, internal reviewers, export-author override) go **below the cutoff** in the commit message. `git commit` uses `.uplink/commit-msg.template` after `git uplink init`. That template does not turn off your commit signing: `git uplink` keeps bot identity and unsigned commits on the subprocess only. Network commands (`add --push`, `sync`, `submit`) use `UPLINK_GITHUB_TOKEN` / `GITHUB_TOKEN` or `UPLINK_SSH_KEY`, not your default SSH key.

| Phase | What you do | Result |
| --- | --- | --- |
| Write | Branch from company `main`. Public rationale above the cutoff. | One internal branch. |
| Prepare | Opens with the internal PR. `git uplink prepare` | Scrubbed message, rewritten author, affiliation scan. Report on the PR and in `GITHUB_STEP_SUMMARY`. |
| Export preflight | Same PR | Diff must stand on public `main` + declared deps; `UPLINK_PREFLIGHT` must pass. |
| Internal product | Review + `uplink:import` | Status `queued`. Product builds it. |
| Contribution / IP | Dispatch **Uplink submit**. IP approves GitHub Environment **`oss`**. Same run submits. | Report + approval committed under `.uplink/reports/`. Public PR uses the prepared identity. GitHub audit log records the reviewer. |

Default intent is upstream. `uplink:internal-only` is the escape hatch and never goes through the second gate.

## Contribution approval via the oss GitHub Environment

On GitHub Enterprise Cloud this is an option, and it is the option to use. Do not invent a second spreadsheet for IP sign-off.

Create a repository Environment named **`oss`**. Required reviewers are IP/legal. Put the GitHub App secrets that can push the upstream-owned private fork **on that environment only**. Dispatch `Uplink submit` with the patch id. The workflow:

1. **Packet job** (no environment, no App secrets). Runs `git uplink report <id>`, which writes `.uplink/reports/<id>/prepare.md` and appends the same markdown to `GITHUB_STEP_SUMMARY`. Commits the report to company `main` as a fast-forward (not a force-push). Reports sit under `.uplink/`, so a later queue rebuild copies them.
2. **Submit job** (`environment: oss`). GitHub holds the job until a required reviewer approves the deployment. That click is the IP gate. GitHub records it on the Deployments tab and in the enterprise audit log. The dispatcher is not the approver; turn on **Prevent self-review**.
3. After approval, the job writes `.uplink/reports/<id>/approval.md` (receipt pointing at the run), then `git uplink approve` and `git uplink submit`. App credentials exist only now. Submit is still the first time contribution bytes leave EMU.

Why this fits a workflow:

- The **report is committed**, so reviewers and later auditors read the same packet git has.
- The **approval is committed** as a receipt, and GitHub already has the stronger record (environment review + audit log).
- **`GITHUB_STEP_SUMMARY`** is the page the approver can read without cloning.

Setup steps, secrets table, and ruleset notes: `templates/README.md`. The workflow file is `templates/emu-workflows/uplink-submit.yml`.

Local engine tests still call `git uplink approve` directly. That does not create an Environment review. On GHEC, dispatch the workflow.

**Export preflight** is the guard against “I branched from company `main` so I thought Asha came with me.” Before import, and again before an upstream PR is opened, Uplink applies the candidate onto **public `main` plus declared `dependsOn` only** — not onto company `main`. Then it runs `UPLINK_PREFLIGHT` (the product’s build and test) on that export tree.

- If apply or tests fail, **the change is not imported** and **no upstream PR is created**. The internal PR gets a comment with suggested `--depends-on` / `Uplink-Depends-On:` lines.
- Put `Uplink-Depends-On: upl_…` in the PR body (one per line) and re-run. Required check: `uplink-preflight.yml` on every PR to `main`.
- Set the repo variable `UPLINK_PREFLIGHT` to the command that must pass on a contribution (for example `npm test` or `make test`). Without it, only the apply check runs; tests are what catch “the patch applies but the code calls Asha’s new API.”

Company `main` is always:

```
public upstream/main  +  every patch that is not merged or dropped
```

applied in queue order (insertion order when nobody recorded `dependsOn`; otherwise dependency order, then insertion).

---

## Story 1 — Asha starts a new fix, through internal main, approval, submit, and flow back

Asha needs to change token hashing. Nobody else is in her way.

1. **Fetch latest company `main`.** That tree already includes every queued patch. It is what the product builds.

   ```bash
   git fetch origin
   git checkout main
   git reset --hard origin/main
   git checkout -b feat/sha256
   ```

2. **Write upstream-first.** One branch, one logical change. Public commit message above the cutoff; ticket ids and other internal notes below:

   ```
   Use SHA-256 for tokens

   Replace SHA-1 in the default hasher.

   ----- Uplink: internal below this line -----

   Ticket: PROJ-1234
   Uplink-Export-Author: Asha <asha@users.noreply.github.com>
   ```

   Open an internal PR against company `main`. Two checks start: **prepare** (scrub, author rewrite, affiliation scan) and **export preflight**. Approvers read the prepare report on the PR.

3. **Engineering review.** Required reviewers / CODEOWNERS. This is not IP review. Import is blocked until prepare and preflight are green.

4. **Import — approved for the internal.** Label the PR `uplink:import` (or merge it; that also triggers import). Actions runs:

   ```bash
   git uplink add --title "Use SHA-256 for tokens" \
     --from <PR base sha> --head <PR head sha> \
     --pr <number> --push
   ```

   Uplink isolates Asha’s product diff (not `.uplink/`), appends patch `upl_asha` with status `queued`, rebuilds `main` as upstream plus the queue, and force-with-lease pushes `main`. Asha still has only `feat/sha256`. She does not open a public branch.

5. **Other developers now build her change** the next time they branch from `main`. IP has not run. Nothing has left the enterprise.

6. **Contribution approval.** An operator dispatches **Uplink submit** with `upl_asha`. The packet job commits `.uplink/reports/upl_asha/prepare.md` and writes the Actions job summary. IP/legal approves the waiting **`oss`** Environment deployment (GitHub audit log + Deployments). The same run then records `approval.md`, `git uplink approve`, and `git uplink submit`.

   `approve` is refused if prepare failed or the patch is `internal-only`. `submit` is refused until the patch is `approved` and prepare is still clean. Submit is the first time bytes leave EMU. The public commit uses the prepared author (machine user or `Uplink-Export-Author`) and the scrubbed message, plus `Uplink-Patch-Id`. App credentials are not available until the environment review succeeds.

7. **Upstream review.** Maintainers review a normal GitHub PR. If they want changes, Asha amends the **same** internal patch (fix the files, import again or `git uplink resolve` after a conflict). The next submit force-pushes the fork branch. She still does not grow a second branch.

8. **Flow back.** Upstream squash-merges the PR. Hourly sync (or `git uplink sync`) detects merge in this order: recorded GitHub PR is merged → `Uplink-Patch-Id` trailer → `git patch-id --stable` → empty apply. `upl_asha` becomes `merged` and is **never applied again**.

9. **Rebuild.** Company `main` becomes upstream (now containing Asha’s change, including any maintainer follow-up on those lines) plus remaining patches. The internal copy is gone, so a later upstream salt-the-hash fix is not reverted by re-applying Asha’s old delta.

Asha’s job during this: one internal branch, rebase it if `main` moved, talk to reviewers. The bot owns `main` and the public fork branch.

---

## Story 2 — Asha and Ben work in parallel; Asha is queued first; Ben merges upstream first

Asha and Ben start from the same company `main`. Their changes are independent (different files, or at least they do not need each other’s unmerged code).

### While both are in flight

1. Both branch from `origin/main`, open two internal PRs.
2. Review can overlap. Import is serialized:
   - Actions group `uplink-mutate` (import / sync / submit wait; they do not cancel each other).
   - `.git/uplink.lock` in one checkout.
   - Each job isolates **that PR’s** `base.sha..head.sha`, then refreshes latest `main`, appends, rebuilds, and pushes `--force-with-lease`. If the other import landed first, the lease fails and the job retries. The same internal PR number is imported at most once.
3. Asha’s PR is imported first. Queue: `[upl_asha]`. Company `main` = upstream + Asha.
4. Ben’s import runs second. His diff is still *his* unique delta against the base he branched from, not a replay of live `main`. It is appended. Queue: `[upl_asha, upl_ben]`. Rebuild applies Asha then Ben onto upstream. Both are `queued`. Neither has been IP-approved.

Ben must rebase `feat/ben` onto the new `main` after Asha’s import if he still has an open PR; that is ordinary “integration branch moved.”

Neither records `dependsOn`. Insertion order is Asha then Ben. That order only matters for rebuild apply order on company `main`. It does **not** freeze upstream merge order.

### Export

They can be approved and submitted independently, in either order. On GHEC, dispatch **Uplink submit** for each id and approve the `oss` environment each time. Locally:

```bash
git uplink approve upl_asha && git uplink submit upl_asha
git uplink approve upl_ben  && git uplink submit upl_ben
```

Each public PR is the patch applied onto current public `main` (`uplink/upstream`), not onto the other company patch. That is correct because they did not depend on each other.

### Ben merges upstream first

1. Upstream merges Ben’s public PR. Asha’s public PR is still open.
2. `git uplink sync` marks `upl_ben` `merged` and rebuilds:

   ```
   company main = new upstream (includes Ben) + upl_asha
   ```

   Ben’s patch is not applied internally anymore. His code is present because it is in upstream, including any maintainer edits on top of his PR.
3. Asha’s patch is replayed onto that new upstream. If the files did not overlap, this is clean. If upstream’s version of Ben’s area now touches Asha’s hunks, Asha’s patch goes to `conflict` — same handling as Story 4.
4. When Asha later merges, sync drops `upl_asha`. Company `main` matches upstream plus whatever else is still queued.

**Queue order is not upstream order.** Being first on company `main` does not mean you must merge first publicly. Drop-on-merge is per patch id, so Ben flowing back does not re-apply Ben and does not automatically drop Asha.

---

## Story 3 — Asha is done but not approved/submitted; Ben must build on her change

Asha’s patch is `queued` on company `main`. It has not passed IP. It has not been submitted. Ben’s work needs her new API.

### How Ben creates the change

Ben does **not** wait for IP. Internal product approval already happened at import.

```bash
git fetch origin
git checkout main
git reset --hard origin/main   # this tree already contains upl_asha
git checkout -b feat/ben-on-asha
```

He writes code against Asha’s API, opens an internal PR **targeting company `main`**, gets review, labels `uplink:import`.

Import isolates Ben’s unique delta against that `main` (Asha is already in the base). The queue becomes `[upl_asha, upl_ben]`. Rebuild applies Asha then Ben. Company `main` has both.

Because Ben’s source **needs** Asha, record the dependency at import (repeatable flag):

```bash
git uplink add --title "Log token hashes" \
  --from <base> --head <head> --pr <n> --push \
  --depends-on upl_asha
```

If Asha is not yet on `main` (her PR is still open): **do not import Ben first.** Either wait for `upl_asha` to be queued, or open Ben’s PR against Asha’s feature branch and only import Ben after Asha has been imported and Ben has been rebased onto the new `main`. Importing a stacked PR before its base is on `main` puts Ben’s delta on a tree that does not contain Asha; rebuild will miss her API.

### If Asha is never merged upstream

Company `main` keeps applying `upl_asha` forever (until someone `git uplink drop`s it). Ben’s patch keeps applying on top. The product still has both. That is the point of the queue.

Export is the part that hurts:

- `git uplink submit upl_ben` **refuses** while `upl_asha` is an unmerged, unsubmitted upstream-bound dependency (`Submit upl_asha before upl_ben`).
- If Ben omits `dependsOn`, **import already refuses** when his diff does not apply on public `main` alone, or when `UPLINK_PREFLIGHT` fails on that export tree. He is not queued on company `main` until he records `Uplink-Depends-On: upl_asha` (or rewrites the change so it stands on public `main`). That is the guard: company `main` is not allowed to become the silent base of a later incomplete upstream PR.
- Submit runs the same preflight again. If it fails, Ben stays `approved`, the contrib fork is not pushed, and no public PR is opened. The internal PR is commented.
- If Asha is reclassified `internal-only`, an upstream-bound Ben **cannot** depend on her. The engine rejects that at add time. Ben must be rewritten so it applies on public `main`, or Ben becomes internal-only too, or Asha must stay an upstream-bound patch that will eventually be submitted.

So: Asha never merging is fine for the **internal product**. It blocks **Ben’s contribution** until Asha is submitted or Ben is rewritten not to need her.

### If Asha does merge upstream

1. Sync marks `upl_asha` `merged`. It is never applied again.
2. Rebuild:

   ```
   company main = new upstream (includes Asha, plus later upstream fixes) + upl_ben
   ```

   Ben’s patch is the unique delta; it should apply onto upstream-with-Asha. If maintainers edited Asha’s lines, Ben may conflict — he amends `upl_ben` once (Story 4).
3. Submit of Ben no longer requires Asha (merged dependencies are satisfied). `submit` applies Ben onto current `uplink/upstream`. His public PR is “the rest of the work,” not a replay of Asha.

Ben still has one branch. He never cloned Asha’s public fork branch.

---

## Story 4 — Upstream conflicts with Asha; Ben is queued after her but is not based on her

Queue before the sync: `[upl_asha, upl_ben]`. Ben did not record `dependsOn`. He branched from `main` *before* Asha was imported, or from `main` after her import but his diff does not use her API — either way, his patch is independent.

Upstream changes a file Asha also changed. Sync:

1. Fetch public `main` into `uplink/upstream`.
2. Drop any patches already merged (none in this story).
3. Replay the queue from that upstream. **Asha’s patch does not apply.** Rebuild **stops**.

What you have then:

- `upl_asha` status `conflict`. Sync records that on company `main` (product files stay at the last successful rebuild) and commits `uplink/conflict/upl_asha` with the conflicted files. The Actions sync job opens an internal PR from that branch. **Do not merge it into `main`.**
- **Ben is not applied**, even though he does not depend on Asha. A blocked patch blocks the rest of the rebuild. Company `main` is not updated to “upstream + Ben, skip Asha.” There is no skip.
- Ben’s public PR, if he already submitted, is untouched until his patch is replayed.

### How it is resolved

Asha owns this. Ben does not merge her conflict for her unless he is covering.

```bash
git fetch origin
git checkout uplink/conflict/upl_asha
# fix files so the change is correct on the new upstream
git add -A
git uplink resolve upl_asha
```

`resolve` refreshes **only** `upl_asha`’s patch file (same id), then rebuilds. Remaining patches replay. If Ben still applies, he stays `queued` / `submitted` and company `main` becomes new upstream + amended Asha + Ben.

If Ben **also** conflicts with the new upstream, rebuild stops on him next (`uplink/conflict/upl_ben`). He resolves the same way. Order is the queue order: Asha first, then Ben. You cannot resolve Ben while Asha is still `conflict`; the queue is blocked on her.

If Asha was already `submitted`, the next `git uplink submit upl_asha` (or submit-on-sync) force-pushes `uplink/upl_asha` so the open public PR is the amended patch. Same id, same PR, no second branch.

---

## Story 5 — Cam depends on both Asha and Ben; run until Ben flows back

Asha and Ben are independent. Both are already `queued`:

```
queue: [upl_asha, upl_ben]
company main = upstream + Asha + Ben
```

Cam needs **both** APIs.

### How Cam creates the change

Order of Asha vs Ben on `main` does not matter to Cam as long as **both are imported** before Cam branches.

```bash
git fetch origin && git checkout main && git reset --hard origin/main
git checkout -b feat/cam
# uses Asha's API and Ben's API
```

Internal PR against `main`. Review. Import **after** both bases are queued, with both dependencies recorded:

```bash
git uplink add --title "Wire hash logs into the dashboard" \
  --from <base> --head <head> --pr <n> --push \
  --depends-on upl_asha \
  --depends-on upl_ben
```

`--depends-on` order is recorded as `[upl_asha, upl_ben]`. Rebuild order is still Asha, then Ben, then Cam (dependencies first, then Cam). Cam’s patch file is only Cam’s unique delta against a tree that already had Asha and Ben.

Do not import Cam based on only one of them. A PR opened before the second of Asha/Ben is on `main` will isolate a diff that either contains the missing patch or does not compile.

### Export — order is important

Asha and Ben do **not** depend on each other. Their public PRs are independent and may be submitted in **either order** (separate `oss` environment reviews):

```bash
git uplink approve upl_asha && git uplink submit upl_asha
git uplink approve upl_ben  && git uplink submit upl_ben
```

Cam **cannot** be submitted until each upstream-bound dependency is `submitted` or `merged`. The engine says `Submit upl_asha before upl_cam` (and the same for Ben).

Do **not** submit Cam while Asha and Ben are only `submitted` and still unmerged, expecting one public PR that contains all three. Submit of Cam applies Cam onto **one** stacked fork branch: the last *still-submitted* dependency in `dependsOn`. With `--depends-on upl_asha --depends-on upl_ben` that is Ben’s branch (`uplink/upl_ben`), which is Ben on public `main` **without Asha**. Cam will not apply.

**Rule:** for a patch that depends on two independent patches, wait until those two have **merged upstream** (or until you have a single stacked series A → B → C where each `dependsOn` is a chain, not a pair of siblings). The intended contribution for Cam is “the leftover delta once Asha and Ben are in public `main`.”

### Ben flows back first (Asha still unmerged)

1. Upstream merges Ben. Sync marks `upl_ben` `merged`. Never applied again.
2. Rebuild:

   ```
   company main = new upstream (includes Ben) + upl_asha + upl_cam
   ```

   Cam’s dependency on Ben is now satisfied by upstream itself. Active queue: Asha, then Cam (`dependsOn` still lists Ben, but merged patches are not applied).
3. If Asha’s hunks disagree with upstream-that-has-Ben, Asha conflicts and **Cam waits** (Story 4). Resolve Asha, then Cam replays.
4. Re-submit Asha if she already had a public PR, so `uplink/upl_asha` is Asha on the new upstream (the one that includes Ben).
5. Cam still cannot be submitted until Asha is `submitted` or `merged`. Ben no longer blocks him.
6. When Asha later merges too, sync drops her. Rebuild is:

   ```
   company main = newer upstream (Asha + Ben) + upl_cam
   ```

   Now Cam’s `dependsOn` are both merged. Dispatch **Uplink submit** for `upl_cam` (or locally `git uplink approve upl_cam && git uplink submit upl_cam`) so Cam applies onto public `main`. That is the PR upstream wanted: Cam’s leftover, not a replay of Asha or Ben.

If instead Asha had merged first and Ben second, swap the names. The last of the two to merge is the moment Cam becomes a standalone public PR.

If you had made a **chain** (`Ben dependsOn Asha`, `Cam dependsOn Ben`) because Ben really needed Asha, then submit order is strict: Asha, then Ben (stacked on Asha’s fork branch), then Cam (stacked on Ben’s). Ben cannot merge first in that world without Asha; if he did, you would have exported a broken stack. Do not record a chain unless the source actually needs it.

---

## How a developer figures out what to base their change on

You are choosing a **tree to write code against**, not a second long-lived branch.

1. **Default: latest company `main`.**  
   `git fetch origin && git checkout main && git reset --hard origin/main`  
   That is public upstream plus every patch that is not `merged` or `dropped`. If the product should include it, it is already there. This is the correct base for a new independent fix (Story 1, Story 2).

2. **Look at the queue, not `git log main`.**  
   `git uplink status` and `.uplink/queue.json` list patch ids, titles, `queued` / `approved` / `submitted` / `conflict`, and `dependsOn`. `main`’s commits are synthetic and may disappear on the next rebuild.

3. **If you need someone else’s unmerged work:**  
   - Already `queued`? It is on `main`. Branch from `main` (Story 3, Story 5). Record `--depends-on` for every patch your source actually needs so submit cannot skip it.  
   - Still only an open internal PR? Wait for `uplink:import`, or stack your PR on their feature branch and **rebased onto `main` before you import**. Never import your patch before theirs if you need theirs.

4. **If you do not need their work:**  
   Still branch from latest `main` (it may already contain their patch; that is fine — your isolated diff will not include it). Do **not** record `dependsOn`. You stay an independent public PR. Their earlier queue position does not trap you into merging after them (Story 2).

5. **Never branch from these to start product work:**  
   - `uplink/upstream` — public `main` without company patches. You would reinvent the queue in your working tree.  
   - `uplink/<id>` on the contribution fork — generated, bot-owned, may be force-pushed.  
   - `uplink/conflict/<id>` — only to resolve that patch, then `git uplink resolve`.

6. **After every import or sync, rebase in-flight branches onto new `main`.**  
   The bot may have force-updated it. Same habit as any integration branch.

7. **If `git uplink status` shows `conflict`:**  
   You cannot treat `main` as current. The queue is blocked on that patch. The owner of that id resolves it before anyone else’s later patch (including independent ones) will rebuild (Story 4).

8. **If you are about to submit and apply fails on public `main`:**  
   You had a dependency you did not record, or a dependency that is not merged yet. Either submit/wait for those patches, or rewrite yours so it applies on upstream alone. Do not hand-edit the fork branch.

8. **Run export preflight before you ask for import.**  
   `git uplink preflight --from origin/main --head HEAD` (CI does this on the PR). If it asks for `--depends-on`, you were about to land a change that only makes sense on company `main`. Record the ids, do not merge yet.

The one-line version: **base product work on company `main` after the patches you need are `queued`; record `dependsOn` for submit; let merge-detection drop them when upstream takes them so you never re-apply an old delta. Export preflight is what makes a missing `dependsOn` a blocked PR, not a broken public contribution.**
