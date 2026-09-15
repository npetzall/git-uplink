import { useEffect, useState } from "react";
import { marked } from "marked";
import { AppShell } from "../components/app-shell";

export function WorkingPage() {
  const [html, setHtml] = useState("<p>Loading way of working…</p>");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    fetch("/docs/way-of-working.md")
      .then(async (response) => {
        if (!response.ok) throw new Error(`Could not load document (${response.status})`);
        return response.text();
      })
      .then((markdown) => setHtml(marked.parse(markdown, { async: false }) as string))
      .catch((err: Error) => setError(err.message));
  }, []);

  return (
    <AppShell>
      <article className="mx-auto max-w-3xl">
        <p className="mb-4 text-xs font-medium tracking-[0.25em] text-teal-400 uppercase">
          Developer stories
        </p>
        {error ? (
          <p className="text-sm text-rose-300">{error}</p>
        ) : (
          <div className="markdown-body" dangerouslySetInnerHTML={{ __html: html }} />
        )}
      </article>
    </AppShell>
  );
}
