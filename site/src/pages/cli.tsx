import { marked } from "marked";
import { useMemo } from "react";
import cliReference from "../../../docs/cli.md?raw";
import { DocPage } from "../components/doc-page";

export function CliPage() {
  const html = useMemo(
    () => marked.parse(cliReference, { async: false }) as string,
    [],
  );

  return (
    <DocPage eyebrow="CLI">
      <div
        className="markdown-body"
        // biome-ignore lint/security/noDangerouslySetInnerHtml: Markdown from this repository, rendered at build time
        dangerouslySetInnerHTML={{ __html: html }}
      />
    </DocPage>
  );
}
