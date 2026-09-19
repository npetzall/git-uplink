import { useMemo } from "react";
import { marked } from "marked";
import { AppShell } from "../components/app-shell";
import wayOfWorking from "../../../way-of-working.md?raw";

export function WorkingPage() {
  const html = useMemo(() => marked.parse(wayOfWorking, { async: false }) as string, []);

  return (
    <AppShell>
      <article className="mx-auto max-w-3xl">
        <p className="mb-4 text-xs font-medium tracking-[0.25em] text-teal-400 uppercase">
          Way of working
        </p>
        <div className="markdown-body" dangerouslySetInnerHTML={{ __html: html }} />
      </article>
    </AppShell>
  );
}
