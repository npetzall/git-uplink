import { useEffect, useId, useState } from "react";
import mermaid from "mermaid";

let initialized = false;
let renderSeq = 0;

function ensureMermaid() {
  if (initialized) return;
  mermaid.initialize({
    startOnLoad: false,
    theme: "dark",
    securityLevel: "strict",
    fontFamily: "ui-sans-serif, system-ui, sans-serif",
    flowchart: { useMaxWidth: true, htmlLabels: false },
    sequence: { useMaxWidth: false },
  });
  initialized = true;
}

export function MermaidDiagram({ chart, title }: { chart: string; title: string }) {
  const reactId = useId().replace(/:/g, "");
  const [svg, setSvg] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    ensureMermaid();
    const id = `uplinkMermaid${reactId}${++renderSeq}`;
    mermaid
      .render(id, chart.trim())
      .then(({ svg: next }) => {
        if (!cancelled) {
          setSvg(next);
          setError(null);
        }
      })
      .catch((err: unknown) => {
        if (!cancelled) {
          setSvg("");
          setError(err instanceof Error ? err.message : "Failed to render diagram");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [chart, reactId]);

  if (error) {
    return (
      <p role="alert" className="rounded-xl border border-rose-500/30 bg-rose-500/10 p-4 text-sm text-rose-100">
        {error}
      </p>
    );
  }

  return (
    <figure className="space-y-2">
      <div
        className="mermaid-diagram overflow-x-auto rounded-xl border border-border bg-card p-4 [&_svg]:mx-auto"
        dangerouslySetInnerHTML={svg ? { __html: svg } : undefined}
        aria-label={title}
      />
      <figcaption className="text-xs text-muted-foreground">{title}</figcaption>
    </figure>
  );
}
