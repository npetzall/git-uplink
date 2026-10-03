import type React from "react";
import { useSearchParams } from "react-router-dom";
import { DocPage } from "../components/doc-page";
import { Tabs } from "../components/ui/tabs";
import { SetupGuide } from "../components/setup/guide";
import type { Mode } from "../components/setup/step";
import type { Derive, Field } from "../components/setup/values";
import { renderRepoMarkdown } from "../lib/markdown";
import { GITHUB_FIELDS, GithubSteps, githubDerive } from "./setup/github-steps";
import githubWorkflows from "../../../templates/github/README.md?raw";

/** One entry per production-ready forge. Add a forge by adding an entry. */
const FORGES: {
  id: string;
  label: string;
  Steps: () => React.ReactNode;
  fields: Field[];
  derive: Derive;
  workflows: string;
  source: string;
}[] = [
  {
    id: "github",
    label: "GitHub",
    Steps: GithubSteps,
    fields: GITHUB_FIELDS,
    derive: githubDerive,
    workflows: githubWorkflows,
    source: "templates/github/README.md",
  },
];

const VIEWS = [
  { id: "steps", label: "Steps" },
  { id: "workflows", label: "Workflows" },
];

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
                  <SetupGuide
                    fields={forge.fields}
                    storageKey={`uplink-setup-values-${forge.id}`}
                    derive={forge.derive}
                    mode={mode}
                    onMode={(m) => update({ mode: m })}
                    intro={
                      <>
                        Every preparation, in order, ending with <code>git uplink init</code> and the first push.
                      </>
                    }
                  >
                    <forge.Steps />
                  </SetupGuide>
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
      <Tabs label="Forge" items={forgeItems} value={forgeId} onChange={(id) => update({ forge: id })} />
    </DocPage>
  );
}
