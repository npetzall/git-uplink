# Way of working

This is how developers use Uplink day to day. You open one internal PR per change and merge it after review. You never maintain a second branch for upstream. Operators can walk the model in the live lab on the project site, or inspect a checkout with `git uplink web-ui`.

Read this alongside `.uplink/queue.json` (on `uplink/state`) and `git uplink status`. The binary is `git-uplink` (a Git subcommand). Durable queue history lives on the orphan branch `uplink/state`. Company `main` is product-only: public upstream plus every patch that is not merged or dropped. Sync force-updates `main` only when upstream moved (or a drop/resolve requires a replay).

Write every change **as if it were the upstream submission**. Company-only details (issue ids, internal reviewers, export-author override) go **below the cutoff** in the **pull request** title and body. `git uplink init --forge ghec` installs `templates/github/pull_request_template.md` as `.github/pull_request_template.md` in the product repo. HTML comments in that template are visible while writing the PR and are stripped when Uplink stores the message. Company `main` keeps the cutoff; the contribution fork does not. That template does not turn off your commit signing: `git uplink` keeps bot identity and unsigned commits on the subprocess only. Network git (`push`, `sync`, `submit`) uses per-remote `UPLINK_INTERNAL_*`, `UPLINK_CONTRIB_*`, or `UPLINK_UPSTREAM_*` KEY or TOKEN (KEY wins; keys must be passwordless), not `GITHUB_TOKEN` or your default SSH key. GitHub itself is `gh` in the workflows; `submitted` / `gated` record the result.

| Phase | What you do | Result |
| --- | --- | --- |
| Write | Branch from company `main`. Public rationale in the PR title and above the cutoff in the body. | One internal branch. |
| Assess | Opens with the internal PR. `git uplink assess` | Scrubbed message, rewritten author, affiliation scan. Report on the PR and in `GITHUB_STEP_SUMMARY`. |
| Export preflight | Same PR | Diff must stand on public `main` + declared deps; `UPLINK_PREFLIGHT` must pass. |
| Internal product | Review + merge | Status `queued`. Product builds it. |
| Contribution / IP | Dispatch **Uplink submit**. Optional company assessment-hook extras prepend onto the packet. IP approves GitHub Environment **`to-upstream`**. Same run submits. | Report + approval committed under `.uplink/reports/`. Public PR uses the prepared identity. GitHub audit log records the reviewer. |
| Inbound upstream | Hourly **Uplink sync**. Foreign public commits wait on GitHub Environment **`from-upstream`**. Flow-back of our patches skips that wait. | `uplink/upstream` updates; company `main` rebuilds. |

Default destination is the upstream queue. `uplink:internal-only` is the escape hatch: it records the patch in `internal[]` and never goes through the second gate.

## Contribution approval via the to-upstream GitHub Environment

On GitHub Enterprise Cloud this is an option, and it is the option to use. Do not invent a second spreadsheet for IP sign-off.

Create a repository Environment named **`to-upstream`**. Required reviewers are IP/legal. Put the GitHub App secrets that can push the public contribution fork **on that environment only**. Dispatch `Uplink submit` with the patch id. The workflow:

1. **Packet job** (no environment, no App secrets). Runs `git uplink report <id>`, which writes `.uplink/reports/<id>/assessment.md` and appends the same markdown to `GITHUB_STEP_SUMMARY`. Commits the report to `uplink/state` as a fast-forward (not a force-push). Reports stay on that orphan branch, so a later product rebuild does not drop them.
2. **Finalize job** (no environment). If `.github/workflows/uplink-assessment-hook.yml` exists, dispatches it with `patch_id` and `caller_run_id`, waits (`timeout-minutes` on the job), downloads artifact `uplink-packet-extra` from that run, and re-runs `git uplink report --extra-dir` so company extras sit at the top of `assessment.md`. A failed assessment hook fails this job, so IP is never asked. Job outputs are not the contract (1 MB, same-run only). `on: workflow_run` for **Uplink submit** is too late — it fires after the environment wait. The hook must not push `uplink/state`. Copy-paste: `examples/github/patches/uplink-assessment-hook.yml`.
3. **Submit job** (`environment: to-upstream`). GitHub holds the job until a required reviewer approves the deployment. That click is the IP gate. GitHub records it on the Deployments tab and in the enterprise audit log. The dispatcher is not the approver; turn on **Prevent self-review**.
4. After approval, the job writes `.uplink/reports/<id>/approval.md` (receipt pointing at the run), then `git uplink approve` and `git uplink submit`. App credentials exist only now. Submit is still the first time contribution bytes leave the private forge.

Why this fits a workflow:

- The **report is committed**, so reviewers and later auditors read the same packet git has.
- The **approval is committed** as a receipt, and GitHub already has the stronger record (environment review + audit log).
- **`GITHUB_STEP_SUMMARY`** is the page the approver can read without cloning.

Setup steps, secrets table, and ruleset notes: `templates/README.md`. The workflow file is `templates/ghec/.github/workflows/uplink-submit.yml`.

Local engine tests still call `git uplink approve` directly. That does not create an Environment review. On GHEC, dispatch the workflow.

## Inbound approval via the from-upstream GitHub Environment

Public `main` can move for reasons that are not a company contribution flowing back. Sync must not rebuild company `main` onto those commits until someone reviews them.

Create a repository Environment named **`from-upstream`**. Required reviewers are inbound/security (not the `to-upstream` IP reviewers unless you want the same people). Do not put origin-push or contrib secrets on it.

1. **Inspect job** (no environment). `git uplink sync` fetches public `main` but does not move `uplink/upstream`. Commits that match a company patch (trailer / `patch-id`) apply immediately. Unchanged public `main` is a no-op.
2. **Apply job** (`environment: from-upstream`). Only scheduled when inspect found unmatched commits. GitHub holds the job until a reviewer approves. The packet is `.uplink/reports/from-upstream/incoming.md` on `uplink/state` (and `GITHUB_STEP_SUMMARY`). After approval, `git uplink accept-upstream` promotes the frozen SHA, marks flowed-back patches `merged`, and rebuilds. Apply conflicts still open `uplink/conflict/<id>` — after this gate, not instead of it.

Workflow group `uplink-sync` serializes inbound reviews (hourly cron will not stack deployments). Inspect/apply take `uplink-mutate` per job so the wait does not freeze imports.

Local engine tests call `git uplink accept-upstream` directly after a sync that set `needsApproval`.

Company `main` is always:

```
public upstream/main  +  tooling  +  active upstream[]  +  active internal[]
```

Tooling is installed by `init` / `--upgrade`. Product patches default to the **upstream** queue (bound for contribution). Label `uplink:internal-only` (or `add --internal-only`) appends to **internal**, which always applies last and is never exported. Membership in a queue is the intent; patches do not store an intent field.

Within `upstream` and within `internal`, insertion order is used if nobody recorded `dependsOn`; otherwise topological order, then insertion. Internal may depend on upstream. Upstream must not depend on internal or tooling.

If an upstream-bound change only applies on internal work, pick one:

1. Rewrite it so it does not need the internal code, or
2. Promote that internal patch into `upstream` and record `dependsOn`, or
3. Put the new change in `internal` (`uplink:internal-only` / `--internal-only`).

**Export preflight** is the guard against “I branched from company `main` so I thought Asha came with me.” Before import, Uplink applies the candidate onto **public `main` plus declared `dependsOn`**, then onto **tooling + queued upstream** (internal omitted). Then it runs `UPLINK_PREFLIGHT` (the product’s build and test) on that export tree. Internal-only PRs skip both checks.

- If apply or tests fail, **the change is not imported** and **no upstream PR is created**. The internal PR gets a comment with suggested `--depends-on` / `Uplink-Depends-On:` lines, or the three remediations above.
- Put `Uplink-Depends-On: upl_…` in the PR body (one per line) and re-run. Required check: `uplink-preflight.yml` on every upstream-bound PR to `main`.
- Set the repo variable `UPLINK_PREFLIGHT` to the command that must pass on a contribution (for example `npm test` or `make test`). Without it, only the apply check runs; tests are what catch “the patch applies but the code calls Asha’s new API.”

## Onboarding a repo that is already ahead of upstream

Greenfield init (company `main` matches public upstream) still installs the tooling patch and rebuilds `main`. If internal `main` is a fast-forward of public upstream — typically 10–20 private commits — `git uplink init` does **not** rebuild and does **not** push. It snapshots `HEAD` as `uplink/adopt-from`, writes the tooling patch first on `uplink/state`, then turns unique **first-parent** commits into queued patches after it.

- Merge-commit history: each merge on `main` is one patch (the PR as landed). Side-branch commits stay hidden.
- Rebase / squash-linear history: assign group numbers in the terminal UI so related commits become one patch. `--adopt-groups` is the non-interactive form of that list.
- `main` still has the original commits so you can compare.

Preview, then publish:

```bash
git uplink rebuild --branch uplink/verify
git diff main uplink/verify
git uplink rebuild --push
```

`--branch` other than company `main` is read-only on the queue. After you are satisfied, a normal rebuild rewrites those original commits into synthetic apply commits (`public upstream` + tooling + adopted patches) and `--push` publishes `uplink/state` and `main`. If internal is also behind upstream, rebase or merge current upstream first; init refuses a diverged history.

After that, work as in the stories below: one internal PR per change.

---

## Story 1 — Asha starts a new fix, through internal main, approval, submit, and flow back

Asha needs to change token hashing. Nobody else is in her way.

1. **Fetch latest company `main`.** That tree already includes every queued patch. It is what the product builds.

   ```bash
   git uplink reset
   git checkout -b feat/sha256
   ```

2. **Write upstream-first.** One branch, one logical change. The PR title is the commit subject. Public rationale above the cutoff in the PR body; ticket ids and other internal notes below. Git commit messages on the branch are not concatenated.

   ```
   Use SHA-256 for tokens

   Replace SHA-1 in the default hasher.

   ----- Uplink: internal below this line -----

   Ticket: PROJ-1234
   Uplink-Export-Author: Asha <asha@users.noreply.github.com>
   ```

   Open an internal PR against company `main`. Two checks start: **assess** (scrub, author rewrite, affiliation scan) and **export preflight**. Approvers read the assess report on the PR, including both the company commit message (cutoff kept) and the upstream commit message (cutoff removed).

3. **Engineering review.** Required reviewers / CODEOWNERS. This is not IP review. Merge is blocked until assess and preflight are green.

4. **Import — approved for the internal.** Merge the PR after review. Actions runs:

   ```bash
   git uplink add --title "Use SHA-256 for tokens" \
     --message-file <title-and-body> \
     --from <PR base sha> --head <PR head sha> \
     --pr <number>
   git uplink push
   ```

   Uplink isolates Asha’s product diff (not `.uplink/`) and appends patch `upl_asha` with status `queued` on `uplink/state`. The merge already put the change on company `main`. Because this patch is upstream-bound, import then rebuilds `main` so `upl_asha` sits under any `internal[]` patches. Internal-only import records the patch and leaves `main` at the merge tree. The patch stays `queued` because `uplink/upstream` does not have it yet. Asha still has only `feat/sha256`. She does not open a public branch.

5. **Other developers now build her change** the next time they branch from `main`. IP has not run. Nothing has left the enterprise.

6. **Contribution approval.** An operator dispatches **Uplink submit** with `upl_asha`. The packet job commits `.uplink/reports/upl_asha/assessment.md` and writes the Actions job summary. IP/legal approves the waiting **`to-upstream`** Environment deployment (GitHub audit log + Deployments). The same run then records `approval.md`, `git uplink approve`, and `git uplink submit`.

   `approve` is refused if assess failed or the patch is `internal-only`. `submit` is refused until the patch is `approved` and assess is still clean. Submit is the first time bytes leave the private forge. The public commit uses the export author (machine user or `Uplink-Export-Author`) and the scrubbed message, plus `Uplink-Patch-Id`. App credentials are not available until the environment review succeeds.

7. **Upstream review.** Maintainers review a normal GitHub PR. If they want changes, Asha amends the **same** internal patch (fix the files, import again or `git uplink resolve` after a conflict). If the patch was already submitted, it becomes `amended` and IP approves the delta before submit force-pushes the same fork branch. She still does not grow a second branch.

8. **Flow back.** Upstream squash-merges the PR. Hourly sync (or `git uplink sync`) classifies new public commits. If the only new commits match Asha (trailer / `patch-id`), it skips **`from-upstream`** approval, marks `upl_asha` `merged`, and is **never applied again**. If public `main` also has commits that are not ours, inspect writes `.uplink/reports/from-upstream/incoming.md` and waits on Environment **`from-upstream`** before `uplink/upstream` moves.

9. **Rebuild.** After auto-apply or `from-upstream` approval, company `main` becomes upstream (now containing Asha’s change, including any maintainer follow-up on those lines) plus remaining patches. The internal copy is gone, so a later upstream salt-the-hash fix is not reverted by re-applying Asha’s old delta. That follow-up is a foreign commit: it goes through `from-upstream` first.

Asha’s job during this: one internal branch, rebase it if `main` moved, talk to reviewers. The bot owns the public fork branch; humans merge product PRs.

---

## Story 2 — Asha and Ben work in parallel; Asha is queued first; Ben merges upstream first

Asha and Ben start from the same company `main`. Their changes are independent (different files, or at least they do not need each other’s unmerged code).

### While both are in flight

1. Both branch from `origin/main`, open two internal PRs.
2. Review can overlap. GitHub serializes the merges. Import is serialized:
   - Actions group `uplink-mutate` (import / sync / submit wait; they do not cancel each other).
   - `.git/uplink.lock` in one checkout.
   - Each job isolates **that PR’s** `base.sha..head.sha`, then refreshes latest `main` and `uplink/state`, appends the patch on the state branch, and fast-forward pushes `uplink/state`. If the other import landed first, the push fails and the job retries. The same internal PR number is imported at most once.
3. Asha’s PR is merged and imported first. Queue: `[upl_asha]`. Company `main` already includes Asha.
4. Ben’s PR is merged onto that `main`, then imported. His diff is still *his* unique delta against the base he branched from, not a replay of live `main`. It is appended on `uplink/state`. Queue: `[upl_asha, upl_ben]`. Both are `queued`. Neither has been IP-approved.

Ben must rebase `feat/ben` onto the new `main` after Asha’s merge if he still has an open PR; that is ordinary “integration branch moved.”

Neither records `dependsOn`. Insertion order is Asha then Ben. That order only matters for rebuild apply order on company `main`. It does **not** freeze upstream merge order.

### Export

They can be approved and submitted independently, in either order. On GHEC, dispatch **Uplink submit** for each id and approve the `to-upstream` environment each time. Locally:

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
git uplink reset   # this tree already contains upl_asha
git checkout -b feat/ben-on-asha
```

He writes code against Asha’s API, opens an internal PR **targeting company `main`**, gets review, merges.

Import isolates Ben’s unique delta against that `main` (Asha is already in the base). The queue becomes `[upl_asha, upl_ben]`. Rebuild applies Asha then Ben. Company `main` has both.

Because Ben’s source **needs** Asha, record the dependency at import (repeatable flag):

```bash
git uplink add --title "Log token hashes" \
  --from <base> --head <head> --pr <n> \
  --depends-on upl_asha
git uplink push
```

If Asha is not yet on `main` (her PR is still open): **do not merge Ben first.** Either wait for Asha’s merge, or open Ben’s PR against Asha’s feature branch and only merge Ben after Asha is on `main` and Ben has been rebased onto that `main`. Merging a stacked PR before its base is on `main` puts Ben’s delta on a tree that does not contain Asha; rebuild will miss her API.

### If Asha is never merged upstream

Company `main` keeps applying `upl_asha` forever (until someone `git uplink drop`s it). Ben’s patch keeps applying on top. The product still has both. That is the point of the queue.

Export is the part that hurts:

- `git uplink submit upl_ben` **refuses** while `upl_asha` is an unmerged, unsubmitted upstream-bound dependency (`Submit upl_asha before upl_ben`).
- If Ben omits `dependsOn`, **import already refuses** when his diff does not apply on public `main` alone, or when `UPLINK_PREFLIGHT` fails on that export tree. Required checks must stay red so he cannot merge until he records `Uplink-Depends-On: upl_asha` (or rewrites the change so it stands on public `main`). That is the guard: company `main` is not allowed to become the silent base of a later incomplete upstream PR.
- Submit runs the same preflight again. If it fails, Ben stays `approved`, the contrib fork is not pushed, and no public PR is opened. The workflow comments the internal PR.
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

Upstream changes a file Asha also changed. That is a foreign commit. Sync:

1. Fetch public `main` **without** moving `uplink/upstream`. Classify the range. Unmatched commits write `.uplink/reports/from-upstream/incoming.md`.
2. A required reviewer approves Environment **`from-upstream`**. The apply job runs `git uplink accept-upstream`, which points `uplink/upstream` at the reviewed SHA.
3. Drop any patches already merged (none in this story).
4. Replay the queue from that upstream. **Asha’s patch does not apply.** Rebuild **stops**.

What you have then:

- `upl_asha` status `conflict`. Sync records that on `uplink/state` (product files on `main` stay at the last successful rebuild) and creates `uplink/conflict/upl_asha` (protected base at the apply prefix) plus `uplink/conflict/upl_asha-work` (conflict markers). The Actions sync job opens a gated PR from `-work` into the base (`uplink:conflict`).
- **Ben is not applied**, even though he does not depend on Asha. A blocked patch blocks the rest of the rebuild. Company `main` is not updated to “upstream + Ben, skip Asha.” There is no skip.
- Ben’s public PR, if he already submitted, is untouched until his patch is replayed.

### How it is resolved

Asha owns this. Ben does not merge her conflict for her unless he is covering.

```bash
git fetch origin
git checkout uplink/conflict/upl_asha-work
# fix files so the change is correct on the new upstream
git add -A
git commit -m "Resolve upl_asha onto the new upstream"
git push origin uplink/conflict/upl_asha-work
```

Merge the gated PR into `uplink/conflict/upl_asha`. On GHEC that merge runs **Uplink resolve** (`uplink-resolve.yml`). Locally (or if the workflow is not installed), checkout the base or work branch after the tree is clean:

```bash
git uplink resolve upl_asha
```

`resolve` refreshes **only** `upl_asha`’s patch file (same id), then rebuilds. Remaining patches replay. If Ben still applies, he stays `queued` / `submitted` and company `main` becomes new upstream + amended Asha + Ben.

If Asha was never submitted, she returns to `queued`. If she **was** already submitted (public PR still open), she becomes `amended`. Company `main` has the new bytes immediately. The contribution fork still has the last IP-approved bytes. On GHEC, **Uplink resolve** dispatches **Uplink submit** for that id. IP reviews a **delta-first** packet: the change since the last approval, then the historical packet marked already approved. After to-upstream approval, submit force-pushes `uplink/upl_asha`. Same id, same PR, no second branch. `git uplink submit` refuses `amended` until that delta is approved.

If Ben **also** conflicts with the new upstream, rebuild stops on him next (`uplink/conflict/upl_ben` plus `-work`). `git uplink resolve` exits **2** (this id was amended; the next id did not apply). On GHEC the resolve job pushes company `main` (amend + Ben’s `conflict` status), publishes Ben’s gated PR, then Asha’s PR is already merged. If Asha is `amended`, it still dispatches submit for her delta. He resolves the same way. Order is the queue order: Asha first, then Ben. You cannot resolve Ben while Asha is still `conflict`; the queue is blocked on her.

---

## Transfer between queues

`git uplink transfer <id> --to-upstream` or `--to-internal` moves a patch between `internal[]` and `upstream[]`. The patch must already be in the source queue. If git apply **and** preflight (`UPLINK_PREFLIGHT` / export checks for `--to-upstream`) both pass, the move is committed immediately. If either fails, git-uplink cuts `uplink/transfer-to-*/<id>` plus `-work` and does **not** write `queue.json`. Merge the gated PR to complete; **close the PR without merging** to abort (branches deleted, queue unchanged). `--to-internal` of a submitted patch abandons the public contrib PR.

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
git uplink reset
git checkout -b feat/cam
# uses Asha's API and Ben's API
```

Internal PR against `main`. Review. Import **after** both bases are queued, with both dependencies recorded:

```bash
git uplink add --title "Wire hash logs into the dashboard" \
  --from <base> --head <head> --pr <n> \
  --depends-on upl_asha \
  --depends-on upl_ben
git uplink push
```

`--depends-on` order is recorded as `[upl_asha, upl_ben]`. Rebuild order is still Asha, then Ben, then Cam (dependencies first, then Cam). Cam’s patch file is only Cam’s unique delta against a tree that already had Asha and Ben.

Do not merge Cam based on only one of them. A PR opened before the second of Asha/Ben is on `main` will isolate a diff that either contains the missing patch or does not compile.

### Export — order is important

Asha and Ben do **not** depend on each other. Their public PRs are independent and may be submitted in **either order** (separate `to-upstream` environment reviews):

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
4. If Asha already had a public PR, resolve left her `amended` and dispatched submit. After IP approves the delta, `uplink/upl_asha` is Asha on the new upstream (the one that includes Ben).
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
   `git uplink reset`  
   That is public upstream plus every patch that is not `merged` or `dropped`. If the product should include it, it is already there. This is the correct base for a new independent fix (Story 1, Story 2).

2. **Look at the queue, not `git log main`.**  
   `git uplink status` and `.uplink/queue.json` on `uplink/state` list patch ids, titles, `queued` / `approved` / `submitted` / `amended` / `conflict`, and `dependsOn`. Merge commits on `main` are ordinary product history. Sync still may force-update `main` when upstream moved.

3. **If you need someone else’s unmerged work:**  
   - Already `queued`? It is on `main`. Branch from `main` (Story 3, Story 5). Record `--depends-on` for every patch your source actually needs so submit cannot skip it.  
   - Still only an open internal PR? Wait for their merge, or stack your PR on their feature branch and **rebase onto `main` before you merge**. Never merge your patch before theirs if you need theirs.

4. **If you do not need their work:**  
   Still branch from latest `main` (it may already contain their patch; that is fine — your isolated diff will not include it). Do **not** record `dependsOn`. You stay an independent public PR. Their earlier queue position does not trap you into merging after them (Story 2).

5. **Never branch from these to start product work:**  
   - `uplink/state` — queue, patches, and uplink reports. Do not commit product work here.  
   - `uplink/upstream` — public `main` without company patches. You would reinvent the queue in your working tree.  
   - `uplink/<id>` on the contribution fork — generated, bot-owned, may be force-pushed.  
   - `uplink/conflict/<id>` — protected conflict base; do not push it. Work on `uplink/conflict/<id>-work` and merge the gated PR. Same pattern for `uplink/transfer-to-*/<id>`.

6. **After every merge or sync that moved `main`, rebase in-flight branches onto new `main`.**
   Merge lands product history. Sync force-updates `main` only when upstream (or drop/resolve) requires a replay.

7. **If `git uplink status` shows `conflict`:**  
   You cannot treat `main` as current. The queue is blocked on that patch. The owner of that id resolves it before anyone else’s later patch (including independent ones) will rebuild (Story 4).

8. **If you are about to submit and apply fails on public `main`:**  
   You had a dependency you did not record, or a dependency that is not merged yet. Either submit/wait for those patches, or rewrite yours so it applies on upstream alone. Do not hand-edit the fork branch.

8. **Run export preflight before you merge.**  
   `git uplink preflight --from origin/main --head HEAD` (CI does this on the PR). If it asks for `--depends-on`, you were about to land a change that only makes sense on company `main`. Record the ids, do not merge yet.

The one-line version: **base product work on company `main` after the patches you need are `queued`; record `dependsOn` for submit; let merge-detection drop them when upstream takes them so you never re-apply an old delta. Export preflight is what makes a missing `dependsOn` a blocked PR, not a broken public contribution.**
