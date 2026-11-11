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

1. **Force-push only with a lease on what the job's scope includes.** An import's scope is its merge commit: push with `--force-with-lease=refs/heads/main:<merge_commit_sha>`. Never lease on the `main` the job happened to fetch: it can hold later merges the rebuild left out.
2. **A rebuild that only reorders must keep the tree.** Before an import pushes its rebuilt `main`, its tree must equal the merge commit's tree. If it differs, do not push (`push.main` is `differs`).
3. **A lost lease is not a failure when a later job pushes `main` anyway.** The import of the merge that landed later rebuilds and pushes `main`. Report it (`push.main` is `moved`) and go on.
4. **Push `uplink/state` before `main`.** When the state push fails, `main` stays as it was.

## Failures

1. **Keep the change whatever failed.** A failed import pushes `uplink/rescue/import-<pr>-<run>-<attempt>` with `git uplink rescue`: the change, the message, the run's output and the queue as the run saw it.
2. **Have a fallback that does not need the binary.** When `git uplink rescue` fails, the workflow keeps the same files with plain git (`-git` suffix). A failure of git-uplink itself is one of the failures to survive.
3. **Never push a rescue to `uplink/state`.** It goes to a new branch, without force.

## Tests

Every rule above has a test.

- **Races:** engine tests in `tests/engine.rs` with real repositories: a bare origin and two clones. To make a push lose a race at the right moment, give origin a `pre-receive` hook that moves the ref once (`unset GIT_QUARANTINE_PATH` first, so `git update-ref` is allowed); see `move_state_on_first_push`. Assert what origin has afterwards: fast-forward history, both changes kept, `main` as expected.
- **Workflow wiring:** text tests in `src/tooling.rs` check that a workflow calls the command with `--push`, has no bare `git push origin main --force` or `git uplink push` where the command pushes, and has the rescue step with its fallback.
- **Output:** a command that gains `--push` keeps its output. Test that the output with `--push` matches the output without it.

## Known gaps

These still push without the rules above and are left for later changes:

- Sync, resolve, amend and transfer force-push `main` without a lease.
- The submit packet and rejected jobs, resolve, amend, sync, transfer and verify push `uplink/state` with a separate `git push origin uplink/state`. They hold `uplink-mutate`, but submit's approve does not, so their push can lose to it and fail the job (nothing is lost; the job fails visibly).
