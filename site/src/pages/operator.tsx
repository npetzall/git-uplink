import { Link } from "react-router-dom";
import { Code, DocPage, Section } from "../components/doc-page";

const TASKS = [
  ["setup", "Set up a repository"],
  ["upgrade", "Upgrade the pack and the binary"],
  ["adopt", "Adopt existing history"],
  ["health", "See the queue and check the setup"],
  ["in-step", "Keep a clone in step with origin"],
  ["dispatch", "Workflows you dispatch"],
  ["hooks", "Hooks and settings"],
  ["fallbacks", "Fix by hand when the forge cannot"],
];

export function OperatorPage() {
  return (
    <DocPage
      eyebrow="Operator"
      title="Set it up, keep it healthy, repair it by hand."
      lead={
        <>
          The operator is the one person or team that runs{" "}
          <code>git uplink</code> by hand. Day-to-day work is the{" "}
          <Link to="/forge">forge</Link>&apos;s, and developers only use pull
          requests (see <Link to="/developer">Developer</Link>).
        </>
      }
    >
      <Section title="What is yours, and what is not">
        <ul>
          <li>
            <strong>Yours:</strong> the repositories, Apps, Environments and
            rulesets, <code>init</code> and <code>init --upgrade</code>,
            adopting history, <code>uplink/hooks</code>, troubleshooting, and
            repairs the forge cannot make.
          </li>
          <li>
            <strong>The forge&apos;s:</strong> importing merged pull requests,
            syncing upstream, submitting, rebasing pull requests, and finishing
            conflicts, amends and transfers. Do not run those commands to speed
            things up; dispatch the workflow instead, so every change goes
            through its gate.
          </li>
          <li>
            <strong>The developer&apos;s:</strong> the change itself. A
            developer who wants a patch amended or moved dispatches{" "}
            <strong>Uplink amend</strong> or <strong>Uplink transfer</strong>{" "}
            and works in the pull request the forge opens. Don&apos;t rewrite a
            patch for them.
          </li>
          <li id="forge-failed" className="scroll-mt-20">
            <strong>When a forge workflow fails,</strong> developers come to you
            with the link to the run. Read its log and summary, check the setup
            with <code>git uplink doctor</code>, and fix the cause (credentials,
            a ruleset, <code>uplink/hooks</code>). Then run the workflow again.
            Repair by hand only when the forge cannot do it (
            <a href="#fallbacks">below</a>).
          </li>
        </ul>
        <nav
          aria-label="Tasks"
          className="rounded-xl border border-border bg-card p-4"
        >
          <p className="mb-2 text-xs font-medium tracking-wide text-foreground uppercase">
            Tasks
          </p>
          <ul className="grid gap-1 sm:grid-cols-2">
            {TASKS.map(([id, label]) => (
              <li key={id}>
                <a href={`#${id}`}>{label}</a>
              </li>
            ))}
          </ul>
        </nav>
      </Section>

      <Section id="setup" title="Set up a repository">
        <p>
          <Link to="/setup">Production setup</Link> walks through it: the
          contribution fork, the three Apps, variables and secrets, the
          Environments, labels and rulesets. It ends with <code>init</code>,
          which writes the queue on <code>uplink/state</code>, installs the
          forge pack as the tooling patch, and creates <code>uplink/hooks</code>
          , and with the first push:
        </p>
        <Code>{`git uplink init --upstream <url> --contrib <url> --forge github
git uplink push
git uplink rebuild --push`}</Code>
        <p>
          Run <code>git uplink init</code> with no arguments in any other clone
          to restore it from origin. Credentials for each remote are in{" "}
          <Link to="/security#local">Security</Link>: give each command the
          smallest token it needs.
        </p>
      </Section>

      <Section id="upgrade" title="Upgrade the pack and the binary">
        <p>
          The pack and the binary are one version. Install the new release with{" "}
          <code>git uplink self-update</code> (see{" "}
          <Link to="/install">Install</Link>), then, in a clone of the company
          repository:
        </p>
        <Code>{`git uplink init --upgrade`}</Code>
        <p>
          The upgrade builds <code>uplink/preview/upgrade</code> instead of
          rebuilding <code>main</code>, lists any product file it would change
          against origin's <code>main</code> (it never should), and asks{" "}
          <em>Update remote?</em> Yes rebuilds <code>main</code> and pushes{" "}
          <code>uplink/state</code>, <code>uplink/hooks</code> and{" "}
          <code>main</code>, each with a lease. No pushes nothing and keeps the
          preview. When the clone has local work, it offers a reset to origin
          first, which deletes that work and untracked files. When origin's{" "}
          <code>main</code> has commits <code>uplink/state</code> does not
          record, it asks to adopt them, overwrite them or abort. Without a
          terminal, <code>--allow-reset</code> and{" "}
          <code>--push-if-unchanged</code> answer for it.
        </p>
        <p>
          It replays patches that touch pack files, and stops the way{" "}
          <code>git rebase</code> does when one no longer applies:{" "}
          <code>--continue</code>, <code>--drop</code> or <code>--abort</code>.
          It also adds settings that <code>uplink.toml</code> on{" "}
          <code>uplink/hooks</code> lacks. Details are in the{" "}
          <Link to="/setup?forge=github&amp;view=workflows">
            workflow reference
          </Link>
          .
        </p>
      </Section>

      <Section id="adopt" title="Adopt existing history">
        <p>
          When company <code>main</code> is already ahead of upstream,{" "}
          <code>init</code> adopts its commits as patches. Commits on another
          branch are adopted after init. Both group the commits in a terminal UI
          (or from a groups file), check each group against upstream and{" "}
          <code>preflight.sh</code>, and build{" "}
          <code>uplink/preview/verify</code> to compare:
        </p>
        <Code>{`git fetch origin main-fork
git uplink adopt --branch main-fork
git diff main uplink/preview/verify
git uplink push
git uplink rebuild --push`}</Code>
        <p>
          Adoption is local and locks <code>uplink/state</code> until it ends;{" "}
          <code>git uplink adopt --continue</code> takes up a paused run. If
          origin&apos;s <code>uplink/state</code> moved meanwhile,{" "}
          <code>git uplink refresh</code> then <code>git uplink restack</code>{" "}
          adds your patches again on top of it before the push.
        </p>
      </Section>

      <Section id="health" title="See the queue and check the setup">
        <Code>{`git uplink status
git uplink doctor
git uplink web-ui`}</Code>
        <ul>
          <li>
            <code>status</code> prints the queue. <code>doctor</code> checks the
            queue, the remotes, the pack on <code>main</code>,{" "}
            <code>uplink/hooks</code>, the <code>UPLINK_*</code> credentials,
            and a pending adoption.
          </li>
          <li>
            <code>web-ui</code> is the operator dashboard, on{" "}
            <code>127.0.0.1</code>. It lists every patch, its status and its
            dependencies, and what happens next. <strong>Local</strong> is{" "}
            <code>uplink/state</code> in your clone, <strong>Remote</strong> is{" "}
            <code>origin/uplink/state</code> as last fetched.{" "}
            <strong>Ready to submit</strong> lists the queued upstream changes
            submit can take now, and <strong>Submit stream</strong> draws the
            order to submit and merge them.
          </li>
        </ul>
      </Section>

      <Section id="in-step" title="Keep a clone in step with origin">
        <ul>
          <li>
            <code>git uplink refresh</code> fetches origin&apos;s Uplink
            branches without moving local ones.
          </li>
          <li>
            <code>git uplink reset</code> makes the clone match origin. Use it
            to throw away local work.
          </li>
          <li>
            <code>git uplink restack</code> adds local patches again on top of
            origin&apos;s <code>uplink/state</code> when both moved; then{" "}
            <code>git uplink push</code>.
          </li>
        </ul>
      </Section>

      <Section id="dispatch" title="Workflows you dispatch">
        <ul>
          <li>
            <strong>Uplink sync</strong>, when you want upstream now rather than
            at the next hourly, daily or weekly run.
          </li>
          <li>
            <strong>Uplink verify</strong>, when <code>main</code> turned out
            broken and no rebuild noticed.
          </li>
          <li>
            <strong>Uplink submit</strong>, after you marked a patch merged or
            rebuilt by hand: only workflows dispatch it on their own.
          </li>
        </ul>
        <p>
          Amend, transfer and submit for a developer&apos;s own change are the
          developer&apos;s to dispatch. Each workflow is described on the{" "}
          <Link to="/forge">Forge</Link> page.
        </p>
      </Section>

      <Section id="hooks" title="Hooks and settings">
        <p>
          <code>uplink/hooks</code> holds <code>preflight.sh</code>, the
          optional toolchain and assessment hooks, and <code>uplink.toml</code>{" "}
          with the banned words. Change it through a pull request against{" "}
          <code>uplink/hooks</code>, and try a new <code>preflight.sh</code>{" "}
          before it lands:
        </p>
        <Code>{`git uplink preflight --command-only --hooks <branch>`}</Code>
        <p>
          See <Link to="/setup?forge=github&amp;view=hooks">Hooks</Link>.
        </p>
      </Section>

      <Section id="fallbacks" title="Fix by hand when the forge cannot">
        <p>
          These change the queue without a gated pull request, so keep them for
          when the forge is stuck.
        </p>
        <ul>
          <li>
            <strong>A conflict:</strong> fix it on its branches, or inline the
            way <code>git rebase</code> stops. <code>rebuild -i</code> writes
            nothing to <code>uplink/state</code> until every stop is fixed.
          </li>
        </ul>
        <Code>{`git switch uplink/conflict/<id>-work
# fix the files
git add <files>
git uplink resolve <id>
git uplink push
git uplink rebuild --push

# or inline
git uplink rebuild -i --push
git add <files>
git uplink rebuild --continue   # --abort changes nothing`}</Code>
        <ul>
          <li>
            <strong>A patch that should leave the queue:</strong>{" "}
            <code>git uplink drop &lt;id&gt; --reason &lt;text&gt;</code>.
          </li>
          <li>
            <strong>Upstream took a patch in another form:</strong>{" "}
            <code>git uplink merged &lt;id&gt; --sha &lt;sha&gt;</code>. The
            from-upstream review packet names these.
          </li>
          <li>
            <strong>Old depends-on lists that name whole chains:</strong>{" "}
            <code>git uplink trim-deps</code> shortens them once.
          </li>
          <li>
            <strong>
              <code>main</code> has what <code>uplink/state</code> lacks:
            </strong>{" "}
            <code>rebuild --push</code> then pushes <code>uplink/state</code>{" "}
            and refuses <code>main</code>. Record the change first (a failed
            import is on an <code>uplink/rescue/</code> branch), or add{" "}
            <code>--force</code> to replace <code>main</code> anyway. The push
            is leased either way, so a merge that lands meanwhile is kept.
          </li>
        </ul>
        <p>
          Then <code>git uplink push</code> (and{" "}
          <code>git uplink rebuild --push</code> where <code>main</code>{" "}
          changed).
        </p>
      </Section>
    </DocPage>
  );
}
