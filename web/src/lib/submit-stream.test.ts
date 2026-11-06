import { describe, expect, it } from "vitest";
import type { StreamNode } from "./api";
import { columnTitle, edgePath, edgesOf, neighbours } from "./submit-stream";

function node(id: string, dependsOn: string[] = []): StreamNode {
  return {
    id,
    title: id,
    status: "queued",
    dependsOn,
    missing: [],
    stale: false,
  };
}

describe("edgesOf", () => {
  it("links each dependency to the patch that waits on it", () => {
    const layers = [[node("a"), node("b")], [node("c", ["a", "b", "gone"])]];
    expect(edgesOf(layers)).toEqual([
      { from: "a", to: "c" },
      { from: "b", to: "c" },
    ]);
  });

  it("has no edges for independent patches", () => {
    expect(edgesOf([[node("a"), node("b")]])).toEqual([]);
    expect(edgesOf([])).toEqual([]);
  });
});

describe("neighbours", () => {
  it("lists the patch, its dependencies and its dependants", () => {
    const edges = [
      { from: "a", to: "c" },
      { from: "c", to: "d" },
      { from: "b", to: "e" },
    ];
    expect([...neighbours(edges, "c")].sort()).toEqual(["a", "c", "d"]);
  });
});

describe("edgePath", () => {
  it("curves from the right middle to the left middle", () => {
    const from = { x: 0, y: 0, width: 100, height: 40 };
    const to = { x: 200, y: 100, width: 100, height: 40 };
    expect(edgePath(from, to)).toBe("M 100 20 C 150 20, 150 120, 200 120");
  });

  it("bends at least 24px when the cards are close", () => {
    const from = { x: 0, y: 0, width: 100, height: 40 };
    const to = { x: 110, y: 0, width: 100, height: 40 };
    expect(edgePath(from, to)).toBe("M 100 20 C 124 20, 86 20, 110 20");
  });
});

describe("columnTitle", () => {
  it("names the first column submit next", () => {
    expect(columnTitle(0)).toBe("Submit next");
    expect(columnTitle(2)).toBe("Step 3");
  });
});
