import { rebuild, type LabScenario, type SimPatch } from "../../simulator";
import {
  TOKENS_TTL_1800,
  TOKENS_TTL_7200,
  TOKENS_TTL_CONFLICT,
  exampleSeed,
  tree,
} from "../fixtures";
import {
  branchPush,
  fixConflict,
  importOps,
  resetStatus,
  resolveOps,
  startExample,
  syncFromUpstream,
  you,
} from "../ops";

export const upstreamConflict: LabScenario = {
  id: "upstream-conflict",
  title: "04 — Upstream conflict",
  blurb:
    "Ben’s TTL patch is queued. Upstream shortens the same lines. Sync stops; Ben is conflict; you work on uplink/conflict/<id>-work and merge the gated PR.",
  highlight: "7200",
  initial: exampleSeed,
  startOperations: startExample(),
  steps: [
    {
      id: "import-ben",
      title: "Import Ben’s TTL",
      summary:
        "Ben extends ttl() to 7200. After import he is queued. Company main still has ttl() == 7200 and no conflict markers.",
      why: "The product builds the queued patch. Upstream has not moved yet.",
      operations: [
        branchPush("feat/ttl", "ben-ttl.diff", "Extend TTL"),
        ...importOps({ title: "Extend TTL" }),
        resetStatus(),
      ],
      apply: (state) => {
        const patch: SimPatch = {
          id: "upl_ben",
          title: "Extend TTL",
          queue: "upstream",
          status: "queued",
          dependsOn: [],
          files: tree(TOKENS_TTL_7200),
        };
        return rebuild({
          ...state,
          stepId: "import-ben",
          patches: [...state.patches, patch],
          log: [...state.log, "Imported upl_ben as queued. Company main ttl() == 7200."],
        });
      },
    },
    {
      id: "sync-conflict",
      title: "Upstream overlaps; sync stops",
      summary:
        "Upstream shortens default ttl to 1800. Inspect finds a foreign commit, so apply waits on from-upstream. After approval, Ben’s patch does not apply. Status conflict. Company main stays at the last successful rebuild (still 7200, no markers). A gated PR is opened from -work into the protected base.",
      why: "Sync must fail closed. Later patches wait. A conflict is not an export.",
      operations: [
        you(
          [
            "git fetch origin && git checkout main && git reset --hard origin/main",
            'git apply "$KIT/patches/upstream-shorten-ttl.diff"',
            'git commit -am "shorten default ttl"',
            "git push origin main",
          ],
          "In the upstream clone.",
        ),
        ...syncFromUpstream("upl_ben"),
      ],
      apply: (state) => {
        const patches = state.patches.map((patch) =>
          patch.id === "upl_ben" ? { ...patch, status: "conflict" as const } : patch,
        );
        return {
          ...state,
          stepId: "sync-conflict",
          upstream: tree(TOKENS_TTL_1800),
          patches,
          company: state.company,
          contrib: state.contrib,
          conflict: {
            patchId: "upl_ben",
            branch: "uplink/conflict/upl_ben",
            blockedIds: [],
            files: tree(TOKENS_TTL_CONFLICT),
            phase: "stopped",
          },
          log: [
            ...state.log,
            "from-upstream approved. Sync conflict on upl_ben. Opened uplink/conflict/upl_ben and -work. Company main not rebuilt.",
          ],
        };
      },
    },
    {
      id: "conflict-fix",
      title: "Checkout the work branch and fix",
      summary:
        "Fetch, check out uplink/conflict/upl_ben-work, keep Ben’s 7200 on the new upstream, commit, and push. Status stays conflict until the gated PR is merged and resolve runs.",
      why: "Humans push -work only. Merge is the only update to the protected base. They do not hand-edit company main.",
      operations: fixConflict("upl_ben"),
      apply: (state) => {
        if (!state.conflict) return state;
        return {
          ...state,
          stepId: "conflict-fix",
          conflict: {
            ...state.conflict,
            files: tree(TOKENS_TTL_7200),
            phase: "fixed",
          },
          log: [
            ...state.log,
            "Checked out uplink/conflict/upl_ben-work. Kept return 7200 and staged src/tokens.js.",
          ],
        };
      },
    },
    {
      id: "resolve",
      title: "Resolve the same patch id",
      summary:
        "Uplink resolve rewrites only that patch file, rebuilds main, and deletes the base and -work branches. Ben is queued again. src/tokens.js has return 7200 and not 1800.",
      why: "One patch identity. Resolve is not submit; queued work still needs to-upstream before it can leave the private forge.",
      operations: resolveOps("upl_ben"),
      apply: (state) => {
        const patches = state.patches.map((patch) =>
          patch.id === "upl_ben"
            ? { ...patch, status: "queued" as const, files: tree(TOKENS_TTL_7200) }
            : patch,
        );
        return rebuild({
          ...state,
          stepId: "resolve",
          patches,
          conflict: undefined,
          contrib: state.contrib,
          log: [
            ...state.log,
            "Resolved upl_ben. Company main rebuilt (upstream 1800 overwritten by amended ttl 7200). Fork unchanged.",
          ],
        });
      },
    },
  ],
};
