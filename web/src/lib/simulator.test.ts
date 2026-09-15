import { describe, expect, it } from "vitest";
import { LAB_STEPS, runThrough } from "./simulator";

describe("live lab scenario", () => {
  it("drops the merged hash patch so the salted upstream fix remains", () => {
    const merged = runThrough(
      LAB_STEPS.findIndex((step) => step.id === "upstream-merges") + 1,
    );
    expect(merged.patches.find((patch) => patch.id === "upl_hash")?.status).toBe("merged");
    expect(merged.company["src/tokens.js"]).toContain("saltedSha256");
    expect(merged.company["src/tokens.js"]).not.toMatch(/return sha256\(value\)/);
    expect(merged.company["src/tokens.js"]).toContain("vendorTelemetry");
  });

  it("keeps internal-only patches off the contribution fork until a later export of other work", () => {
    const submitted = runThrough(
      LAB_STEPS.findIndex((step) => step.id === "approve-submit") + 1,
    );
    expect(submitted.contrib[0]?.files["src/tokens.js"]).toContain("sha256");
    expect(submitted.contrib[0]?.files["src/tokens.js"]).not.toContain("vendorTelemetry");
  });

  it("runs every lab step without throwing", () => {
    for (let count = 0; count <= LAB_STEPS.length; count += 1) {
      const state = runThrough(count);
      expect(state.log.length).toBeGreaterThan(0);
    }
    const last = runThrough(LAB_STEPS.length);
    expect(last.stepId).toBe("amend");
    expect(last.company["src/tokens.js"]).toContain("vendorTelemetry");
  });
});
