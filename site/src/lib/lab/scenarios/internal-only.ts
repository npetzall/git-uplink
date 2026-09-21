import { rebuild, type LabScenario, type SimPatch } from "../../simulator";
import { TOKENS_TELEMETRY, exampleSeed, tree } from "../fixtures";
import {
  branchPush,
  ciJob,
  importOps,
  assessCi,
  resetStatus,
  startExample,
  you,
} from "../ops";

export const internalOnly: LabScenario = {
  id: "internal-only",
  title: "06 — Internal-only",
  blurb:
    "A company-only telemetry hook must never pass the IP gate. Assess fails without the label. Submit is refused.",
  highlight: "companyTelemetry",
  initial: exampleSeed,
  startOperations: startExample(),
  steps: [
    {
      id: "assess-fail",
      title: "Assess fails without the label",
      summary:
        "Open a PR for vendor telemetry without uplink:internal-only. Uplink upstream assess fails: the export surface contains companyTelemetry (in UPLINK_REDACT_KEYWORDS). Close this PR.",
      why: "Affiliation scan is the guard. Internal-only is an explicit label, not a silent default.",
      operations: [
        branchPush("feat/telemetry-public", "internal-telemetry.diff", "Vendor telemetry"),
        you(
          [
            'git uplink assess --title "Vendor telemetry" --message-file /tmp/uplink-msg.txt --from <base.sha> --head <head.sha>',
          ],
          "No uplink:internal-only label. Assess fails on companyTelemetry.",
        ),
        assessCi("Vendor telemetry"),
      ],
      apply: (state) => ({
        ...state,
        stepId: "assess-fail",
        log: [
          ...state.log,
          "Assess failed: companyTelemetry is in UPLINK_REDACT_KEYWORDS. PR not merged.",
        ],
      }),
    },
    {
      id: "import-internal",
      title: "Import with uplink:internal-only",
      summary:
        "Open a new PR and add label uplink:internal-only before Create (Uplink PR checks only see labels that exist when the check runs). Merge. The patch is queued on the internal queue. Company main calls companyTelemetry().",
      why: "The label appends to internal[] and skips both Uplink PR checks. Tooling from bootstrap is the same class of change.",
      operations: [
        branchPush("feat/telemetry", "internal-telemetry.diff", "Vendor telemetry"),
        ...importOps({ title: "Vendor telemetry", internalOnly: true }),
        resetStatus(),
      ],
      apply: (state) => {
        const patch: SimPatch = {
          id: "upl_telemetry",
          title: "Vendor telemetry",
          queue: "internal",
          status: "queued",
          dependsOn: [],
          files: tree(TOKENS_TELEMETRY),
        };
        return rebuild({
          ...state,
          stepId: "import-internal",
          patches: [...state.patches, patch],
          log: [
            ...state.log,
            "Imported upl_telemetry as internal-only. companyTelemetry() is on company main.",
          ],
        });
      },
    },
    {
      id: "submit-refused",
      title: "Submit is refused",
      summary:
        "Dispatch Uplink submit with that patch id. git uplink approve / submit refuse internal-only. Nothing is pushed to the contrib fork.",
      why: "Internal-only is the odd case. Almost every internal contribution is also contributed upstream.",
      operations: [
        you(
          ["git uplink approve upl_telemetry", "git uplink submit upl_telemetry"],
          "Refused: patch is internal-only.",
        ),
        ciJob(
          "Uplink submit",
          [
            "git uplink init",
            "git uplink report upl_telemetry",
            "git uplink approve upl_telemetry",
            "git uplink submit upl_telemetry",
          ],
          "approve / submit refuse internal-only. Fork unchanged.",
        ),
      ],
      apply: (state) => ({
        ...state,
        stepId: "submit-refused",
        contrib: [],
        log: [
          ...state.log,
          "Submit refused for upl_telemetry (internal-only). Contribution fork empty of telemetry.",
        ],
      }),
    },
  ],
};
