import { useMemo } from "react";
import { marked } from "marked";
import { AppShell } from "../components/app-shell";
import templatesReadme from "../../../templates/README.md?raw";

export function SetupPage() {
  const html = useMemo(() => marked.parse(templatesReadme, { async: false }) as string, []);

  return (
    <AppShell>
      <article className="mx-auto max-w-3xl">
        <p className="mb-4 text-xs font-medium tracking-[0.25em] text-teal-400 uppercase">
          Forge packs
        </p>
        <div className="markdown-body" dangerouslySetInnerHTML={{ __html: html }} />
      </article>
    </AppShell>
  );
}
