import type { StreamNode } from "./api";

export type Edge = { from: string; to: string };

/** A card's box relative to the graph. */
export type Box = { x: number; y: number; width: number; height: number };

/** One edge per dependency between patches in the stream: from the dependency to the patch that waits on it. */
export function edgesOf(layers: StreamNode[][]): Edge[] {
  const ids = new Set(layers.flat().map((node) => node.id));
  return layers
    .flat()
    .flatMap((node) =>
      node.dependsOn
        .filter((dep) => ids.has(dep))
        .map((dep) => ({ from: dep, to: node.id })),
    );
}

/** Patches one edge away from `id`, and `id` itself. */
export function neighbours(edges: Edge[], id: string): Set<string> {
  const near = new Set([id]);
  for (const edge of edges) {
    if (edge.from === id) {
      near.add(edge.to);
    }
    if (edge.to === id) {
      near.add(edge.from);
    }
  }
  return near;
}

/** A curve from the right middle of `from` to the left middle of `to`. */
export function edgePath(from: Box, to: Box): string {
  const sx = from.x + from.width;
  const sy = from.y + from.height / 2;
  const ex = to.x;
  const ey = to.y + to.height / 2;
  const bend = Math.max(24, (ex - sx) / 2);
  return `M ${sx} ${sy} C ${sx + bend} ${sy}, ${ex - bend} ${ey}, ${ex} ${ey}`;
}

/** The heading of column `index`. */
export function columnTitle(index: number): string {
  return index === 0 ? "Submit next" : `Step ${index + 1}`;
}
