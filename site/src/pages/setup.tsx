import type React from "react";
import { useSearchParams } from "react-router-dom";
import { marked } from "marked";
import { DocPage } from "../components/doc-page";
import { Tabs } from "../components/ui/tabs";
import { ModeContext, ModeToggle, type Mode } from "../components/setup/step";
import { SetupValuesForm, SetupValuesProvider } from "../components/setup/values";
import { GITHUB_BLOB } from "../lib/links";
import { GhecSteps } from "./setup/ghec-steps";
import ghecWorkflows from "../../../templates/ghec/README.md?raw";

/** One entry per production-ready forge. Add a forge by adding an entry. */
const FORGES: { id: string; label: string; Steps: () => React.ReactNode; workflows: string; source: string }[] = [
  {
    id: "ghec",
    label: "GitHub Enterprise Cloud",
    Steps: GhecSteps,
    workflows: ghecWorkflows,
    source: "templates/ghec/README.md",
  },
];

const VIEWS = [
  { id: "steps", label: "Steps" },
  { id: "workflows", label: "Workflows" },
];

/** Render a repo markdown file, pointing its relative links at the file's location on GitHub. */
function renderRepoMarkdown(markdown: string, source: string): string {
  const base = `${GITHUB_BLOB}/${source}`;
  const html = marked.parse(markdown.replace(/^# .*\n/, ""), { async: false }) as string;
  return html.replace(/href="(?![a-z]+:|#|\/)([^"]+)"/g, (_, href: string) => `href="${new URL(href, base)}"`);
}

export function SetupPage() {
  const [params, setParams] = useSearchParams();
  const forgeId = FORGES.some((f) => f.id === params.get("forge")) ? params.get("forge")! : FORGES[0].id;
  const view = params.get("view") === "workflows" ? "workflows" : "steps";
  const mode: Mode = params.get("mode") === "gh" ? "gh" : "manual";

  function update(next: Record<string, string>) {
    const merged = new URLSearchParams(params);
    for (const [key, value] of Object.entries(next)) merged.set(key, value);
    setParams(merged, { replace: true });
  }

  const forgeItems = FORGES.map((forge) => ({
        id: forge.id,
        label: forge.label,
        content: (
          <Tabs
            label="View"
            items={[
              {
                id: "steps",
                label: VIEWS[0].label,
                content: (
                  <ModeContext.Provider value={mode}>
                    <div className="space-y-8">
                      <div className="flex flex-wrap items-center justify-between gap-3">
                        <p className="text-sm text-muted-foreground">
                          Every preparation, in order, ending with{" "}
                          <code className="rounded bg-muted px-1.5 py-0.5 text-[13px] text-foreground">git uplink init</code> and
                          the first push.
                        </p>
                        <ModeToggle mode={mode} onChange={(m) => update({ mode: m })} />
                      </div>
                      <SetupValuesForm />
                      <forge.Steps />
                    </div>
                  </ModeContext.Provider>
                ),
              },
              {
                id: "workflows",
                label: VIEWS[1].label,
                content: (
                  <div
                    className="markdown-body"
                    dangerouslySetInnerHTML={{ __html: renderRepoMarkdown(forge.workflows, forge.source) }}
                  />
                ),
              },
            ]}
            value={view}
            onChange={(v) => update({ view: v })}
          />
        ),
      }));

  return (
    <DocPage
      eyebrow="Production setup"
      title="Set up Uplink for a product repository"
      lead="Choose your forge. Steps walks you through the setup; Workflows describes what each installed workflow does and needs."
    >
      <SetupValuesProvider>
        <Tabs label="Forge" items={forgeItems} value={forgeId} onChange={(id) => update({ forge: id })} />
      </SetupValuesProvider>
    </DocPage>
  );
}
