import { Link } from "react-router-dom";
import { Command, Step, V } from "../../components/setup/step";
import type { Derive, Field } from "../../components/setup/values";

export const EXAMPLE_FIELDS: Field[] = [
  { key: "org", label: "Your new organization", token: "ORG", initial: "" },
  { key: "work", label: "Directory for the three clones", token: "WORK", initial: "~/src" },
  { key: "checkout", label: "Your git-uplink clone", token: "GIT_UPLINK", initial: "~/src/git-uplink", help: "For the example kit in examples/github" },
];

export const exampleDerive: Derive = (get) => ({ kit: `${get("checkout")}/examples/github` });

const UPSTREAM = "uplink-example-upstream";
const CONTRIB = "uplink-example-upstream-contrib";
const INTERNAL = "uplink-example-internal";
const repoUrl = (repo: string) => `https://github.com/$ORG/${repo}.git`;

const LABELS = [
  ["uplink:internal-only", "5319E7", "Never approve or submit this change upstream"],
  ["uplink:conflict", "B60205", "Uplink sync conflict; resolve via the gated work PR"],
  ["uplink:transfer-to-upstream", "1D76DB", "Uplink gated transfer to the upstream queue"],
  ["uplink:transfer-to-internal", "1D76DB", "Uplink gated transfer to the internal queue"],
  ["uplink:amend", "0E8A16", "Uplink gated amend of a patch"],
  ["uplink:rebase", "FBCA04", "Ask Uplink rebase to rebase this branch onto main"],
];

const VARIABLES = [
  ["UPLINK_INTERNAL_AUTH", "pat", "Token model for the internal role"],
  ["UPLINK_UPSTREAM_AUTH", "pat", "Token model for the upstream role"],
  ["UPLINK_CONTRIB_AUTH", "pat", "Token model for the contrib role"],
];

const TOKENS = [
  [
    "internal",
    INTERNAL,
    "Contents: read and write · Workflows: read and write · Pull requests: read and write",
    "Import, sync, resolve, transfer, and amend push company main and the gated branches (including workflow files) and open the gated PRs",
  ],
  [
    "upstream",
    `${UPSTREAM}, ${CONTRIB}`,
    "Contents: read · Pull requests: read and write",
    "Fetch public upstream; open (submit) and close (withdraw) the public PR",
  ],
  ["contrib", CONTRIB, "Contents: read and write", "Push and delete uplink/<id> on the fork"],
];

const ENVIRONMENTS = [
  ["to-upstream", "The IP gate before a contribution leaves. In production: IP / legal."],
  ["from-upstream", "Review of foreign upstream commits before they land on main."],
  ["abandon-contrib", "Withdrawing a submitted contribution after it moved to internal."],
];

const RULESET = `{
  "name": "Uplink: required checks",
  "target": "branch",
  "enforcement": "active",
  "bypass_actors": [
    { "actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always" },
    { "actor_id": 15368, "actor_type": "Integration", "bypass_mode": "always" }
  ],
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "rules": [
    { "type": "required_status_checks", "parameters": {
        "strict_required_status_checks_policy": false,
        "required_status_checks": [
          { "context": "Uplink upstream assess" },
          { "context": "Uplink upstream preflight" } ] } }
  ]
}`;

/** Publish orphan branch example-reset with the Reset example workflow and this repo's reset script. */
function resetBranch(script: string, replace: boolean) {
  return `git switch --orphan example-reset
mkdir -p .github/workflows scripts
cp "$KIT/example-reset.yml" .github/workflows/
cp "$KIT/reset/${script}.sh" scripts/reset-example.sh
git add .github/workflows scripts && git commit -m "Reset example"
git push ${replace ? "--force " : ""}origin example-reset
git switch main`;
}

function ResetBranch({ script, replace = false }: { script: string; replace?: boolean }) {
  return (
    <>
      <p>
        Publish the reset branch. It holds the <strong>Reset example</strong> workflow and this repository&apos;s reset
        script, which every story runs first:
      </p>
      <Command>{resetBranch(script, replace)}</Command>
      {replace ? (
        <p>
          <code>--force</code> because a fork that copied all branches already has upstream&apos;s{" "}
          <code>example-reset</code>; this replaces it with the contrib reset script.
        </p>
      ) : null}
    </>
  );
}

export function ExampleSteps() {
  return (
    <div className="space-y-12">
      <Step
        n={0}
        id="before"
        title="Before you start"
        manual={<Before />}
        gh={
          <>
            <Before />
            <p>
              Log in with the <code>workflow</code> scope, because you will push <code>.github/workflows</code>, and let
              git use that login:
            </p>
            <Command>{`gh auth login --scopes workflow
gh auth setup-git`}</Command>
          </>
        }
      />

      <Step
        n={1}
        id="org"
        title="Create an organization"
        manual={
          <>
            <p>
              On github.com, <strong>Your organizations → New organization</strong> (the free plan is enough). A fresh
              organization keeps the example isolated and lets one fine-grained token cover several repositories.
            </p>
            <p>
              In <strong>Organization settings → Personal access tokens → Settings</strong>, allow fine-grained personal
              access tokens. If you require approval, approve your own tokens under <strong>Pending requests</strong>{" "}
              after step 8.
            </p>
          </>
        }
      />

      <Step
        n={2}
        id="upstream"
        title="Upstream: the public project"
        manual={
          <>
            <p>
              Create a <strong>public</strong>, <strong>empty</strong> repository <code>{UPSTREAM}</code> in{" "}
              <V>{"{{org}}"}</V> (no README, license, or .gitignore).
            </p>
            <UpstreamBody />
          </>
        }
        gh={
          <>
            <Command>{`gh repo create "$ORG/${UPSTREAM}" --public`}</Command>
            <UpstreamBody />
          </>
        }
      />

      <Step
        n={3}
        id="contrib"
        title="Contribution fork"
        manual={
          <>
            <p>
              On <code>{UPSTREAM}</code>, choose <strong>Fork</strong>. Owner: <V>{"{{org}}"}</V>, name:{" "}
              <code>{CONTRIB}</code>. A fork in the same organization lets fine-grained tokens open the public PR.
            </p>
            <ContribBody />
          </>
        }
        gh={
          <>
            <Command>{`gh repo fork "$ORG/${UPSTREAM}" --org "$ORG" --fork-name ${CONTRIB} --default-branch-only --clone=false`}</Command>
            <ContribBody />
          </>
        }
      />

      <Step
        n={4}
        id="internal"
        title="Internal: the company product"
        manual={
          <>
            <p>
              Create a <strong>private</strong>, <strong>empty</strong> repository <code>{INTERNAL}</code> in{" "}
              <V>{"{{org}}"}</V>.
            </p>
            <InternalBody />
          </>
        }
        gh={
          <>
            <Command>{`gh repo create "$ORG/${INTERNAL}" --private`}</Command>
            <InternalBody />
          </>
        }
      />

      <Step
        n={5}
        id="labels"
        title="Labels"
        manual={
          <>
            <p>
              In <code>{INTERNAL}</code>, <strong>Issues → Labels → New label</strong>:
            </p>
            <ul>
              {LABELS.map(([name, , description]) => (
                <li key={name}>
                  <code>{name}</code>: {description}
                </li>
              ))}
            </ul>
          </>
        }
        gh={
          <Command>
            {LABELS.map(
              ([name, color, description]) =>
                `gh label create ${name} --repo "$ORG/${INTERNAL}" --color ${color} --description "${description}"`,
            ).join("\n")}
          </Command>
        }
      />

      <Step
        n={6}
        id="variables"
        title="Repository variables"
        manual={
          <>
            <p>
              In <code>{INTERNAL}</code>, <strong>Settings → Secrets and variables → Actions → Variables</strong>:
            </p>
            <VariableTable />
          </>
        }
        gh={
          <>
            <VariableTable />
            <Command>
              {VARIABLES.map(([name, value]) => `gh variable set ${name} --repo "$ORG/${INTERNAL}" --body '${value}'`).join(
                "\n",
              )}
            </Command>
          </>
        }
      />

      <Step
        n={7}
        id="actions"
        title="Actions permissions"
        manual={
          <>
            <p>
              In <code>{INTERNAL}</code>, <strong>Settings → Actions → General</strong>: allow all actions, keep{" "}
              <strong>Workflow permissions</strong> at read (every job declares what it needs), and leave{" "}
              <strong>Allow GitHub Actions to create and approve pull requests</strong> unchecked. The internal token opens
              the gated PRs.
            </p>
          </>
        }
        gh={
          <Command>{`gh api -X PUT "repos/$ORG/${INTERNAL}/actions/permissions" -F enabled=true -f allowed_actions=all
gh api -X PUT "repos/$ORG/${INTERNAL}/actions/permissions/workflow" \\
  -f default_workflow_permissions=read -F can_approve_pull_request_reviews=false`}</Command>
        }
      />

      <Step
        n={8}
        id="tokens"
        title="Three fine-grained tokens"
        manual={
          <>
            <p>
              <strong>Settings → Developer settings → Personal access tokens → Fine-grained tokens → Generate new token</strong>
              , three times. <strong>Resource owner</strong>: <V>{"{{org}}"}</V>. <strong>Repository access</strong>: only
              the listed repositories. Metadata read is added automatically. Keep each token for the next steps.
            </p>
            <div className="overflow-x-auto">
              <table className="min-w-[640px]">
                <thead>
                  <tr>
                    <th>Token</th>
                    <th>Repositories</th>
                    <th>Permissions</th>
                    <th>Used for</th>
                  </tr>
                </thead>
                <tbody>
                  {TOKENS.map(([name, repos, perms, why]) => (
                    <tr key={name}>
                      <td className="text-foreground">{name}</td>
                      <td className="font-mono text-xs">{repos}</td>
                      <td>{perms}</td>
                      <td>{why}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </>
        }
      />

      <Step
        n={9}
        id="environments"
        title="The three gates"
        manual={
          <>
            <p>
              In <code>{INTERNAL}</code>, <strong>Settings → Environments</strong>, for each environment below: add
              yourself under <strong>Required reviewers</strong>, leave <strong>Prevent self-review</strong> off (you are
              the only reviewer), and restrict <strong>Deployment branches and tags</strong> to <code>main</code>.
            </p>
            <EnvironmentList />
          </>
        }
        gh={
          <>
            <EnvironmentList />
            <Command>{`ME=$(gh api user --jq .id)
for env in to-upstream from-upstream abandon-contrib; do
  gh api -X PUT "repos/$ORG/${INTERNAL}/environments/$env" --input - <<EOF
{ "prevent_self_review": false,
  "reviewers": [ { "type": "User", "id": $ME } ],
  "deployment_branch_policy": { "protected_branches": false, "custom_branch_policies": true } }
EOF
  gh api -X POST "repos/$ORG/${INTERNAL}/environments/$env/deployment-branch-policies" -f name=main -f type=branch
done`}</Command>
          </>
        }
      />

      <Step
        n={10}
        id="secrets"
        title="Store the tokens"
        manual={
          <>
            <p>In <code>{INTERNAL}</code>:</p>
            <SecretList />
            <p>
              Repository secrets: <strong>Settings → Secrets and variables → Actions → New repository secret</strong>.
              Environment secrets: <strong>Settings → Environments → (name) → Add environment secret</strong>.
            </p>
          </>
        }
        gh={
          <>
            <SecretList />
            <p>Each command prompts for the token, so it stays out of your shell history:</p>
            <Command>{`gh secret set UPLINK_INTERNAL_TOKEN --repo "$ORG/${INTERNAL}"
gh secret set UPLINK_UPSTREAM_TOKEN --repo "$ORG/${INTERNAL}"
gh secret set UPLINK_CONTRIB_TOKEN --repo "$ORG/${INTERNAL}" --env to-upstream
gh secret set UPLINK_CONTRIB_TOKEN --repo "$ORG/${INTERNAL}" --env abandon-contrib`}</Command>
          </>
        }
      />

      <Step
        n={11}
        id="checks"
        title="Required checks on main"
        manual={
          <>
            <p>
              In <code>{INTERNAL}</code>, <strong>Settings → Rules → Rulesets → New branch ruleset</strong>: target the
              default branch, enforcement <strong>Active</strong>, rule <strong>Require status checks to pass</strong>{" "}
              with <code>Uplink upstream assess</code> and <code>Uplink upstream preflight</code>.
            </p>
            <RulesetBypass />
          </>
        }
        gh={
          <>
            <RulesetBypass />
            <Command>{`gh api -X POST "repos/$ORG/${INTERNAL}/rulesets" --input - <<'EOF'
${RULESET}
EOF`}</Command>
          </>
        }
      />

      <Step
        n={12}
        id="start"
        title="Reset and start the first story"
        manual={
          <>
            <p>
              On each of the three repositories, <strong>Actions → Reset example → Run workflow</strong>. Then in your
              internal clone:
            </p>
            <StartBody />
          </>
        }
        gh={
          <>
            <Command>{`for repo in ${UPSTREAM} ${CONTRIB} ${INTERNAL}; do
  gh workflow run "Reset example" --repo "$ORG/$repo"
done`}</Command>
            <p>When the three runs are green, in your internal clone:</p>
            <StartBody />
          </>
        }
      />
    </div>
  );
}

function Before() {
  return (
    <>
      <p>
        You need <code>git</code>, <Link to="/install">git-uplink</Link> on your <code>PATH</code>, and for the gh CLI
        path <a href="https://cli.github.com/">gh</a>. Commands are for bash or zsh. On Windows, use WSL or Git Bash.
      </p>
      <p>
        Clone git-uplink for the example kit, then set the three variables every command below uses. They are the same
        ones the stories use:
      </p>
      <Command>{`git clone https://github.com/npetzall/git-uplink.git {{checkout}}
export ORG={{org}}
export WORK={{work}}
export KIT={{kit}}`}</Command>
    </>
  );
}

function UpstreamBody() {
  return (
    <>
      <p>
        Clone it, make the first commit from the kit (the <code>tokenkit</code> project plus the Reset example workflow),
        and keep a <code>seed</code> branch to reset to:
      </p>
      <Command>{`git clone ${repoUrl(UPSTREAM)} "$WORK/${UPSTREAM}"
cd "$WORK/${UPSTREAM}"
cp -R "$KIT/upstream/." .
mkdir -p .github/workflows && cp "$KIT/example-reset.yml" .github/workflows/
git add -A && git commit -m "initial tokens"
git branch -M main && git push -u origin main
git branch seed && git push origin seed`}</Command>
      <ResetBranch script="upstream" />
    </>
  );
}

function ContribBody() {
  return (
    <>
      <p>
        Clone it. Leave its <code>main</code> alone: it must stay a fork of upstream <code>main</code>.
      </p>
      <Command>{`git clone ${repoUrl(CONTRIB)} "$WORK/${CONTRIB}"
cd "$WORK/${CONTRIB}"`}</Command>
      <ResetBranch script="contrib" replace />
    </>
  );
}

function InternalBody() {
  return (
    <>
      <p>
        Clone it, start <code>main</code> from upstream, and let <code>git uplink init</code> install the example pack as
        the tooling patch. Then push, and keep seed refs for resets:
      </p>
      <Command>{`git clone ${repoUrl(INTERNAL)} "$WORK/${INTERNAL}"
cd "$WORK/${INTERNAL}"
git remote add upstream ${repoUrl(UPSTREAM)}
git remote add contrib ${repoUrl(CONTRIB)}
git fetch upstream
git checkout -B main upstream/main
git uplink init --upstream ${repoUrl(UPSTREAM)} --contrib ${repoUrl(CONTRIB)} --forge try-it-on-github \\
  --preflight 'npm test' --redact-keyword companyTelemetry,AcmeCorp --internal-domain acme.example
git uplink status
git push -u origin main
git push origin uplink/state uplink/upstream uplink/hooks
git branch seed main && git branch seed-state uplink/state && git branch seed-upstream uplink/upstream
git push origin seed seed-state seed-upstream`}</Command>
      <p>
        <code>git uplink status</code> shows <strong>Uplink tooling</strong> in the tooling slot. The three{" "}
        <code>init</code> flags are the stories&apos; settings; <code>init</code> writes them to{" "}
        <code>preflight.sh</code> and <code>uplink.toml</code> on <code>uplink/hooks</code>.
      </p>
      <ResetBranch script="internal" />
    </>
  );
}

function VariableTable() {
  return (
    <div className="overflow-x-auto">
      <table className="min-w-[560px]">
        <thead>
          <tr>
            <th>Variable</th>
            <th>Value</th>
            <th>Purpose</th>
          </tr>
        </thead>
        <tbody>
          {VARIABLES.map(([name, value, purpose]) => (
            <tr key={name}>
              <td className="font-mono text-xs text-foreground">{name}</td>
              <td>
                <V>{value}</V>
              </td>
              <td>{purpose}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p>
        Jobs install the <code>git-uplink</code> release that wrote the pack, from <code>npetzall/git-uplink</code>. Set{" "}
        <code>UPLINK_SRC</code> or <code>UPLINK_VERSION</code> only to install from a mirror or to pin another release.
        Every job verifies the binary&apos;s cosign signature against the release workflow of{" "}
        <code>npetzall/git-uplink</code>, so a mirror must carry the <code>.sigstore.json</code> files. Set{" "}
        <code>UPLINK_SIGNER</code> only for a mirror that builds and signs its own releases.
      </p>
    </div>
  );
}

function EnvironmentList() {
  return (
    <ul>
      {ENVIRONMENTS.map(([name, what]) => (
        <li key={name}>
          <code>{name}</code>: {what}
        </li>
      ))}
    </ul>
  );
}

function SecretList() {
  return (
    <ul>
      <li>
        <code>UPLINK_INTERNAL_TOKEN</code> (internal token) and <code>UPLINK_UPSTREAM_TOKEN</code> (upstream token) as{" "}
        <strong>repository</strong> secrets. Sync cannot wait on an environment.
      </li>
      <li>
        <code>UPLINK_CONTRIB_TOKEN</code> (contrib token) on the <code>to-upstream</code> <strong>and</strong>{" "}
        <code>abandon-contrib</code> environments. The fork write credential only exists behind a review. Environments
        don&apos;t share secrets, so store it twice, and never at repository level. GitHub creates the contribution
        commit with this token, so the machine user is its author. Commits made with a PAT are not shown as Verified;
        use a GitHub App for that.
      </li>
    </ul>
  );
}

function RulesetBypass() {
  return (
    <p>
      Bypass list: <strong>Repository admin</strong> (the internal token is yours, and you are an admin) and{" "}
      <strong>GitHub Actions</strong> (the Reset example workflow force-pushes <code>main</code> back to{" "}
      <code>seed</code>). Everyone else merges through PRs with both checks green.
    </p>
  );
}

function StartBody() {
  return (
    <>
      <Command>{`cd "$WORK/${INTERNAL}"
git uplink reset
git fetch origin --prune
git uplink status`}</Command>
      <p>
        Setup is done. Open the <strong>Stories</strong> tab and start with <em>01 — Solo fix</em>.
      </p>
    </>
  );
}
