import { useMemo } from "react";
import { marked } from "marked";
import { DocPage } from "../components/doc-page";
import cliReference from "../../../docs/cli.md?raw";

export function CliPage() {
  const html = useMemo(() => marked.parse(cliReference, { async: false }) as string, []);

  return (
    <DocPage eyebrow="CLI">
      <div className="markdown-body" dangerouslySetInnerHTML={{ __html: html }} />
    </DocPage>
  );
}
