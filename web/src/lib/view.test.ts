import { describe, expect, it } from "vitest";
import {
  parseShown,
  parseView,
  patchSubject,
  viewSearch,
  visiblePatches,
} from "./view";

describe("parseView", () => {
  it("defaults to the queue", () => {
    expect(parseView(null)).toBe("queue");
    expect(parseView("bogus")).toBe("queue");
    expect(parseView("ready")).toBe("ready");
    expect(parseView("stream")).toBe("stream");
  });
});

describe("viewSearch", () => {
  it("leaves the defaults out", () => {
    expect(viewSearch("queue")).toBe("");
    expect(viewSearch("stream")).toBe("?view=stream");
    expect(viewSearch("ready")).toBe("?view=ready");
    expect(viewSearch("queue", [])).toBe("");
  });

  it("writes shown finished statuses in a fixed order", () => {
    expect(viewSearch("queue", ["internal-dropped", "upstream-merged"])).toBe(
      "?show=upstream-merged%2Cinternal-dropped",
    );
  });
});

describe("parseShown", () => {
  it("shows nothing finished by default", () => {
    expect(parseShown(null)).toEqual([]);
    expect(parseShown("")).toEqual([]);
  });

  it("keeps known tokens and drops the rest", () => {
    expect(parseShown("internal-dropped,bogus,upstream-merged")).toEqual([
      "upstream-merged",
      "internal-dropped",
    ]);
    expect(parseShown("internal-merged")).toEqual([]);
  });
});

describe("visiblePatches", () => {
  const patches = [
    { status: "queued" },
    { status: "merged" },
    { status: "dropped" },
  ];

  it("hides merged and dropped upstream patches by default", () => {
    expect(visiblePatches(patches, "upstream", [])).toEqual([
      { status: "queued" },
    ]);
  });

  it("shows the finished statuses that are switched on", () => {
    expect(visiblePatches(patches, "upstream", ["upstream-merged"])).toEqual([
      { status: "queued" },
      { status: "merged" },
    ]);
    expect(visiblePatches(patches, "upstream", ["internal-dropped"])).toEqual([
      { status: "queued" },
    ]);
  });

  it("only hides dropped internal patches", () => {
    expect(visiblePatches(patches, "internal", [])).toEqual([
      { status: "queued" },
      { status: "merged" },
    ]);
    expect(visiblePatches(patches, "internal", ["internal-dropped"])).toEqual(
      patches,
    );
  });
});

describe("patchSubject", () => {
  it("is the first non-empty line of the message", () => {
    expect(patchSubject({ commitMessage: "\nUse SHA-256\n\nWhy." })).toBe(
      "Use SHA-256",
    );
    expect(patchSubject({ commitMessage: "" })).toBe("");
  });
});
