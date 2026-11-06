import { describe, expect, it } from "vitest";
import { parseView, sourceBranch, viewSearch } from "./view";

describe("parseView", () => {
  it("defaults to the queue", () => {
    expect(parseView(null)).toBe("queue");
    expect(parseView("bogus")).toBe("queue");
    expect(parseView("ready")).toBe("ready");
    expect(parseView("stream")).toBe("stream");
  });
});

describe("sourceBranch", () => {
  it("names the branch each source reads", () => {
    expect(sourceBranch("local")).toBe("local uplink/state");
    expect(sourceBranch("remote")).toBe("origin/uplink/state");
  });
});

describe("viewSearch", () => {
  it("leaves the defaults out", () => {
    expect(viewSearch("local", "queue")).toBe("");
    expect(viewSearch("remote", "queue")).toBe("?source=remote");
    expect(viewSearch("local", "stream")).toBe("?view=stream");
    expect(viewSearch("remote", "ready")).toBe("?source=remote&view=ready");
  });
});
