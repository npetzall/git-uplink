import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";
import {
  loadSubmitStream,
  type QueueSource,
  type StreamNode,
  type SubmitStreamResponse,
} from "../lib/api";
import { NEEDS_APPROVAL } from "../lib/status";
import {
  type Box,
  columnTitle,
  edgePath,
  edgesOf,
  neighbours,
} from "../lib/submit-stream";
import { cn } from "../lib/utils";
import { sourceBranch } from "../lib/view";
import { PatchIdLink, patchHref } from "./patch-id-link";
import { StatusBadge } from "./status-badge";

function NodeCard({
  node,
  first,
  search,
  dimmed,
  onHover,
  cardRef,
}: {
  node: StreamNode;
  first: boolean;
  search: string;
  dimmed: boolean;
  onHover: (id: string | null) => void;
  cardRef: (element: HTMLDivElement | null) => void;
}) {
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: hover only highlights edges; the links inside are the controls
    <div
      ref={cardRef}
      onMouseEnter={() => onHover(node.id)}
      onMouseLeave={() => onHover(null)}
      onFocus={() => onHover(node.id)}
      onBlur={() => onHover(null)}
      className={cn(
        "relative z-10 w-64 space-y-2 rounded-xl border bg-card p-3 text-sm shadow-sm transition-opacity",
        first ? "border-primary/50 ring-1 ring-primary/30" : "border-border",
        dimmed && "opacity-30",
      )}
    >
      <PatchIdLink id={node.id} search={search} />
      <Link
        to={patchHref(node.id, search)}
        className="line-clamp-2 block font-medium hover:underline"
        title={node.title}
      >
        {node.title}
      </Link>
      <div className="flex flex-wrap gap-1">
        <StatusBadge value={node.status} />
        {node.stale ? <StatusBadge value={NEEDS_APPROVAL} /> : null}
      </div>
      {node.missing.length ? (
        <p className="text-xs text-amber-300">
          Waits on <span className="font-mono">{node.missing.join(", ")}</span>,
          not an active upstream patch.
        </p>
      ) : null}
    </div>
  );
}

export function SubmitStream({
  source,
  search,
  reload,
}: {
  source: QueueSource;
  /** Query string carried to the patch page. */
  search: string;
  /** Changes when the stream should be read again, for example after a refresh. */
  reload: number;
}) {
  const [data, setData] = useState<SubmitStreamResponse | null>(null);
  const [hovered, setHovered] = useState<string | null>(null);
  const [paths, setPaths] = useState<
    { key: string; from: string; to: string; d: string }[]
  >([]);
  const graph = useRef<HTMLDivElement>(null);
  const cards = useRef(new Map<string, HTMLDivElement>());

  useEffect(() => {
    setData(null);
    loadSubmitStream(source)
      .then(setData)
      .catch((err: Error) =>
        setData({ present: false, source, error: err.message, layers: [] }),
      );
    // reload is a trigger: a new value reads the stream again.
  }, [source, reload]);

  const layers = useMemo(() => data?.layers ?? [], [data]);
  const edges = useMemo(() => edgesOf(layers), [layers]);
  const near = hovered ? neighbours(edges, hovered) : null;

  useLayoutEffect(() => {
    const root = graph.current;
    if (!root) {
      return;
    }
    function measure() {
      if (!root) {
        return;
      }
      const origin = root.getBoundingClientRect();
      const box = (id: string): Box | null => {
        const rect = cards.current.get(id)?.getBoundingClientRect();
        return rect
          ? {
              x: rect.left - origin.left,
              y: rect.top - origin.top,
              width: rect.width,
              height: rect.height,
            }
          : null;
      };
      setPaths(
        edges.flatMap((edge) => {
          const from = box(edge.from);
          const to = box(edge.to);
          return from && to
            ? [
                {
                  key: `${edge.from}>${edge.to}`,
                  ...edge,
                  d: edgePath(from, to),
                },
              ]
            : [];
        }),
      );
    }
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    for (const card of cards.current.values()) {
      observer.observe(card);
    }
    return () => observer.disconnect();
  }, [edges]);

  if (!data) {
    return <p className="text-sm text-muted-foreground">Reading queue…</p>;
  }
  if (data.error) {
    return <p className="text-sm text-rose-300">{data.error}</p>;
  }
  if (!layers.length) {
    return (
      <p className="text-sm text-muted-foreground">
        No active upstream patches.
      </p>
    );
  }

  return (
    <div className="space-y-4">
      <p className="max-w-3xl text-sm text-muted-foreground">
        Active upstream patches on {sourceBranch(source)}, in the order they can
        go upstream. The first column has no open dependencies. A patch in a
        later column goes once every patch it depends on is merged. Hover a
        patch to trace its dependencies.
      </p>
      <div className="overflow-x-auto pb-2">
        <div ref={graph} className="relative flex w-max gap-16 p-1">
          <svg
            aria-hidden="true"
            className="pointer-events-none absolute inset-0 size-full overflow-visible"
          >
            {paths.map((path) => {
              const lit =
                hovered !== null &&
                (path.from === hovered || path.to === hovered);
              return (
                <path
                  key={path.key}
                  d={path.d}
                  fill="none"
                  strokeWidth={lit ? 2 : 1.5}
                  className={cn(
                    "transition-opacity",
                    lit ? "stroke-primary" : "stroke-muted-foreground/40",
                    hovered !== null && !lit && "opacity-20",
                  )}
                />
              );
            })}
          </svg>
          {layers.map((column, index) => (
            <section
              key={column.map((node) => node.id).join(",")}
              className="flex flex-col gap-3"
              aria-label={columnTitle(index)}
            >
              <h3
                className={cn(
                  "text-xs font-medium tracking-wide uppercase",
                  index === 0 ? "text-primary" : "text-muted-foreground",
                )}
              >
                {columnTitle(index)}
                <span className="ml-2 font-mono normal-case">
                  {column.length}
                </span>
              </h3>
              {column.map((node) => (
                <NodeCard
                  key={node.id}
                  node={node}
                  first={index === 0}
                  search={search}
                  dimmed={near !== null && !near.has(node.id)}
                  onHover={setHovered}
                  cardRef={(element) => {
                    if (element) {
                      cards.current.set(node.id, element);
                    } else {
                      cards.current.delete(node.id);
                    }
                  }}
                />
              ))}
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
