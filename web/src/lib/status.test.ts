import { describe, expect, it } from "vitest";
import { shownStatus } from "./status";

describe("shownStatus", () => {
  it("shows queued tooling and internal patches as active", () => {
    expect(shownStatus("queued", "tooling")).toBe("active");
    expect(shownStatus("queued", "internal")).toBe("active");
  });

  it("keeps queued upstream patches queued", () => {
    expect(shownStatus("queued", "upstream")).toBe("queued");
    expect(shownStatus("queued")).toBe("queued");
    expect(shownStatus("queued", null)).toBe("queued");
  });

  it("keeps other statuses in every layer", () => {
    expect(shownStatus("conflict", "internal")).toBe("conflict");
    expect(shownStatus("dropped", "tooling")).toBe("dropped");
    expect(shownStatus("waiting", "upstream")).toBe("waiting");
  });
});
