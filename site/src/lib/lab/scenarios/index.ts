import type { LabScenario } from "../../simulator";
import { camTwoDeps } from "./cam-two-deps";
import { fullLifecycle } from "./full-lifecycle";
import { internalOnly } from "./internal-only";
import { parallelIndependent } from "./parallel-independent";
import { soloFix } from "./solo-fix";
import { stackedDependsOn } from "./stacked-depends-on";
import { upstreamConflict } from "./upstream-conflict";

export const DEFAULT_SCENARIO_ID = "full-lifecycle";

export const LAB_SCENARIOS: LabScenario[] = [
  fullLifecycle,
  soloFix,
  parallelIndependent,
  stackedDependsOn,
  upstreamConflict,
  camTwoDeps,
  internalOnly,
];

export function scenarioById(id: string | null | undefined): LabScenario {
  return LAB_SCENARIOS.find((scenario) => scenario.id === id) ?? fullLifecycle;
}
