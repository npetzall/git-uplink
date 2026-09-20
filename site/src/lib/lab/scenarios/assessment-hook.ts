import { rebuild, type LabScenario, type SimPatch } from "../../simulator";
import { TOKENS_SHA256, exampleSeed, tree } from "../fixtures";
import {
  importOps,
  resetStatus,
  startExample,
  submitOps,
  you,
} from "../ops";

const HOOK_PATH = ".github/workflows/uplink-assessment-hook.yml";
const HOOK_YAML = "name: Uplink assessment hook\n";

export const assessmentHook: LabScenario = {
  id: "assessment-hook",
  title: "07 — Assessment hook",
  blurb:
    "Add uplink-assessment-hook.yml as internal-only. Submit Asha; extras prepend onto the packet. The hook file never leaves the private forge.",
  highlight: "Uplink assessment hook",
  initial: exampleSeed,
  startOperations: startExample(),
  steps: [
    {
      id: "import-hook",
      title: "Import the assessment hook",
      summary:
        "Copy the kit YAML to .github/workflows/uplink-assessment-hook.yml. Open a PR, add uplink:internal-only before Create, merge. Import records it on the internal queue. Merging a workflow file needs workflows write.",
      why: "The forge pack does not ship this file. --upgrade must not overwrite a company-owned assessment hook.",
      operations: [
        you(
          [
            "git checkout -b feat/assessment-hook",
            "mkdir -p .github/workflows",
            'cp "$KIT/patches/uplink-assessment-hook.yml" .github/workflows/uplink-assessment-hook.yml',
            "git add .github/workflows/uplink-assessment-hook.yml",
            'git commit -m "Add Uplink assessment hook"',
            "git push -u origin feat/assessment-hook",
          ],
          "Label uplink:internal-only before Create. Do not git apply a product diff.",
        ),
        ...importOps({ title: "Add Uplink assessment hook", internalOnly: true }),
        resetStatus(),
      ],
      apply: (state) => {
        const patch: SimPatch = {
          id: "upl_hook",
          title: "Add Uplink assessment hook",
          queue: "internal",
          status: "queued",
          dependsOn: [],
          files: { [HOOK_PATH]: HOOK_YAML },
        };
        return rebuild({
          ...state,
          stepId: "import-hook",
          patches: [...state.patches, patch],
          log: [
            ...state.log,
            "Imported upl_hook as internal-only. Company main has uplink-assessment-hook.yml.",
          ],
        });
      },
    },
    {
      id: "import-asha",
      title: "Import Asha’s SHA-256 change",
      summary:
        "Asha branches from company main, switches SHA-1 to SHA-256, and opens one internal PR. Merge plus import records upl_asha as queued.",
      why: "The assessment hook needs a real upstream-bound packet. Assess and preflight run on this PR, not on the hook file.",
      operations: [
        you(
          [
            "git checkout -b feat/sha256",
            'git apply "$KIT/patches/asha-sha256.diff"',
            "git add -A",
            'git commit -m "Use SHA-256 for tokens"',
            "git push -u origin feat/sha256",
          ],
        ),
        ...importOps({ title: "Use SHA-256 for tokens" }),
        resetStatus(),
      ],
      apply: (state) => {
        const patch: SimPatch = {
          id: "upl_asha",
          title: "Use SHA-256 for tokens",
          queue: "upstream",
          status: "queued",
          dependsOn: [],
          files: tree(TOKENS_SHA256),
        };
        return rebuild({
          ...state,
          stepId: "import-asha",
          patches: [...state.patches, patch],
          log: [
            ...state.log,
            "Imported upl_asha as queued. src/tokens.js on main calls sha256. Hook file still on main.",
          ],
        });
      },
    },
    {
      id: "submit-asha",
      title: "Submit Asha; extras lead the packet",
      summary:
        "Dispatch Uplink submit. Finalize runs Uplink assessment hook and prepends ## Company review notes above # Contribution packet. Then approve to-upstream. The contribution fork has SHA-256 only; the hook YAML is not exported.",
      why: "Submit is the only writer of uplink/state. The assessment hook returns an artifact; it must not push state.",
      operations: submitOps("upl_asha"),
      apply: (state) => {
        const patches = state.patches.map((patch) =>
          patch.id === "upl_asha" ? { ...patch, status: "submitted" as const, prNumber: 412 } : patch,
        );
        const asha = patches.find((patch) => patch.id === "upl_asha")!;
        return {
          ...state,
          stepId: "submit-asha",
          patches,
          contrib: [{ branch: "uplink/upl_asha", files: asha.files, prNumber: 412 }],
          log: [
            ...state.log,
            "Finalize prepended ## Company review notes above # Contribution packet.",
            "Approved to-upstream for upl_asha. Opened public PR #412 from uplink/upl_asha.",
          ],
        };
      },
    },
  ],
};
