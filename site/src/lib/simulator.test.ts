import { describe, expect, it } from "vitest";
import { operationsFor, runThrough } from "./simulator";
import { LAB_SCENARIOS, scenarioById } from "./lab/scenarios";

const fullLifecycle = scenarioById("full-lifecycle");

function stepCount(id: string, scenario = fullLifecycle): number {
  return scenario.steps.findIndex((step) => step.id === id) + 1;
}

function contribTokens(state: ReturnType<typeof runThrough>): string[] {
  return state.contrib.map((branch) => branch.files["src/tokens.js"] ?? "");
}

describe("full-lifecycle lab scenario", () => {
  it("drops the merged hash patch so the salted upstream fix remains", () => {
    const merged = runThrough(fullLifecycle, stepCount("upstream-merges"));
    expect(merged.patches.find((patch) => patch.id === "upl_hash")?.status).toBe("merged");
    expect(merged.company["src/tokens.js"]).toContain("saltedSha256");
    expect(merged.company["src/tokens.js"]).not.toMatch(/return sha256\(value\)/);
    expect(merged.company["src/tokens.js"]).toContain("vendorTelemetry");
  });

  it("keeps internal-only patches off the contribution fork until a later export of other work", () => {
    const submitted = runThrough(fullLifecycle, stepCount("approve-submit"));
    expect(submitted.contrib[0]?.files["src/tokens.js"]).toContain("sha256");
    expect(submitted.contrib[0]?.files["src/tokens.js"]).not.toContain("vendorTelemetry");
  });

  it("never puts vendorTelemetry on the contribution fork", () => {
    for (let count = 0; count <= fullLifecycle.steps.length; count += 1) {
      for (const tokens of contribTokens(runThrough(fullLifecycle, count))) {
        expect(tokens).not.toContain("vendorTelemetry");
      }
    }
  });

  it("does not export upl_logs until to-upstream approval", () => {
    const beforeSubmit = runThrough(fullLifecycle, stepCount("resolve"));
    expect(beforeSubmit.patches.find((patch) => patch.id === "upl_logs")?.status).toBe("queued");
    expect(beforeSubmit.contrib.some((branch) => branch.branch === "uplink/upl_logs")).toBe(false);

    const submitted = runThrough(fullLifecycle, stepCount("submit-logs"));
    expect(submitted.patches.find((patch) => patch.id === "upl_logs")?.status).toBe("submitted");
    const logs = submitted.contrib.find((branch) => branch.branch === "uplink/upl_logs");
    expect(logs?.prNumber).toBe(418);
    expect(logs?.files["src/tokens.js"]).toContain('console.log("hash", value)');
    expect(logs?.files["src/tokens.js"]).toContain("const digest = saltedSha256(value)");
    expect(logs?.files["src/tokens.js"]).not.toContain("vendorTelemetry");
  });

  it("freezes company main and blocks later patches while upl_logs conflicts", () => {
    const before = runThrough(fullLifecycle, stepCount("upstream-merges"));
    const stopped = runThrough(fullLifecycle, stepCount("conflict"));
    expect(stopped.company).toEqual(before.company);
    expect(stopped.company["src/tokens.js"]).toContain('console.log("hash", value)');
    expect(stopped.upstream["src/tokens.js"]).toContain("const digest = saltedSha256(value)");
    expect(stopped.conflict?.branch).toBe("uplink/conflict/upl_logs");
    expect(stopped.conflict?.phase).toBe("stopped");
    expect(stopped.conflict?.files["src/tokens.js"]).toContain("<<<<<<<");
    expect(stopped.conflict?.blockedIds).toContain("upl_vendor");
    expect(stopped.patches.find((patch) => patch.id === "upl_logs")?.status).toBe("conflict");
    expect(stopped.contrib).toEqual([]);
  });

  it("fixes the conflict branch before resolve, then rebuilds without exporting", () => {
    const fixed = runThrough(fullLifecycle, stepCount("conflict-fix"));
    expect(fixed.conflict?.phase).toBe("fixed");
    expect(fixed.conflict?.files["src/tokens.js"]).not.toContain("<<<<<<<");
    expect(fixed.conflict?.files["src/tokens.js"]).toContain("const digest = saltedSha256(value)");
    expect(fixed.patches.find((patch) => patch.id === "upl_logs")?.status).toBe("conflict");
    expect(fixed.contrib).toEqual([]);

    const resolved = runThrough(fullLifecycle, stepCount("resolve"));
    expect(resolved.conflict).toBeUndefined();
    expect(resolved.patches.find((patch) => patch.id === "upl_logs")?.status).toBe("queued");
    expect(resolved.company["src/tokens.js"]).toContain("const digest = saltedSha256(value)");
    expect(resolved.company["src/tokens.js"]).toContain('console.log("hash", value)');
    expect(resolved.company["src/tokens.js"]).toContain("vendorTelemetry");
    expect(resolved.contrib).toEqual([]);
  });

  it("runs every lab step without throwing", () => {
    for (let count = 0; count <= fullLifecycle.steps.length; count += 1) {
      const state = runThrough(fullLifecycle, count);
      expect(state.log.length).toBeGreaterThan(0);
    }
    const last = runThrough(fullLifecycle, fullLifecycle.steps.length);
    expect(last.stepId).toBe("submit-logs");
    expect(last.company["src/tokens.js"]).toContain("vendorTelemetry");
  });
});

describe("lab scenarios", () => {
  it("runs every scenario step without throwing", () => {
    for (const scenario of LAB_SCENARIOS) {
      for (let count = 0; count <= scenario.steps.length; count += 1) {
        const state = runThrough(scenario, count);
        expect(state.log.length).toBeGreaterThan(0);
      }
    }
  });

  it("gives every step at least one manual operation", () => {
    for (const scenario of LAB_SCENARIOS) {
      for (const step of scenario.steps) {
        expect(
          operationsFor(step.operations, "manual").length,
          `${scenario.id}/${step.id}`,
        ).toBeGreaterThan(0);
      }
    }
  });

  it("lists only git uplink or gh commands in CI mode", () => {
    const allowed = /^(git uplink|gh)\b/;
    for (const scenario of LAB_SCENARIOS) {
      for (const step of scenario.steps) {
        for (const op of operationsFor(step.operations, "ci")) {
          for (const command of op.commands) {
            expect(command, `${scenario.id}/${step.id}: ${command}`).toMatch(allowed);
            expect(command).not.toMatch(/\bgit checkout\b/);
            expect(command).not.toMatch(/\bgit commit\b/);
          }
        }
      }
    }
  });

  it("keeps Asha queued after Ben merges upstream first", () => {
    const scenario = scenarioById("parallel-independent");
    const afterBen = runThrough(scenario, stepCount("merge-ben-first", scenario));
    expect(afterBen.patches.find((patch) => patch.id === "upl_ben")?.status).toBe("merged");
    expect(afterBen.patches.find((patch) => patch.id === "upl_asha")?.status).toBe("submitted");
    expect(afterBen.company["src/tokens.js"]).toContain("sha256");
    expect(afterBen.company["src/tokens.js"]).toContain("return 7200");
    expect(afterBen.upstream["src/tokens.js"]).toContain("return 7200");
    expect(afterBen.upstream["src/tokens.js"]).not.toContain("sha256");
  });

  it("never puts companyTelemetry on the contribution fork", () => {
    const scenario = scenarioById("internal-only");
    for (let count = 0; count <= scenario.steps.length; count += 1) {
      for (const tokens of contribTokens(runThrough(scenario, count))) {
        expect(tokens).not.toContain("companyTelemetry");
      }
    }
    const refused = runThrough(scenario, scenario.steps.length);
    expect(refused.patches.find((patch) => patch.id === "upl_telemetry")?.queue).toBe("internal");
    expect(refused.contrib).toEqual([]);
  });

  it("submits Cam only after both siblings have merged", () => {
    const scenario = scenarioById("cam-two-deps");
    const afterSiblings = runThrough(scenario, stepCount("merge-siblings", scenario));
    expect(afterSiblings.patches.find((patch) => patch.id === "upl_asha")?.status).toBe("merged");
    expect(afterSiblings.patches.find((patch) => patch.id === "upl_ben")?.status).toBe("merged");
    expect(afterSiblings.patches.find((patch) => patch.id === "upl_cam")?.status).toBe("queued");
    expect(afterSiblings.contrib).toEqual([]);

    const submitted = runThrough(scenario, stepCount("submit-cam", scenario));
    expect(submitted.patches.find((patch) => patch.id === "upl_cam")?.status).toBe("submitted");
    const cam = submitted.contrib.find((branch) => branch.branch === "uplink/upl_cam");
    expect(cam?.prNumber).toBe(420);
    expect(cam?.files["src/tokens.js"]).toContain("describeToken");
  });

  it("prepends assessment-hook extras and never exports the workflow file", () => {
    const scenario = scenarioById("assessment-hook");
    const submitted = runThrough(scenario, scenario.steps.length);
    expect(submitted.company[".github/workflows/uplink-assessment-hook.yml"]).toContain(
      "Uplink assessment hook",
    );
    expect(submitted.patches.find((patch) => patch.id === "upl_hook")?.queue).toBe("internal");
    expect(submitted.patches.find((patch) => patch.id === "upl_asha")?.status).toBe("submitted");
    const asha = submitted.contrib.find((branch) => branch.branch === "uplink/upl_asha");
    expect(asha?.files["src/tokens.js"]).toContain("sha256");
    expect(JSON.stringify(asha?.files)).not.toContain("uplink-assessment-hook.yml");
  });
});
