# Rules: pushing `uplink/state` and `main`

The forge pack runs jobs at the same time. `uplink-mutate` serializes most of them, but not all: the submit job waits on an Environment and holds no lock, and a pull request can merge into `main` at any moment. These rules keep every change when that happens. Follow them in any command or workflow that writes either branch.

## Goal: no data loss

A change that reached the company (a merged pull request, an approval, a recorded patch) is never dropped by a race or a failed job. It is pushed, or it is pushed again by the next run, or it is kept on a rescue branch.

## `uplink/state`

1. **Never force-push it.** Push it only as a fast-forward. The ruleset `uplink-state-ruleset.json` refuses anything else.
2. **The command that writes it pushes it.** Use `--push` (or `--push-remote`) on the command, not a separate `git uplink push` or `git push` step afterwards. The command knows what it recorded and can do it again.
3. **On a lost race, record again on the state that won.** Fetch origin's `uplink/state`, move local `uplink/state` and `.uplink/` to it, and run the whole change again, checks included (for example the review token). Do not restack local commits onto origin when the change also rebuilt `main`: the rebuild has to start from the state that won. The change must be safe to run twice (an import records a pull request once; an approval that already covers the patch is kept).
4. **Work on origin's state, not the checkout, when the job holds no lock.** `approve --push` reads the queue and the patch from `origin/uplink/state` and builds its commit with a temporary index (`commit_files_on`), so nothing local can be stale.
5. **Refuse to throw away local work.** Before resetting to origin, refuse when local `uplink/state` has commits origin lacks, or `.uplink/` has uncommitted changes.
6. **Only push races are retried.** `is_push_lease_rejected` in `src/lock.rs` lists what counts as a race (`stale info`, `non-fast-forward`, `fetch first`, `cannot lock ref`, GitHub's `fatal error in commit_refs`). Add a new forge message there, with a unit test, when a forge reports a race another way. Auth and hook refusals are never retried.

## `main`

`main` moves when a pull request merges, and when a job pushes a rebuilt `main`.

What matters first is that `uplink/state` has the change, every patch updated. Pushing the rebuilt `main` comes second; it only must not overwrite anything.

1. **Push `uplink/state` before `main`.** When the state push fails, `main` stays as it was.
2. **Force-push only with a lease, and never drop an unimported merge.**
   - Import (`add --push --merged-as`): its scope is its merge commit. Lease on it (`--force-with-lease=refs/heads/main:<merge_commit_sha>`), and push only a rebuild whose tree equals the merge's tree, since an import only reorders (`push_imported_main`). Never lease on the `main` the job happened to fetch: it can hold later merges the rebuild left out.
   - Sync, accept-upstream, resolve, amend, transfer (`--push`): they change content, so their own rebuild cannot be compared with `main`. Fetch origin's `main` right before the push and lease on it. Push only when `uplink/state` already had everything that `main` has: every first-parent commit above `uplink/upstream` has an `Uplink-Patch-Id` (a rebuild wrote it), or a rebuild of the state the command started from, or of one of its last 20 first-parent ancestors on `uplink/state`, gives `main`'s tree (`recorded_in_history`, built in a temporary index on the upstream `main` was built on). Then everything on `main` is in the history of `uplink/state`. This covers merges an internal-only import recorded and left on `main`, and a queue that changed since `main` was built, such as the tooling patch of `init --upgrade`, a drop or an amend: no `--force` is needed for those. Otherwise it is `unrecorded`: do not push.
3. **A main that is not pushed is not a failure.** The import of the merge that is waiting rebuilds from the recorded state and pushes `main`. Report it (`push.main` is `moved`, `differs` or `unrecorded`) and go on.
   - An operator's `rebuild --push` (and `rebuild -i --push`): the same check and lease as the workflows, and it fails when `main` is not pushed. `--force` skips the check, never the lease. On a first publish the lease is that `main` does not exist yet.
4. **Reset every ref the command moves before recording again.** `record_on_remote` puts the company branch and `uplink/upstream` back where they were before each attempt, so a sync run again still sees the new upstream.

## Failures

1. **Keep the change whatever failed.** A failed import pushes `uplink/rescue/import-<pr>-<run>-<attempt>` with `git uplink rescue`: the change, the message, the run's output and the queue as the run saw it.
2. **Have a fallback that does not need the binary.** When `git uplink rescue` fails, the workflow keeps the same files with plain git (`-git` suffix). A failure of git-uplink itself is one of the failures to survive.
3. **Never push a rescue to `uplink/state`.** It goes to a new branch, without force.
4. **Do not rely on a re-run when the command can redo its work.** A packet, a waiting or rejected record and a verify gate are written for the state they are published on, so they record again on the state that won (`--push`) instead of failing the job.

## Tests

Every rule above has a test.

- **Races:** engine tests in `tests/engine.rs` with real repositories: a bare origin and two clones. To make a push lose a race at the right moment, give origin a `pre-receive` hook that moves the ref once (`unset GIT_QUARANTINE_PATH` first, so `git update-ref` is allowed); see `move_state_on_first_push`. Assert what origin has afterwards: fast-forward history, both changes kept, `main` as expected.
- **Workflow wiring:** text tests in `src/tooling.rs` check that a workflow calls the command with `--push`, that no workflow pushes `main` itself (`no_workflow_force_pushes_main_itself`), and that import has the rescue step and the rescue job, both using the `uplink-rescue-import` action.
- **Fallbacks:** a fallback that does not need the binary is run for real: `rescue_action_falls_back_to_plain_git` runs the rescue action's script with `bash` and no `git-uplink` on `PATH`.
- **Output:** a command that gains `--push` keeps its output. Test that the output with `--push` matches the output without it.

## Known gaps

These still push without the rules above and are left for later changes:

- `differs` and `unrecorded` mean `main` has content `uplink/state` lacks: a failed import (its change is on a rescue branch), a push to `main` that bypassed import, or a merge whose content differs from its patch. Jobs keep `main` as it is until the change is recorded (`git uplink add` from the rescue branch) or an operator runs `git uplink rebuild --push --force`.
- A command run again on a moved state can fail where its preflight result no longer matches (the state changed content, not only records). Nothing is lost: the gated branches are still there, and the workflow can be run again.
