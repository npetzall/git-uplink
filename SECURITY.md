# Security policy

## Reporting a vulnerability

Please do not open a public issue. Report it privately through GitHub's
[private vulnerability reporting](https://github.com/npetzall/git-uplink/security/advisories/new).
Include the affected version, how to reproduce it, and the impact you see.

## Supported versions

Only the latest release is supported. The binary and the forge pack are
released and upgraded together, so a fix ships as a new release of both.

## Verifying a release

Release assets are signed with cosign (`*.sigstore.json`) and carry build
provenance and SBOM attestations. `gh attestation verify` checks them.
