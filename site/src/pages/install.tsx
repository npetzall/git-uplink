import { Link } from "react-router-dom";
import { DocPage } from "../components/doc-page";
import { Button } from "../components/ui/button";
import { GITHUB_BLOB, GITHUB_REPO } from "../lib/links";

const code = "rounded bg-muted px-1.5 py-0.5 text-foreground";
const pre =
  "overflow-x-auto rounded-lg bg-black/40 p-4 font-mono text-xs leading-6 text-zinc-200";
const link = "text-primary underline-offset-4 hover:underline";

export function InstallPage() {
  return (
    <DocPage
      eyebrow="Install"
      title="Get git uplink on PATH"
      lead={
        <>
          The binary is <code>git-uplink</code>, so Git treats it as{" "}
          <code>git uplink</code>. The <Link to="/operator">operator</Link>{" "}
          installs it, and the forge installs its own copy in every job.
          Developers don&apos;t need it. On macOS and Linux, a signed script
          installs it under <code>~/.uplink</code>, and after that it upgrades
          itself.
        </>
      }
    >
      <section className="space-y-3">
        <h2 className="text-xl font-semibold">macOS and Linux</h2>
        <p className="text-[15px] leading-7 text-muted-foreground">
          Download <code className={code}>install.sh</code> from the latest
          release, check its{" "}
          <a
            href="https://docs.sigstore.dev/cosign/system_config/installation/"
            className={link}
          >
            cosign
          </a>{" "}
          signature, and run it. It needs <code className={code}>curl</code> and{" "}
          <code className={code}>cosign</code>.
        </p>
        <pre
          className={pre}
        >{`curl -fsSLO https://github.com/npetzall/git-uplink/releases/latest/download/install.sh
curl -fsSLO https://github.com/npetzall/git-uplink/releases/latest/download/install.sh.sigstore.json
cosign verify-blob install.sh --bundle install.sh.sigstore.json \\
  --certificate-identity https://github.com/npetzall/git-uplink/.github/workflows/release.yml@refs/heads/main \\
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
sh install.sh`}</pre>
        <p className="text-[15px] leading-7 text-muted-foreground">
          Each version is kept as{" "}
          <code className={code}>~/.uplink/bin/git-uplink-X.Y.Z</code>, and{" "}
          <code className={code}>~/.uplink/bin/git-uplink</code> links to the
          active one. Every download is checked against the release&apos;s{" "}
          <code className={code}>SHA256SUMS</code>, its cosign signature by the
          release workflow, and the version the binary reports. Put{" "}
          <code className={code}>~/.uplink/bin</code> first on{" "}
          <code className={code}>PATH</code>, then open a new shell:
        </p>
        <pre
          className={pre}
        >{`echo 'export PATH="$HOME/.uplink/bin:$PATH"' >> ~/.zshrc        # zsh
echo 'export PATH="$HOME/.uplink/bin:$PATH"' >> ~/.bashrc       # bash on Linux
echo 'export PATH="$HOME/.uplink/bin:$PATH"' >> ~/.bash_profile # bash on macOS`}</pre>
      </section>

      <section className="space-y-3">
        <h2 className="text-xl font-semibold">Upgrade and switch versions</h2>
        <p className="text-[15px] leading-7 text-muted-foreground">
          The binary installs other versions with the same checks, and switches
          between them by moving the link.
        </p>
        <pre
          className={pre}
        >{`git uplink self-available        # the ten latest releases
git uplink self-check            # is a newer version installed or released?
git uplink self-update           # install the latest and switch to it
git uplink self-update 1.2.3     # install a given version and switch to it
git uplink self-list             # installed versions, * marks the active one
git uplink self-use 1.2.3        # switch back to an installed version`}</pre>
        <p className="text-[15px] leading-7 text-muted-foreground">
          Releases come from the repository in{" "}
          <code className={code}>~/.uplink/config.toml</code>, which{" "}
          <code className={code}>install.sh</code> writes. A fork points its
          users at its own releases and release workflow (
          <code className={code}>sh install.sh --repo acme/git-uplink</code>{" "}
          writes it on a first install):
        </p>
        <pre className={pre}>{`repo = "acme/git-uplink"
signer = "https://github.com/acme/git-uplink/.github/workflows/release.yml@refs/heads/main"`}</pre>
      </section>

      <section className="space-y-3">
        <h2 className="text-xl font-semibold">By hand</h2>
        <p className="text-[15px] leading-7 text-muted-foreground">
          With the{" "}
          <a href="https://cli.github.com/" className={link}>
            GitHub CLI
          </a>{" "}
          you can also check GitHub&apos;s release attestation and the build
          provenance:
        </p>
        <pre
          className={pre}
        >{`target=aarch64-apple-darwin   # or x86_64-unknown-linux-musl, aarch64-unknown-linux-musl
gh release download --repo npetzall/git-uplink --pattern "git-uplink-$target" --pattern "git-uplink-$target.sigstore.json" --pattern SHA256SUMS
grep " git-uplink-$target\\$" SHA256SUMS | shasum -a 256 -c
cosign verify-blob "git-uplink-$target" --bundle "git-uplink-$target.sigstore.json" \\
  --certificate-identity https://github.com/npetzall/git-uplink/.github/workflows/release.yml@refs/heads/main \\
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
gh release verify-asset --repo npetzall/git-uplink "git-uplink-$target"
gh attestation verify "git-uplink-$target" --repo npetzall/git-uplink
chmod +x "git-uplink-$target"
mkdir -p ~/.local/bin && mv "git-uplink-$target" ~/.local/bin/git-uplink
git uplink version`}</pre>
        <p className="text-[15px] leading-7 text-muted-foreground">
          Without <code className={code}>gh</code>, download the same files from{" "}
          <a href={`${GITHUB_REPO}/releases`} className={link}>
            Releases
          </a>
          .
        </p>
      </section>

      <section className="space-y-3">
        <h2 className="text-xl font-semibold">Windows</h2>
        <p className="text-[15px] leading-7 text-muted-foreground">
          Download{" "}
          <code className={code}>git-uplink-x86_64-pc-windows-msvc.exe</code>{" "}
          from{" "}
          <a href={`${GITHUB_REPO}/releases`} className={link}>
            Releases
          </a>
          , rename it to <code className={code}>git-uplink.exe</code>, and place
          it in a directory on <code className={code}>PATH</code>.
        </p>
      </section>

      <section className="space-y-3">
        <h2 className="text-xl font-semibold">Check it</h2>
        <p className="text-[15px] leading-7 text-muted-foreground">
          Use <code className={code}>git-uplink -h</code> or{" "}
          <code className={code}>git uplink -h</code>. Plain{" "}
          <code className={code}>git uplink --help</code> goes through
          Git&apos;s man-page path, not clap.{" "}
          <code className={code}>git uplink web-ui</code> opens the local
          operator dashboard for the checkout you start it in (
          <code className={code}>--no-open</code>,{" "}
          <code className={code}>--port 43721</code>).
        </p>
      </section>

      <section className="space-y-3">
        <h2 className="text-xl font-semibold">Next</h2>
        <p className="text-[15px] leading-7 text-muted-foreground">
          Installing only adds the command. To see it work, follow{" "}
          <Link to="/examples" className={link}>
            Try it yourself
          </Link>{" "}
          on three GitHub repositories. To wire up a real product repository, go
          to{" "}
          <Link to="/setup" className={link}>
            Production setup
          </Link>
          . Every command and flag is in the{" "}
          <Link to="/cli" className={link}>
            CLI reference
          </Link>
          . Building from source is in{" "}
          <a href={`${GITHUB_BLOB}/CONTRIBUTING.md`} className={link}>
            CONTRIBUTING.md
          </a>
          .
        </p>
      </section>

      <Button asChild variant="outline" size="sm">
        <a href={GITHUB_REPO}>Source on GitHub</a>
      </Button>
    </DocPage>
  );
}
