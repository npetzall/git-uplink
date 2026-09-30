import { Link } from "react-router-dom";
import { DocPage, Section } from "../components/doc-page";

export function WhyPage() {
  return (
    <DocPage
      eyebrow="Why"
      title="Give back to the project you build on, and carry less while you do it"
      lead={
        <>
          Uplink exists for companies that build on open source from a private forge. It makes contributing
          upstream the default path for every change, so the set of private patches you carry stays small.
        </>
      }
    >
      <Section title="You use open source. Do your part.">
        <p>
          Your product stands on a public project that someone else maintains. Fixes and features you make
          are useful to everyone who uses that project. Contributing them keeps the project healthy, and it
          keeps it heading in a direction that works for you.
        </p>
        <p>
          A smaller maintenance burden is the bonus. Or the other way round, if that is what convinces your
          organization: every change that lands upstream is one less change you maintain alone.
        </p>
      </Section>

      <Section title="Carrying internal changes is expensive">
        <p>
          Upstream moves on without knowing about your changes. A refactor, a renamed function, or a fix in
          the same lines can break your patch at any time. Every private change must be re-applied,
          re-resolved, and re-tested on each upstream update, for as long as you keep it.
        </p>
        <ul>
          <li>
            A long-lived fork drifts. Merging upstream into it gets harder each time, and nobody can say which
            differences are deliberate.
          </li>
          <li>
            Uplink keeps the carried set <strong>explicit</strong>: a queue of patches, each with an id,
            rebuilt on top of the latest accepted upstream.
          </li>
          <li>
            When upstream takes a patch, Uplink <strong>drops</strong> it. You never re-apply an old version of
            a change on top of a newer upstream fix.
          </li>
        </ul>
      </Section>

      <Section title="But the work has to stay private until it is cleared">
        <p>
          Many companies cannot simply push to a public fork. Unreleased work must stay on the private forge,
          and every contribution needs an IP review before it becomes public. On GitHub Enterprise Cloud with
          Enterprise Managed Users, accounts can read public GitHub.com but cannot write to it at all — no
          forks, no pull requests, no comments. Other forges have the same shape through policy.
        </p>
        <p>
          So the contribution goes through a public <strong>contribution fork</strong> owned by the upstream
          side, pushed by a bot, and only after IP has approved it.
        </p>
      </Section>

      <Section title="Two gates, not one">
        <p>
          Shipping a change in your product and publishing it are different decisions, made by different
          people, on different timelines.
        </p>
        <ul>
          <li>
            <strong>Product gate</strong> — engineering review and merge. The change is in the company build
            right away.
          </li>
          <li>
            <strong>IP gate</strong> — legal or IP approval, later. Only then does the change leave the private
            forge.
          </li>
        </ul>
        <p>Legal review can take as long as it needs. The company keeps shipping in the meantime.</p>
      </Section>

      <Section title="One branch per change">
        <p>
          Developers do what they already know: branch, open a pull request, get it reviewed, merge. There is
          no second branch to keep in sync for upstream, and no public fork to maintain by hand. The upstream
          version of a change is generated from the same patch.
        </p>
      </Section>

      <Section title="Some changes are yours alone">
        <p>
          Not everything belongs upstream: company integrations, telemetry, branding. Those are marked{" "}
          <strong>internal-only</strong>. They are carried like any other patch, applied last, and never
          exported. A change bound for upstream may not depend on internal-only code, because it could not
          stand on its own in public.
        </p>
      </Section>

      <Section title="Why not an existing tool?">
        <ul>
          <li>
            <strong>GitHub Private Mirrors</strong> solves the airlock if you are willing to own a public fork.
            It does not rebuild your mainline from upstream plus unmerged work, and it does not drop patches
            after merge.
          </li>
          <li>
            <strong>Copybara</strong> is excellent at one-to-one repository transforms and weaker at an ordered
            queue that drops patches on merge.
          </li>
          <li>
            <strong>StGit</strong> has the queue semantics, but developers should not have to run it. Uplink
            takes that model and binds it to forge review, the contribution fork, and merge detection.
          </li>
        </ul>
      </Section>

      <p className="text-sm text-muted-foreground">
        Next:{" "}
        <Link to="/how" className="text-primary underline-offset-4 hover:underline">
          how it works
        </Link>
        , or what it means for developers{" "}
        <Link to="/day-to-day" className="text-primary underline-offset-4 hover:underline">
          day to day
        </Link>
        .
      </p>
    </DocPage>
  );
}
