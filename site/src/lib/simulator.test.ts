import { describe, expect, it } from "vitest";
import { LAB_STEPS, runThrough } from "./simulator";

function stepCount(id: string): number {
  return LAB_STEPS.findIndex((step) => step.id === id) + 1;
}

function contribTokens(state: ReturnType<typeof runThrough>): string[] {
  return state.contrib.map((branch) => branch.files["src/tokens.js"] ?? "");
}

describe("live lab scenario", () => {
  it("drops the merged hash patch so the salted upstream fix remains", () => {
    const merged = runThrough(stepCount("upstream-merges"));
    expect(merged.patches.find((patch) => patch.id === "upl_hash")?.status).toBe("merged");
    expect(merged.company["src/tokens.js"]).toContain("saltedSha256");
    expect(merged.company["src/tokens.js"]).not.toMatch(/return sha256\(value\)/);
    expect(merged.company["src/tokens.js"]).toContain("vendorTelemetry");
  });

  it("keeps internal-only patches off the contribution fork until a later export of other work", () => {
    const submitted = runThrough(stepCount("approve-submit"));
    expect(submitted.contrib[0]?.files["src/tokens.js"]).toContain("sha256");
    expect(submitted.contrib[0]?.files["src/tokens.js"]).not.toContain("vendorTelemetry");
  });

  it("never puts vendorTelemetry on the contribution fork", () => {
    for (let count = 0; count <= LAB_STEPS.length; count += 1) {
      for (const tokens of contribTokens(runThrough(count))) {
        expect(tokens).not.toContain("vendorTelemetry");
      }
    }
  });

  it("does not export upl_logs until to-upstream approval", () => {
    const beforeSubmit = runThrough(stepCount("resolve"));
    expect(beforeSubmit.patches.find((patch) => patch.id === "upl_logs")?.status).toBe("queued");
    expect(beforeSubmit.contrib.some((branch) => branch.branch === "uplink/upl_logs")).toBe(false);

    const submitted = runThrough(stepCount("submit-logs"));
    expect(submitted.patches.find((patch) => patch.id === "upl_logs")?.status).toBe("submitted");
    const logs = submitted.contrib.find((branch) => branch.branch === "uplink/upl_logs");
    expect(logs?.prNumber).toBe(418);
    expect(logs?.files["src/tokens.js"]).toContain('console.log("hash", value)');
    expect(logs?.files["src/tokens.js"]).toContain("const digest = saltedSha256(value)");
    expect(logs?.files["src/tokens.js"]).not.toContain("vendorTelemetry");
  });

  it("freezes company main and blocks later patches while upl_logs conflicts", () => {
    const before = runThrough(stepCount("upstream-merges"));
    const stopped = runThrough(stepCount("conflict"));
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
    const fixed = runThrough(stepCount("conflict-fix"));
    expect(fixed.conflict?.phase).toBe("fixed");
    expect(fixed.conflict?.files["src/tokens.js"]).not.toContain("<<<<<<<");
    expect(fixed.conflict?.files["src/tokens.js"]).toContain("const digest = saltedSha256(value)");
    expect(fixed.patches.find((patch) => patch.id === "upl_logs")?.status).toBe("conflict");
    expect(fixed.contrib).toEqual([]);

    const resolved = runThrough(stepCount("resolve"));
    expect(resolved.conflict).toBeUndefined();
    expect(resolved.patches.find((patch) => patch.id === "upl_logs")?.status).toBe("queued");
    expect(resolved.company["src/tokens.js"]).toContain("const digest = saltedSha256(value)");
    expect(resolved.company["src/tokens.js"]).toContain('console.log("hash", value)');
    expect(resolved.company["src/tokens.js"]).toContain("vendorTelemetry");
    expect(resolved.contrib).toEqual([]);
  });

  it("runs every lab step without throwing", () => {
    for (let count = 0; count <= LAB_STEPS.length; count += 1) {
      const state = runThrough(count);
      expect(state.log.length).toBeGreaterThan(0);
    }
    const last = runThrough(LAB_STEPS.length);
    expect(last.stepId).toBe("submit-logs");
    expect(last.company["src/tokens.js"]).toContain("vendorTelemetry");
  });
});
