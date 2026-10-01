import { Link } from "react-router-dom";
import { Command, Step, V } from "../../components/setup/step";
import type { Derive, Field } from "../../components/setup/values";
import { GITHUB_BLOB } from "../../lib/links";

export const GHEC_FIELDS: Field[] = [
  { key: "host", label: "Company GitHub host", token: "COMPANY_HOST", initial: "github.com", help: "github.com, or your GHE.com subdomain" },
  { key: "companyOrg", label: "Company organization", token: "COMPANY_ORG", initial: "" },
  { key: "productRepo", label: "Product repository", token: "PRODUCT_REPO", initial: "" },
  { key: "upstreamUrl", label: "Public upstream URL", token: "UPSTREAM_URL", initial: "", help: "https://github.com/owner/project.git" },
  { key: "contribOrg", label: "Contribution fork organization", token: "CONTRIB_ORG", initial: "", help: "On public github.com; same org as upstream to use Apps" },
  { key: "contribRepo", label: "Contribution fork name", token: "CONTRIB_REPO", initial: "" },
  { key: "ipTeam", label: "IP reviewer team (slug)", token: "IP_TEAM", initial: "" },
  { key: "inboundTeam", label: "Inbound reviewer team (slug)", token: "INBOUND_TEAM", initial: "" },
  { key: "abandonTeam", label: "Withdraw reviewer team (slug)", token: "ABANDON_TEAM", initial: "" },
  { key: "internalAppId", label: "Internal App ID", token: "INTERNAL_APP_ID", initial: "", help: "From step 2" },
  { key: "upstreamAppId", label: "Upstream App ID", token: "UPSTREAM_APP_ID", initial: "", help: "From step 2" },
  { key: "contribAppId", label: "Contrib App ID", token: "CONTRIB_APP_ID", initial: "", help: "From step 2" },
  { key: "uplinkVersion", label: "git-uplink release", token: "UPLINK_VERSION", initial: "latest" },
];

function upstreamSlug(url: string): string {
  const m = url.match(/github\.com[:/]([^/]+)\/([^/]+?)(\.git)?\/?$/);
  return m ? `${m[1]}/${m[2]}` : "";
}

export const ghecDerive: Derive = (get, values) => ({
  companyRepo: `${get("companyOrg")}/${get("productRepo")}`,
  ghRepo:
    (values.host && values.host !== "github.com" ? `${values.host}/` : "") + `${get("companyOrg")}/${get("productRepo")}`,
  companyUrl: `https://${get("host")}/${get("companyOrg")}/${get("productRepo")}.git`,
  contribUrl: `https://github.com/${get("contribOrg")}/${get("contribRepo")}.git`,
  upstreamSlug: upstreamSlug(values.upstreamUrl ?? "") || "<UPSTREAM_OWNER>/<UPSTREAM_REPO>",
});

const LABELS = [
  ["uplink:internal-only", "5319E7", "Never approve or submit this change upstream"],
  ["uplink:conflict", "B60205", "Uplink sync conflict; resolve via the gated work PR"],
  ["uplink:transfer-to-upstream", "1D76DB", "Uplink gated transfer to the upstream queue"],
  ["uplink:transfer-to-internal", "1D76DB", "Uplink gated transfer to the internal queue"],
];

const VARIABLES = [
  ["UPLINK_SRC", "npetzall/git-uplink", "Repository that publishes git-uplink releases"],
  ["UPLINK_VERSION", "{{uplinkVersion}}", "Release to install: latest, or a tag"],
  ["UPLINK_PREFLIGHT", "npm test", "Build and test command run on the export tree"],
  ["UPLINK_REDACT_KEYWORDS", "<company>,<product alias>", "Words that must not appear in a contribution"],
  ["UPLINK_INTERNAL_DOMAINS", "<company.example>", "Email domains flagged in the export"],
  ["UPLINK_INTERNAL_AUTH", "app", "app or pat"],
  ["UPLINK_UPSTREAM_AUTH", "app", "app or pat"],
  ["UPLINK_CONTRIB_AUTH", "app", "app or pat. Only app gives Verified contribution commits"],
];

const APPS = [
  {
    name: "Internal App",
    where: "Your enterprise organization ({{host}}), owned by {{companyOrg}}",
    perms: "Contents: read and write · Workflows: read and write · Pull requests: read and write",
    install: "{{companyRepo}} only",
    why: "Import, sync, resolve, and transfer force-push company main and the gated branches, which contain workflow files, and open the gated pull requests.",
  },
  {
    name: "Upstream App",
    where: "Public github.com, owned by {{contribOrg}} (the org that owns upstream and the fork)",
    perms: "Contents: read · Pull requests: read and write",
    install: "{{upstreamSlug}} and {{contribOrg}}/{{contribRepo}}",
    why: "Fetches public upstream, opens the public PR on submit, closes it on withdraw. No write access to code.",
  },
  {
    name: "Contrib App",
    where: "Public github.com, owned by {{contribOrg}}",
    perms: "Contents: read and write",
    install: "{{contribOrg}}/{{contribRepo}} only",
    why: "Pushes and deletes uplink/<id> on the fork. Its key only exists in the to-upstream and abandon-contrib Environments.",
  },
];

const ENVIRONMENTS = [
  { name: "to-upstream", team: "ipTeam", teamVar: "IP_TEAM_ID", who: "IP / legal", secrets: true, what: "The IP gate before a contribution leaves." },
  { name: "from-upstream", team: "inboundTeam", teamVar: "INBOUND_TEAM_ID", who: "security / engineering", secrets: false, what: "Review of foreign upstream commits before they land on main." },
  { name: "abandon-contrib", team: "abandonTeam", teamVar: "ABANDON_TEAM_ID", who: "engineering", secrets: true, what: "Withdrawing a submitted contribution after it moved to internal." },
];

const MAIN_RULESET = `{
  "name": "Uplink: company main",
  "target": "branch",
  "enforcement": "disabled",
  "bypass_actors": [
    { "actor_id": {{internalAppId}}, "actor_type": "Integration", "bypass_mode": "always" }
  ],
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "rules": [
    { "type": "deletion" },
    { "type": "non_fast_forward" },
    { "type": "pull_request", "parameters": {
        "required_approving_review_count": 1,
        "dismiss_stale_reviews_on_push": true,
        "require_code_owner_review": false,
        "require_last_push_approval": false,
        "required_review_thread_resolution": false } },
    { "type": "required_status_checks", "parameters": {
        "strict_required_status_checks_policy": false,
        "required_status_checks": [
          { "context": "Uplink upstream assess" },
          { "context": "Uplink upstream preflight" } ] } }
  ]
}`;

const GATED_RULESET = `{
  "name": "Uplink: gated bases",
  "target": "branch",
  "enforcement": "disabled",
  "bypass_actors": [
    { "actor_id": {{internalAppId}}, "actor_type": "Integration", "bypass_mode": "always" }
  ],
  "conditions": { "ref_name": {
    "include": ["refs/heads/uplink/conflict/**", "refs/heads/uplink/transfer-to-upstream/**", "refs/heads/uplink/transfer-to-internal/**"],
    "exclude": ["refs/heads/uplink/conflict/*-work", "refs/heads/uplink/transfer-to-upstream/*-work", "refs/heads/uplink/transfer-to-internal/*-work"] } },
  "rules": [
    { "type": "creation" },
    { "type": "deletion" },
    { "type": "non_fast_forward" },
    { "type": "pull_request", "parameters": {
        "required_approving_review_count": 1,
        "dismiss_stale_reviews_on_push": true,
        "require_code_owner_review": false,
        "require_last_push_approval": false,
        "required_review_thread_resolution": false } },
    { "type": "required_status_checks", "parameters": {
        "strict_required_status_checks_policy": false,
        "required_status_checks": [ { "context": "Uplink gate" } ] } }
  ]
}`;

const PACK_RULESET = `{
  "name": "Uplink: pack files",
  "target": "branch",
  "enforcement": "disabled",
  "bypass_actors": [
    { "actor_id": {{internalAppId}}, "actor_type": "Integration", "bypass_mode": "always" }
  ],
  "conditions": { "ref_name": {
    "include": ["refs/heads/uplink/conflict/**", "refs/heads/uplink/transfer-to-upstream/**", "refs/heads/uplink/transfer-to-internal/**"],
    "exclude": [] } },
  "rules": [
    { "type": "file_path_restriction", "parameters": {
        "restricted_file_paths": [".github/workflows/uplink-*.yml", ".github/actions/install-git-uplink/**", ".github/uplink/**"] } }
  ]
}`;

const API = "gh api --hostname {{host}}";

export function GhecSteps() {
  return (
    <div className="space-y-12">
      <Step
        n={0}
        id="before"
        title="Before you start"
        manual={
          <>
            <p>You need three people, or one person with all three hats:</p>
            <ul>
              <li>
                An <strong>admin of the company organization</strong> on <V>{"{{host}}"}</V>, to create the product
                repository, its Environments, rulesets, and the internal App.
              </li>
              <li>
                An <strong>owner of the public organization</strong> <V>{"{{contribOrg}}"}</V> on github.com, using a
                normal (non-EMU) account, to create the fork and the two public Apps.
              </li>
              <li>The <strong>team slugs</strong> of the people who review each gate.</li>
            </ul>
            <p>
              On your machine: <code>git</code>, <Link to="/install">git-uplink</Link>, and for the gh CLI path{" "}
              <a href="https://cli.github.com/">gh</a>. Fill in <strong>Your values</strong> above; every command on
              this page uses them.
            </p>
            <p>
              Commands are for bash or zsh. On Windows, use WSL or Git Bash.
            </p>
          </>
        }
        gh={
          <>
            <p>
              Log in to both hosts. Pushing the pack later includes <code>.github/workflows</code>, so your company
              credential needs the <code>workflow</code> scope. <code>gh auth setup-git</code> makes git use it.
            </p>
            <Command>{`gh auth login --hostname {{host}} --scopes workflow
gh auth setup-git --hostname {{host}}
gh auth login --hostname github.com`}</Command>
            <p>
              If the company organization is also on github.com, gh keeps both accounts; use{" "}
              <code>gh auth switch</code> to change between the public-org steps (1, 2) and the company steps.
            </p>
          </>
        }
      />

      <Step
        n={1}
        id="fork"
        title="Create the contribution fork"
        manual={
          <>
            <p>
              As the public org owner, open the upstream repository on github.com, choose <strong>Fork</strong>, set the
              owner to <V>{"{{contribOrg}}"}</V> and the name to <V>{"{{contribRepo}}"}</V>, and uncheck{" "}
              <strong>Copy the main branch only</strong> if you want all branches (not required).
            </p>
            <p>
              Put the fork in the <strong>same organization as upstream</strong> if you can. Only then can a GitHub App
              (or a fine-grained token) open the public PR from fork to upstream.
            </p>
          </>
        }
        gh={
          <Command>{`gh repo fork {{upstreamSlug}} --org {{contribOrg}} --fork-name {{contribRepo}} --clone=false`}</Command>
        }
      />

      <Step
        n={2}
        id="apps"
        title="Create the three GitHub Apps"
        manual={
          <>
            <p>
              Each role gets its own App so that no job holds more access than it needs. For each App:{" "}
              <strong>Settings → Developer settings → GitHub Apps → New GitHub App</strong> on the owning organization.
              Any homepage URL works. Turn <strong>Webhook → Active</strong> off. Choose{" "}
              <strong>Only on this account</strong>. After creating it, note the <strong>App ID</strong>,{" "}
              <strong>Generate a private key</strong> (a <code>.pem</code> download), then{" "}
              <strong>Install App</strong> on the listed repositories.
            </p>
            <div className="overflow-x-auto">
              <table className="min-w-[640px]">
                <thead>
                  <tr>
                    <th>App</th>
                    <th>Register on</th>
                    <th>Repository permissions</th>
                    <th>Install on</th>
                  </tr>
                </thead>
                <tbody>
                  {APPS.map((app) => (
                    <tr key={app.name}>
                      <td className="text-foreground">{app.name}</td>
                      <td>
                        <V>{app.where}</V>
                      </td>
                      <td>{app.perms}</td>
                      <td>
                        <V>{app.install}</V>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <ul>
              {APPS.map((app) => (
                <li key={app.name}>
                  <strong>{app.name}:</strong> {app.why}
                </li>
              ))}
              <li>
                Register the public Apps from a normal github.com account. An App registered from an EMU account is
                enterprise-scoped and cannot see public repositories.
              </li>
            </ul>
            <p>Enter the three App IDs in Your values; the ruleset and secret steps use them.</p>
          </>
        }
        alternative={{
          summary: "Using PATs instead",
          body: (
            <>
              <p>
                Set the matching <code>UPLINK_*_AUTH</code> variable to <code>pat</code> in step 5 and store{" "}
                <code>UPLINK_*_TOKEN</code> instead of the App ID and key in steps 6 and 7. Same permissions as the
                table.
              </p>
              <p>
                If the fork is <strong>not</strong> in the same organization as upstream, the upstream and contrib
                roles must be a <strong>classic PAT of a machine user</strong> with access to both repositories. Apps
                and fine-grained tokens are limited to one owner and cannot open the public PR. One classic PAT may
                fill both secrets.
              </p>
            </>
          ),
        }}
      />

      <Step
        n={3}
        id="repository"
        title="Create the product repository"
        manual={
          <>
            <p>
              In <V>{"{{companyOrg}}"}</V> create a <strong>private</strong>, <strong>empty</strong> repository named{" "}
              <V>{"{{productRepo}}"}</V> (no README, license, or .gitignore). Then start your local copy from public
              upstream and point <code>origin</code> at the company repository:
            </p>
            <Command>{`git clone {{upstreamUrl}} {{productRepo}}
cd {{productRepo}}
git remote set-url origin {{companyUrl}}`}</Command>
            <p>
              Already have a product repository with your own commits? Skip creation and clone it instead. Step 10
              covers adopting those commits.
            </p>
          </>
        }
        gh={
          <>
            <Command>{`gh repo create {{ghRepo}} --private
git clone {{upstreamUrl}} {{productRepo}}
cd {{productRepo}}
git remote set-url origin {{companyUrl}}`}</Command>
            <p>
              Already have a product repository with your own commits? Clone it instead; step 10 covers adopting them.
            </p>
          </>
        }
      />

      <Step
        n={4}
        id="actions"
        title="Keep Actions permissions minimal"
        manual={
          <>
            <p>
              In the product repository, <strong>Settings → Actions → General → Workflow permissions</strong>:
            </p>
            <ul>
              <li>
                <strong>Read repository contents and packages permissions</strong> is enough. Every Uplink job declares
                the permissions it needs.
              </li>
              <li>
                Leave <strong>Allow GitHub Actions to create and approve pull requests</strong> unchecked. The gated pull
                requests are opened by the internal App, so Actions never needs to create, and can never approve, a pull
                request.
              </li>
            </ul>
          </>
        }
        gh={
          <Command>{`${API} -X PUT repos/{{companyRepo}}/actions/permissions/workflow \\
  -f default_workflow_permissions=read -F can_approve_pull_request_reviews=false`}</Command>
        }
      />

      <Step
        n={5}
        id="variables"
        title="Set repository variables"
        manual={
          <>
            <p>
              <strong>Settings → Secrets and variables → Actions → Variables → New repository variable</strong>:
            </p>
            <VariableTable />
          </>
        }
        gh={
          <>
            <VariableTable />
            <Command>
              {VARIABLES.map(([name, value]) => `gh variable set ${name} --repo {{ghRepo}} --body '${value}'`).join("\n")}
            </Command>
            <p>Replace the values in angle brackets before running.</p>
          </>
        }
      />

      <Step
        n={6}
        id="secrets"
        title="Store the internal and upstream credentials"
        manual={
          <>
            <p>
              <strong>Settings → Secrets and variables → Actions → Secrets → New repository secret</strong>. These are
              repository secrets because sync runs hourly and cannot wait on an Environment.
            </p>
            <ul>
              <li>
                <code>UPLINK_INTERNAL_APP_ID</code> = <V>{"{{internalAppId}}"}</V>, <code>UPLINK_INTERNAL_APP_PRIVATE_KEY</code>{" "}
                = contents of the internal App <code>.pem</code>
              </li>
              <li>
                <code>UPLINK_UPSTREAM_APP_ID</code> = <V>{"{{upstreamAppId}}"}</V>, <code>UPLINK_UPSTREAM_APP_PRIVATE_KEY</code>{" "}
                = contents of the upstream App <code>.pem</code>
              </li>
            </ul>
            <p>Do not store the contrib App here; it goes on Environments in the next step.</p>
          </>
        }
        gh={
          <>
            <Command>{`gh secret set UPLINK_INTERNAL_APP_ID --repo {{ghRepo}} --body {{internalAppId}}
gh secret set UPLINK_INTERNAL_APP_PRIVATE_KEY --repo {{ghRepo}} < internal-app.pem
gh secret set UPLINK_UPSTREAM_APP_ID --repo {{ghRepo}} --body {{upstreamAppId}}
gh secret set UPLINK_UPSTREAM_APP_PRIVATE_KEY --repo {{ghRepo}} < upstream-app.pem`}</Command>
            <p>Delete the downloaded <code>.pem</code> files once every secret is stored.</p>
          </>
        }
        alternative={{
          summary: "Using PATs instead",
          body: (
            <Command>{`gh secret set UPLINK_INTERNAL_TOKEN --repo {{ghRepo}}
gh secret set UPLINK_UPSTREAM_TOKEN --repo {{ghRepo}}`}</Command>
          ),
        }}
      />

      <Step
        n={7}
        id="environments"
        title="Create the three gates"
        manual={
          <>
            <p>
              <strong>Settings → Environments → New environment</strong>, once per row. The names must match exactly.
              For each: add the team under <strong>Required reviewers</strong>, turn on{" "}
              <strong>Prevent self-review</strong>, and under <strong>Deployment branches and tags</strong> choose{" "}
              <strong>Selected branches</strong> and add <code>main</code>.
            </p>
            <EnvironmentTable />
            <p>
              On <code>to-upstream</code> <strong>and</strong> <code>abandon-contrib</code>, add environment secrets{" "}
              <code>UPLINK_CONTRIB_APP_ID</code> = <V>{"{{contribAppId}}"}</V> and{" "}
              <code>UPLINK_CONTRIB_APP_PRIVATE_KEY</code> = the contrib <code>.pem</code>. Environments don&apos;t share
              secrets, so add them twice. Never at repository or organization level.
            </p>
          </>
        }
        gh={
          <>
            <EnvironmentTable />
            {ENVIRONMENTS.map((env) => (
              <Command key={env.name}>{`${env.teamVar}=$(${API} orgs/{{companyOrg}}/teams/{{${env.team}}} --jq .id)
${API} -X PUT repos/{{companyRepo}}/environments/${env.name} --input - <<EOF
{ "prevent_self_review": true,
  "reviewers": [ { "type": "Team", "id": $${env.teamVar} } ],
  "deployment_branch_policy": { "protected_branches": false, "custom_branch_policies": true } }
EOF
${API} -X POST repos/{{companyRepo}}/environments/${env.name}/deployment-branch-policies -f name=main -f type=branch${
                env.secrets
                  ? `
gh secret set UPLINK_CONTRIB_APP_ID --repo {{ghRepo}} --env ${env.name} --body {{contribAppId}}
gh secret set UPLINK_CONTRIB_APP_PRIVATE_KEY --repo {{ghRepo}} --env ${env.name} < contrib-app.pem`
                  : ""
              }`}</Command>
            ))}
          </>
        }
        alternative={{
          summary: "Using PATs instead",
          body: (
            <>
              <p>
                GitHub creates the contribution commit through its API and signs it, but marks it Verified only for
                a GitHub App token. With a PAT the commit is authored by the machine user and shows as unverified.
              </p>
              <Command>{`gh secret set UPLINK_CONTRIB_TOKEN --repo {{ghRepo}} --env to-upstream
gh secret set UPLINK_CONTRIB_TOKEN --repo {{ghRepo}} --env abandon-contrib`}</Command>
            </>
          ),
        }}
      />

      <Step
        n={8}
        id="labels"
        title="Create the labels"
        manual={
          <>
            <p>
              <strong>Issues → Labels → New label</strong>. The workflows read and apply these names:
            </p>
            <LabelList />
          </>
        }
        gh={
          <Command>
            {LABELS.map(
              ([name, color, description]) =>
                `gh label create ${name} --repo {{ghRepo}} --color ${color} --description "${description}"`,
            ).join("\n")}
          </Command>
        }
      />

      <Step
        n={9}
        id="rulesets"
        title="Prepare the rulesets (disabled)"
        manual={
          <>
            <p>
              Create these under <strong>Settings → Rules → Rulesets → New branch ruleset</strong> with{" "}
              <strong>Enforcement status: Disabled</strong>. You push <code>main</code> yourself in step 10; step 11
              turns them on.
            </p>
            <RulesetList />
            <BypassExplainer />
            <p>
              In each ruleset, under <strong>Bypass list → Add bypass</strong>, pick the internal App from the{" "}
              <strong>Apps</strong> list (only Apps installed on the repository appear) and set it to{" "}
              <strong>Always allow</strong>.
            </p>
            <p>
              Faster: <strong>New ruleset → Import a ruleset</strong> with the JSON from the gh CLI view. It already
              contains the bypass list.
            </p>
          </>
        }
        gh={
          <>
            <RulesetList />
            <BypassExplainer />
            <Command>{`${API} -X POST repos/{{companyRepo}}/rulesets --input - <<'EOF'
${MAIN_RULESET}
EOF`}</Command>
            <Command>{`${API} -X POST repos/{{companyRepo}}/rulesets --input - <<'EOF'
${GATED_RULESET}
EOF`}</Command>
            <Command>{`${API} -X POST repos/{{companyRepo}}/rulesets --input - <<'EOF'
${PACK_RULESET}
EOF`}</Command>
          </>
        }
        alternative={{
          summary: "Using a PAT for the internal role",
          body: (
            <>
              <p>
                A bypass is not a property of the token. A PAT acts as the account that created it, so the ruleset must
                let <strong>that account</strong> bypass. Use a dedicated machine user, never a person&apos;s account.
              </p>
              <ol>
                <li>
                  Create a team, for example <code>uplink-bot</code>, in <V>{"{{companyOrg}}"}</V> with only the machine
                  user in it, and give the team <strong>Write</strong> access to <V>{"{{companyRepo}}"}</V>.
                </li>
                <li>
                  In each ruleset, add that team to the bypass list (<strong>Add bypass → Teams</strong>) instead of the
                  internal App.
                </li>
              </ol>
              <p>With gh, look up the team id and use it in place of the internal App entry in the JSON:</p>
              <Command>{`${API} orgs/{{companyOrg}}/teams/uplink-bot --jq .id
# then in each ruleset's bypass_actors, replace the Integration {{internalAppId}} entry with:
#   { "actor_id": <TEAM_ID>, "actor_type": "Team", "bypass_mode": "always" }`}</Command>
            </>
          ),
        }}
      />

      <Step
        n={10}
        id="init"
        title="Initialize and push"
        manual={<InitBody />}
        gh={<InitBody />}
      />

      <Step
        n={11}
        id="enable"
        title="Turn on the rulesets and run a first sync"
        manual={
          <>
            <p>
              <strong>Settings → Rules → Rulesets</strong>: open each <code>Uplink:</code> ruleset and set{" "}
              <strong>Enforcement status</strong> to <strong>Active</strong>.
            </p>
            <p>
              Then <strong>Actions → Uplink sync → Run workflow</strong> on <code>main</code>. A green run confirms the
              internal and upstream credentials. Your first change is a normal pull request to <code>main</code>.
            </p>
          </>
        }
        gh={
          <Command>{`for id in $(${API} repos/{{companyRepo}}/rulesets --jq '.[] | select(.name | startswith("Uplink:")) | .id'); do
  ${API} -X PUT repos/{{companyRepo}}/rulesets/$id -f enforcement=active
done
gh workflow run "Uplink sync" --repo {{ghRepo}}
gh run watch --repo {{ghRepo}} $(gh run list --repo {{ghRepo}} --workflow "Uplink sync" --limit 1 --json databaseId --jq '.[0].databaseId')`}</Command>
        }
      />
    </div>
  );
}

function VariableTable() {
  return (
    <div className="overflow-x-auto">
      <table className="min-w-[560px]">
        <thead>
          <tr>
            <th>Variable</th>
            <th>Example</th>
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
    </div>
  );
}

function EnvironmentTable() {
  return (
    <div className="overflow-x-auto">
      <table className="min-w-[560px]">
        <thead>
          <tr>
            <th>Environment</th>
            <th>Reviewers</th>
            <th>Contrib secrets</th>
            <th>Purpose</th>
          </tr>
        </thead>
        <tbody>
          {ENVIRONMENTS.map((env) => (
            <tr key={env.name}>
              <td className="font-mono text-xs text-foreground">{env.name}</td>
              <td>
                <V>{`{{${env.team}}}`}</V> ({env.who})
              </td>
              <td>{env.secrets ? "yes" : "none"}</td>
              <td>{env.what}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function LabelList() {
  return (
    <ul>
      {LABELS.map(([name, , description]) => (
        <li key={name}>
          <code>{name}</code>: {description}
        </li>
      ))}
    </ul>
  );
}

function RulesetList() {
  return (
    <ul>
      <li>
        <strong>Uplink: company main</strong> (default branch): pull request with one approval, required checks{" "}
        <code>Uplink upstream assess</code> and <code>Uplink upstream preflight</code>, no force-push or deletion. Bypass:
        the internal App, which rebuilds <code>main</code>.
      </li>
      <li>
        <strong>Uplink: gated bases</strong> (<code>uplink/conflict/**</code>, <code>uplink/transfer-to-*/**</code>,
        excluding <code>*-work</code>): pull request with one approval, required check <code>Uplink gate</code>, no direct creation, deletion, or force-push. Bypass: the internal App, which creates and deletes these branches.
      </li>
      <li>
        <strong>Uplink: pack files</strong> (same refs, including <code>*-work</code>): nobody but the bots may change{" "}
        <code>.github/workflows/uplink-*.yml</code>, <code>.github/actions/install-git-uplink/**</code>, or{" "}
        <code>.github/uplink/**</code> on a gated
        branch. Product workflows stay editable. Bypass: the internal App. Sample:{" "}
        <a href={`${GITHUB_BLOB}/templates/github/uplink-pack-files-ruleset.json`}>uplink-pack-files-ruleset.json</a>.
      </li>
      <li>
        <strong>Uplink: hooks branch</strong> (<code>uplink/hooks</code>): pull request with one approval, no deletion or
        force-push. The assessment hook on this branch runs during every submit. Bypass: GitHub Actions, so the
        placeholder workflow can update <code>assessment-hook.md</code>. Sample:{" "}
        <a href={`${GITHUB_BLOB}/templates/github/uplink-hooks-ruleset.json`}>uplink-hooks-ruleset.json</a>.
      </li>
    </ul>
  );
}

function BypassExplainer() {
  return (
    <div className="rounded-lg border border-border bg-card px-4 py-3 text-sm">
      <p className="mb-2 text-foreground">Who bypasses, and why</p>
      <ul>
        <li>
          <strong>Internal App</strong> (<code>Integration</code>, id <V>{"{{internalAppId}}"}</V>): rebuilds and
          force-pushes <code>main</code>, and creates and deletes the gated branches. It needs bypass on every ruleset.
          A ruleset can only list an App that is installed on the repository, which step 2 did.
        </li>
        <li>
          <strong>GitHub Actions</strong> (<code>GITHUB_TOKEN</code>) needs no bypass. It only reads, comments, labels,
          and fast-forwards <code>uplink/state</code>, which none of these rulesets cover.
        </li>
        <li>People never bypass. They merge through pull requests.</li>
      </ul>
    </div>
  );
}

function InitBody() {
  return (
    <>
      <p>
        In your local copy from step 3, record the queue, install the pack as the tooling patch, and push. This is the
        only time a person pushes <code>main</code>.
      </p>
      <Command>{`git uplink init --upstream {{upstreamUrl}} --contrib {{contribUrl}} --forge ghec
git uplink status
git push -u origin main
git push origin uplink/state uplink/upstream`}</Command>
      <p>
        <code>status</code> should show <strong>Uplink tooling</strong> in the tooling slot and an empty queue. If
        upstream&apos;s default branch is not <code>main</code>, add <code>--upstream-branch &lt;name&gt;</code> to{" "}
        <code>init</code>.
      </p>
      <details className="rounded-lg border border-border px-4 py-2">
        <summary className="cursor-pointer text-sm text-foreground">
          The product repository already has commits ahead of upstream
        </summary>
        <div className="mt-3 space-y-3">
          <p>
            <code>init</code> leaves <code>main</code> alone and turns each first-parent commit (or group you choose) into
            a queued patch after the tooling. Preview the rebuilt tree, compare, then publish:
          </p>
          <Command>{`git uplink init --upstream {{upstreamUrl}} --contrib {{contribUrl}} --forge ghec
git uplink rebuild --branch uplink/preview/verify
git diff main uplink/preview/verify
git uplink rebuild --push
git push origin uplink/upstream`}</Command>
          <p>
            If company <code>main</code> is behind upstream, merge upstream first; <code>init</code> refuses diverged
            history.
          </p>
        </div>
      </details>
    </>
  );
}
