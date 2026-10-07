import { Link } from "react-router-dom";
import { DocPage, Section, Code } from "../components/doc-page";
import { MermaidDiagram } from "../components/mermaid-diagram";

const TOPOLOGY_CHART = `
flowchart LR
  subgraph private [Private forge]
    dev["Developer PR"] -->|merge| main["company main"]
    state["uplink/state<br/>patch queue"]
  end
  subgraph public [Public GitHub]
    fork["Contribution fork<br/>uplink/id"]
    up["Upstream main"]
  end
  main -. "import records patch" .-> state
  state -->|"submit after IP approval"| fork
  fork -->|"public PR, maintainer merge"| up
  up -->|"sync"| main
`;

const PHASES = [
  ["Write", "Branch from company main. Public rationale in the PR title and above the cutoff.", "One internal branch"],
  ["Assess + preflight", "Run on the internal PR.", "Scrubbed message, rewritten author, affiliation scan; the change applies and builds on public upstream"],
  ["Product gate", "Review and merge.", "Queued: in the company build"],
  ["IP gate", "IP reviews the packet and approves the to-upstream Environment.", "Approved"],
  ["Submit", "Same run pushes to the contribution fork and opens the public PR.", "Submitted: first time the change is public"],
  ["Upstream merge", "Maintainers merge as usual.", "Sync detects it and marks the patch merged"],
];

export function HowPage() {
  return (
    <DocPage
      eyebrow="How"
      title="One patch per change, everything else derived"
      lead={
        <>
          Every change is a patch with a stable id in a queue. Company <code>main</code>, the public fork branch,
          and the public pull request are all built from it.
        </>
      }
    >
      <Section title="The invariant">
        <p>
          There is exactly one object per logical change: a patch with a stable id (<code>upl_…</code>) stored
          under <code>.uplink/</code> on the orphan branch <code>uplink/state</code>. Company <code>main</code> is
          always a replay:
        </p>
        <Code>{`public upstream/main  +  tooling  +  active upstream[]  +  active internal[]`}</Code>
        <ul>
          <li>
            <strong>Tooling</strong> is the Uplink workflow pack, installed by <code>init</code>.
          </li>
          <li>
            <strong>upstream[]</strong> holds changes bound for contribution: the default.
          </li>
          <li>
            <strong>internal[]</strong> holds internal-only changes. They always apply last and are never
            exported.
          </li>
          <li>
            Within a queue, patches apply in insertion order, or dependency order where{" "}
            <code>dependsOn</code> is recorded.
          </li>
        </ul>
        <p>
          When a conflict is fixed or review feedback is addressed, the same patch is amended. Nobody maintains a
          second copy.
        </p>
      </Section>

      <Section title="Three repositories">
        <MermaidDiagram chart={TOPOLOGY_CHART} title="Where a change lives, from pull request to upstream" />
        <ul>
          <li>
            <strong>Company product repository</strong> on the private forge. A mirror of upstream plus the queue,
            not a member of the public fork network. The only place humans open pull requests.
          </li>
          <li>
            <strong>Contribution fork</strong>: a public fork owned by the upstream side. A bot pushes{" "}
            <code>uplink/&lt;id&gt;</code> there only after IP approval.
          </li>
          <li>
            <strong>Canonical upstream</strong>: maintainers merge with their normal button. Squash or rebase
            merges are fine.
          </li>
        </ul>
      </Section>

      <Section title="Life of a change">
        <div className="overflow-x-auto">
          <table className="w-full min-w-[640px] text-left text-sm">
            <thead className="text-xs tracking-wide uppercase">
              <tr className="border-b">
                <th className="py-2 pr-3 font-medium">Phase</th>
                <th className="py-2 pr-3 font-medium">What happens</th>
                <th className="py-2 font-medium">Result</th>
              </tr>
            </thead>
            <tbody>
              {PHASES.map(([phase, what, result]) => (
                <tr key={phase} className="border-b border-border/60 align-top">
                  <td className="py-2 pr-3 whitespace-nowrap text-foreground">{phase}</td>
                  <td className="py-2 pr-3">{what}</td>
                  <td className="py-2">{result}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Section>

      <Section title="Checks before merge">
        <p>
          A developer branches from company <code>main</code>, which already contains everyone&apos;s queued work.
          That makes it easy to write a change that only works on the company tree. Two required checks on the
          internal pull request catch that before merge:
        </p>
        <ul>
          <li>
            <strong>Assess</strong> rewrites the change as the contribution it will become. The PR title and body
            become the commit message, everything below the cutoff is removed, and the author is set to the
            export identity. The export is then scanned for company names and internal email domains.
          </li>
          <li>
            <strong>Export preflight</strong> applies the change onto public upstream plus its declared
            dependencies, then runs the product&apos;s build and tests there, not on company <code>main</code>.
          </li>
        </ul>
        <p>Both run again at submit, so nothing incomplete reaches the public pull request.</p>
      </Section>

      <Section title="Staying current with upstream">
        <p>Sync runs hourly. It fetches public upstream and sorts every new commit into one of two kinds:</p>
        <ul>
          <li>
            <strong>Flow-back</strong>: one of our patches, merged upstream. It is recognized and applied
            immediately, and the patch is marked merged, so it is never applied again.
          </li>
          <li>
            <strong>Foreign</strong>: anything else. It waits for a reviewer to approve the{" "}
            <code>from-upstream</code> gate before it lands in company <code>main</code>.
          </li>
        </ul>
        <p>
          Company <code>main</code> is then rebuilt on the new upstream. A patch that no longer applies stops the
          rebuild and becomes a <strong>conflict</strong>. Its owner fixes it in a gated pull request, and the
          rebuild continues.
        </p>
        <p>
          Applying is half of it. The preflight script also runs on the rebuilt tree. The approved upstream passed
          it on its own, so when the rebuilt tree fails, <code>git bisect</code> between the two finds the first
          patch the script fails on. That patch becomes a conflict in the same way, and company{" "}
          <code>main</code> stays at the last build that passed.
        </p>
      </Section>

      <Section title="Recognizing a merged contribution">
        <p>
          Once a patch is marked merged it is never applied again. A patch counts as merged when either holds:
        </p>
        <ol>
          <li>
            An upstream commit has the same <code>git patch-id --stable</code> as the patch.
          </li>
          <li>The public pull request recorded on the queue was merged, also when the maintainer squashed or edited it.</li>
        </ol>
        <p>
          The exported commit carries an <code>Uplink-Patch-Id</code> trailer, but patch ids are public, so a
          trailer proves nothing by itself. A commit that names a patch it does not match is shown to the
          inbound reviewer as a claim.
        </p>
        <p>
          A rebuild also marks an upstream-bound patch that applies empty: the tree already has it. That is
          only a hint. If upstream merged your change and then changed the same lines, re-applying your
          original patch would bring back the old code. The patch id and the PR number make sure the patch is
          marked merged instead.
        </p>
      </Section>

      <Section title="Moving between queues">
        <p>
          A change can move from internal to upstream (it is ready to contribute) or back (it should never leave).
          Moving to upstream re-runs assess and preflight. If the change has to be edited to qualify, the move
          happens through a gated pull request. Moving a change that was already submitted back to internal also
          withdraws the public pull request after a separate approval.
        </p>
        <p>
          A merged change that turns out not to qualify for upstream (its required checks were bypassed, or
          something moved after them) takes the same path: it is recorded internal-only, and a transfer to
          upstream is opened for it.
        </p>
      </Section>

      <Section title="Credentials are split by role">
        <p>
          Three credentials, each able to do one thing: push company <code>main</code>, read upstream and open the
          public pull request, and push the contribution fork. The fork-write credential exists only inside jobs
          that passed the IP gate. Uplink never uses a developer&apos;s own keys, signing setup, or tokens. The
          preflight script, which builds and runs product code, runs only in jobs that hold a read-only token.
          The whole model, and what is left to the operator, is on{" "}
          <Link to="/security">Security</Link>.
        </p>
      </Section>

      <p className="text-sm text-muted-foreground">
        The mechanics, step by step:{" "}
        <Link to="/internals" className="text-primary underline-offset-4 hover:underline">
          Internals
        </Link>
        . Every flow is drawn there. Configuring a forge:{" "}
        <Link to="/setup" className="text-primary underline-offset-4 hover:underline">
          Production setup
        </Link>
        .
      </p>
    </DocPage>
  );
}
