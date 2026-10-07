import type React from "react";
import { Link, useSearchParams } from "react-router-dom";
import { DocPage } from "../components/doc-page";
import { Tabs } from "../components/ui/tabs";
import { SetupGuide } from "../components/setup/guide";
import type { Mode } from "../components/setup/step";
import type { Derive, Field } from "../components/setup/values";
import { renderRepoMarkdown } from "../lib/markdown";
import { GITHUB_FIELDS, GithubSteps, githubDerive } from "./setup/github-steps";
import githubWorkflows from "../../../templates/github/README.md?raw";
import githubAssessmentHook from "../../../templates/github-hooks/assessment-hook.md?raw";
import githubToolchainHook from "../../../templates/github-hooks/toolchain-hook.md?raw";
import { GITHUB_BLOB } from "../lib/links";

/** One entry per production-ready forge. Add a forge by adding an entry. */
const FORGES: {
  id: string;
  label: string;
  Steps: () => React.ReactNode;
  fields: Field[];
  derive: Derive;
  workflows: string;
  source: string;
  /** Guides that `git uplink init` writes to `uplink/hooks`, in reading order. */
  hooks: { id: string; title: string; markdown: string; source: string }[];
}[] = [
  {
    id: "github",
    label: "GitHub",
    Steps: GithubSteps,
    fields: GITHUB_FIELDS,
    derive: githubDerive,
    workflows: githubWorkflows,
    source: "templates/github/README.md",
    hooks: [
      {
        id: "toolchain-hook",
        title: "Toolchain hook and preflight.sh",
        markdown: githubToolchainHook,
        source: "templates/github-hooks/toolchain-hook.md",
      },
      {
        id: "assessment-hook",
        title: "Assessment hook",
        markdown: githubAssessmentHook,
        source: "templates/github-hooks/assessment-hook.md",
      },
    ],
  },
];

const VIEWS = [
  { id: "steps", label: "Steps" },
  { id: "workflows", label: "Workflows" },
  { id: "hooks", label: "Hooks" },
];

export function SetupPage() {
  const [params, setParams] = useSearchParams();
  const forgeId = FORGES.some((f) => f.id === params.get("forge")) ? params.get("forge")! : FORGES[0].id;
  const view = VIEWS.find((v) => v.id === params.get("view"))?.id ?? "steps";
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
              {
                id: "hooks",
                label: VIEWS[2].label,
                content: <HooksView hooks={forge.hooks} />,
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
      lead="Choose your forge. Steps walks you through the setup; Workflows describes what each installed workflow does and needs; Hooks covers the company-owned branch they call."
    >
      <Tabs label="Forge" items={forgeItems} value={forgeId} onChange={(id) => update({ forge: id })} />
    </DocPage>
  );
}

const HOOK_FILES = [
  ["uplink.toml", "Settings the CLI reads: redact_keywords and internal_email_domains. They decide what assess reports as a leak."],
  ["preflight.sh", "The build and test command preflight runs on the export tree and on a rebuilt main."],
  [".github/actions/uplink-toolchain-hook/action.yml", "Toolchain hook: installs what preflight.sh needs, right before it runs."],
  [".github/workflows/uplink-assessment-hook.yml", "Assessment hook (optional): company checks whose result IP reads in the packet. init writes an -example starter."],
  ["toolchain-hook.md, assessment-hook.md", "The two guides below."],
];

function HooksView({ hooks }: { hooks: (typeof FORGES)[number]["hooks"] }) {
  return (
    <div className="space-y-10">
      <div className="markdown-body">
        <p>
          Everything company-specific that the workflows run lives on the orphan branch <code>uplink/hooks</code>, not
          on <code>main</code>. It is never queued, replayed, or contributed, and gated jobs read it from the branch,
          never from a pull request.
        </p>
        <table>
          <thead>
            <tr>
              <th>File on uplink/hooks</th>
              <th>What it is</th>
            </tr>
          </thead>
          <tbody>
            {HOOK_FILES.map(([file, what]) => (
              <tr key={file}>
                <td>
                  <code>{file}</code>
                </td>
                <td>{what}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <ul>
          <li>
            <code>git uplink init</code> creates the branch locally and asks for the settings. <code>git uplink push</code>{" "}
            publishes it. <code>git uplink init --upgrade</code> adds files a newer release brings and never changes one
            that is there. <code>git uplink doctor</code> reports when it is missing or not pushed.
          </li>
          <li>
            Change a file by editing it on <code>uplink/hooks</code>, through a pull request against that branch.
          </li>
          <li>
            Its code runs on every PR check, preflight and submit, so protect it: import{" "}
            <a href={`${GITHUB_BLOB}/templates/github/.github/uplink-hooks-ruleset.json`}>uplink-hooks-ruleset.json</a>. See{" "}
            <Link to="/security#forge">Security</Link>.
          </li>
        </ul>
        <p>
          On this page:{" "}
          {hooks.map((hook, i) => (
            <span key={hook.id}>
              {i > 0 ? ", " : null}
              <a href={`#${hook.id}`}>{hook.title}</a>
            </span>
          ))}
          .
        </p>
      </div>
      {hooks.map((hook) => (
        <section key={hook.id} id={hook.id} className="scroll-mt-20 border-t border-border pt-8">
          <h2 className="text-2xl font-semibold tracking-tight text-foreground">{hook.title}</h2>
          <div
            className="markdown-body"
            dangerouslySetInnerHTML={{ __html: renderRepoMarkdown(hook.markdown, hook.source) }}
          />
        </section>
      ))}
    </div>
  );
}
