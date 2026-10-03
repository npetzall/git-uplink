import { Link } from "react-router-dom";
import { DocPage, Section, Code } from "../components/doc-page";

function Lab({ scenario, children }: { scenario: string; children: React.ReactNode }) {
  return (
    <p className="text-sm">
      Try it: <Link to={`/lab?scenario=${scenario}`}>{children}</Link> in the lab.
    </p>
  );
}

const SCENARIOS = [
  ["new-change", "Make a new change"],
  ["depends-on", "Build on someone else's change"],
  ["internal-only", "Make a company-only change"],
  ["assess-fails", "Assess fails"],
  ["preflight-fails", "Preflight fails"],
  ["main-moved", "Main moved under my PR"],
  ["conflict", "My change conflicts with upstream"],
  ["amend", "Change a patch that is already queued or submitted"],
  ["transfer", "Move a change between internal and upstream"],
  ["where-is-it", "Where is my change?"],
];

export function DayToDayPage() {
  return (
    <DocPage
      eyebrow="Day to day"
      title="Branch, pull request, merge. That is the job."
      lead={
        <>
          Developers work the way they already do. Uplink runs in CI around them. This page answers the questions
          that come up.
        </>
      }
    >
      <Section title="The rules">
        <ul>
          <li>
            <strong>Nobody pushes company <code>main</code>.</strong> Developers create branches and merge pull
            requests, nothing more. The bot rewrites <code>main</code> when upstream moves.
          </li>
          <li>
            <strong>One branch per change.</strong> No second branch for upstream, and no public fork to touch.
          </li>
          <li>
            <strong>Write the PR as the upstream submission.</strong> The title and the body above the cutoff are
            the public commit message. Ticket ids, internal reviewers, and other company-only notes go below{" "}
            <code>----- Uplink: internal below this line -----</code>. The pull request template has it.
          </li>
          <li>
            <strong>Merging is the product gate, not the IP gate.</strong> Your change is in the company build as
            soon as you merge. Publishing it is a separate approval that you don&apos;t wait for.
          </li>
          <li>
            Many developers merge to <code>main</code> at the same time. Uplink serializes the bookkeeping, so no
            one needs to coordinate.
          </li>
        </ul>
        <nav aria-label="Scenarios" className="rounded-xl border border-border bg-card p-4">
          <p className="mb-2 text-xs font-medium tracking-wide text-foreground uppercase">Scenarios</p>
          <ul className="grid gap-1 sm:grid-cols-2">
            {SCENARIOS.map(([id, label]) => (
              <li key={id}>
                <a href={`#${id}`}>{label}</a>
              </li>
            ))}
          </ul>
        </nav>
      </Section>

      <Section id="new-change" title="Make a new change">
        <ol>
          <li>
            Branch from the latest company <code>main</code>. It already has everyone&apos;s queued work.
          </li>
          <li>Make the change, and open a pull request to <code>main</code>. Fill in the template.</li>
          <li>
            Wait for <strong>Uplink upstream assess</strong> and <strong>Uplink upstream preflight</strong> to go
            green, plus your normal CI.
          </li>
          <li>Get it reviewed and merge. That&apos;s it. The change is queued for upstream.</li>
        </ol>
        <Code>{`git fetch origin
git switch -c fix/token-hash origin/main`}</Code>
        <p>
          Contribution happens later: an operator dispatches <strong>Uplink submit</strong>, IP approves, and the
          public pull request opens. If maintainers ask for changes, make them in a new internal PR, as usual.
        </p>
        <Lab scenario="solo-fix">Solo fix</Lab>
      </Section>

      <Section id="depends-on" title="Build on someone else's change">
        <p>
          If your code needs another change that is merged internally but not yet upstream, it is already on{" "}
          <code>main</code>. Branch from <code>main</code> and use it. Then declare the dependency, one line per
          patch, in the PR body above the cutoff:
        </p>
        <Code>{`Uplink-Depends-On: upl_ab12cd34ef`}</Code>
        <ul>
          <li>
            Find the id with <code>git uplink status</code> or in the web UI.
          </li>
          <li>
            If the other change is still an open PR, wait for it to merge. Or stack on its branch, and rebase onto{" "}
            <code>main</code> before you merge. Never merge yours first.
          </li>
          <li>
            Only declare what your code really needs. Independent changes stay independent public PRs, even if
            they are queued before yours.
          </li>
          <li>
            Your change can be approved and submitted only after its dependencies have merged upstream. Until
            then the submit workflow stops in its first job. Each public PR stands alone on public{" "}
            <code>main</code>.
          </li>
        </ul>
        <Lab scenario="stacked-depends-on">Stacked depends-on</Lab>
      </Section>

      <Section id="internal-only" title="Make a company-only change">
        <p>
          Add the <code>uplink:internal-only</code> label to the pull request. Assess and preflight are skipped, the
          change is applied after all upstream-bound changes, and it is never exported. Keep these rare: the goal is
          that almost everything goes upstream.
        </p>
        <Lab scenario="internal-only">Internal-only</Lab>
      </Section>

      <Section id="assess-fails" title="My change works, but assess fails">
        <p>
          Assess reviews the change as it would appear in public and posts an <strong>Upstream Assessment</strong>{" "}
          comment on the PR, updated on every push or edit. The public title and body in it are exactly what will
          leave the company: HTML comments from the PR template are stripped, everything below the cutoff is
          removed, and the author is rewritten to the export identity. Below them, a table lists each check:
        </p>
        <ul>
          <li>
            <code>message-scrubbed</code>: the internal section below the cutoff is removed from the public message.
          </li>
          <li>
            <code>cutoff-used</code>: whether the cutoff line was found. Warns when a ticket id sits in the public
            part.
          </li>
          <li>
            <code>co-author</code>: the <code>Co-Authored-By</code> trailer upstream will see, and who wrote the
            change internally. Skipped when there is no <code>Uplink-Export-Author</code>.
          </li>
          <li>
            <code>affiliation-leak</code>: company keywords, product aliases, and internal email domains in the
            message and the diff.
          </li>
          <li>
            <code>binary-files</code>: binary files the scan cannot read.
          </li>
        </ul>
        <p>How to fix the common findings:</p>
        <ul>
          <li>
            <strong>Company name, product alias, or internal email found</strong> (fails). Remove it from the code,
            the tests, and the public part of the message. Test fixtures are the usual culprit. Internal notes
            belong below the cutoff.
          </li>
          <li>
            <strong>Ticket id above the cutoff</strong> (warning). Move it below the cutoff.
          </li>
          <li>
            <strong>Binary files</strong> (warning). The scan can&apos;t read them, so check them yourself.
          </li>
        </ul>
        <p>
          The public commit is authored by the contribution account (the contrib App or machine user). To credit
          a person as well, add <code>Uplink-Export-Author: Name &lt;email&gt;</code> below the cutoff; it becomes
          a <code>Co-Authored-By</code> trailer on the public commit. Use a public address such as your{" "}
          <code>users.noreply.github.com</code> email. Edit the PR and the check reruns. If the change is not meant
          for upstream, label it internal-only.
        </p>
      </Section>

      <Section id="preflight-fails" title="My change works, but preflight fails">
        <p>
          Preflight applies your change onto <strong>public upstream</strong> plus your declared dependencies, and
          runs the build and tests there. It works on your branch because your branch has everyone else&apos;s
          queued work. Upstream won&apos;t have it.
        </p>
        <ul>
          <li>
            <strong>It needs another queued upstream change.</strong> Add{" "}
            <code>Uplink-Depends-On: upl_…</code> for it. The failure comment suggests ids.
          </li>
          <li>
            <strong>It needs internal-only code.</strong> Upstream-bound changes may not depend on internal ones.
            Rewrite it so it stands alone, move the internal change to upstream first, or make yours internal-only
            too.
          </li>
          <li>
            <strong>It fails on its own.</strong> Upstream may have moved. Reproduce it locally:
          </li>
        </ul>
        <Code>{`git uplink preflight --from origin/main --head HEAD`}</Code>
        <Lab scenario="stacked-depends-on">Preflight without the trailer</Lab>
      </Section>

      <Section id="main-moved" title="Main moved under my PR">
        <p>
          Other merges, and syncs from upstream, move <code>main</code>. Sync may rewrite it. Rebase your branch
          onto the new <code>main</code>, as you would for any shared branch:
        </p>
        <Code>{`git fetch origin
git rebase origin/main`}</Code>
        <p>
          Only branch from company <code>main</code>. Never start work from <code>uplink/state</code>,{" "}
          <code>uplink/upstream</code>, the contribution fork, or a protected <code>uplink/…</code> base.
        </p>
      </Section>

      <Section id="conflict" title="My change conflicts with upstream">
        <p>
          When upstream changes the same lines as your patch, sync stops rebuilding <code>main</code> and opens a
          pull request labelled <code>uplink:conflict</code> from <code>uplink/conflict/&lt;id&gt;-work</code>.
          Company <code>main</code> is frozen until that PR merges, so it comes first.
        </p>
        <Code>{`git fetch origin
git switch uplink/conflict/<id>-work
# fix the files so the change is right on the new upstream
git commit -am "Resolve <id> onto the new upstream"
git push`}</Code>
        <p>
          The <strong>Uplink gate</strong> check assesses your resolution with the patch&apos;s message, like the
          original PR. If it fails (for example the fix mentions the company), change the resolution before merging.
          A company-only patch is not assessed; the check runs only the preflight script.
          Get the PR reviewed and merge it. The resolve job updates your patch (same id) and rebuilds{" "}
          <code>main</code>. If a later patch conflicts too, its owner gets the next PR. If your change was already
          submitted, the fix goes to IP as a small delta. After approval, the same public PR is updated.
        </p>
        <Lab scenario="upstream-conflict">Upstream conflict</Lab>
      </Section>

      <Section id="amend" title="Change a patch that is already queued or submitted">
        <p>
          Use this to fix a company-only patch, rework an upstream patch before it is submitted, or answer a
          maintainer who asked for changes on the public PR. Dispatch <strong>Uplink amend</strong> with the patch id.
          You get a draft PR labelled <code>uplink:amend</code> from <code>uplink/amend/&lt;id&gt;-work</code>. Its
          base already has the queue applied up to and including your patch.
        </p>
        <Code>{`git fetch origin
git switch uplink/amend/<id>-work
# make the change
git commit -am "Address review on <id>"
git push`}</Code>
        <p>
          The PR title and description are the patch title and commit message. Edit them if the message should
          change. The <strong>Uplink gate</strong> check assesses the whole amended patch with that message. A
          company-only patch is not assessed and needs no IP approval; the check runs only the preflight script. Mark
          the PR ready, get it reviewed, and merge it. The patch keeps its id and <code>main</code> is rebuilt. If it
          was already submitted, the change goes to IP as a small delta. After approval, the same public PR is
          updated. Close the PR without merging to cancel.
        </p>
      </Section>

      <Section id="transfer" title="Move a change between internal and upstream">
        <p>
          Dispatch <strong>Uplink transfer</strong> with the patch id and <code>to-upstream</code> or{" "}
          <code>to-internal</code>.
        </p>
        <ul>
          <li>
            If it qualifies as is, it moves immediately. Moving to upstream re-runs assess and preflight.
          </li>
          <li>
            If it needs changes, you get a transfer PR with a <code>-work</code> branch, like a conflict. Merge the PR
            to finish the move. Close it without merging to cancel.
          </li>
          <li>
            Moving to internal is refused while an upstream change still depends on it. Move those first.
          </li>
          <li>
            If the change was already submitted, moving it to internal also withdraws the public PR after the{" "}
            <code>abandon-contrib</code> approval.
          </li>
        </ul>
      </Section>

      <Section id="where-is-it" title="Where is my change?">
        <p>
          Look at the queue, not <code>git log main</code>:
        </p>
        <Code>{`git uplink status
git uplink web-ui`}</Code>
        <p>
          The web UI lists every patch, its status, and its dependencies, and explains what each status means and
          what happens next.
        </p>
      </Section>
    </DocPage>
  );
}
