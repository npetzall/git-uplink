#!/usr/bin/env python3
"""Recreate the local Uplink export commit on contrib through the Git Database API.

`git uplink submit` (without --push) builds the export commit locally and prints
`contribCommit`. This script recreates that commit on the contrib repository
with the REST Git Database API, so GitHub signs it. It is shown as Verified only
when UPLINK_CONTRIB_TOKEN is a GitHub App installation token; a PAT works but
the commit is unverified. No author, committer, or signature is sent: the
token's identity is the author.

Environment:
  CONTRIB_COMMIT        the `contribCommit` JSON from `git uplink submit`
  CONTRIB_REPO          owner/name of the contrib repository
  UPLINK_CONTRIB_TOKEN  token with contents write on CONTRIB_REPO
  GITHUB_API_URL        default https://api.github.com
  GITHUB_SERVER_URL     default https://github.com
  CONTRIB_GIT_URL       optional git URL for the base push (default
                        $GITHUB_SERVER_URL/$CONTRIB_REPO.git)

Prints {"sha", "verified", "reason"} as JSON. Python standard library only.
"""

import base64
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

GITLINK = "160000"


class ApiError(Exception):
    def __init__(self, method, path, status, body):
        super().__init__(f"{method} {path} failed with {status}: {body}")
        self.status = status


class Api:
    def __init__(self, base_url, repo, token):
        url = urllib.parse.urlsplit(base_url)
        loopback = url.hostname in ("127.0.0.1", "localhost", "::1")
        if url.scheme != "https" and not (url.scheme == "http" and loopback):
            raise SystemExit(f"GITHUB_API_URL must be https, got {base_url!r}")
        self.base = f"{base_url.rstrip('/')}/repos/{repo}"
        self.token = token

    def call(self, method, path, payload=None, attempts=3):
        data = None if payload is None else json.dumps(payload).encode()
        for attempt in range(1, attempts + 1):
            # The scheme is checked in __init__: https, or http on loopback.
            request = urllib.request.Request(  # noqa: S310
                self.base + path,
                data=data,
                method=method,
                headers={
                    "Accept": "application/vnd.github+json",
                    "Authorization": f"Bearer {self.token}",
                    "Content-Type": "application/json",
                    "X-GitHub-Api-Version": "2022-11-28",
                },
            )
            try:
                with urllib.request.urlopen(request) as response:  # noqa: S310
                    body = response.read()
                    return json.loads(body) if body else None
            except urllib.error.HTTPError as err:
                body = err.read().decode(errors="replace")
                if err.code >= 500 and attempt < attempts:
                    time.sleep(2 * attempt)
                    continue
                raise ApiError(method, path, err.code, body) from None
        raise AssertionError("unreachable")


def git(*args, input=None, env=None):
    # Argument list, no shell; git comes from PATH like every other step.
    result = subprocess.run(  # noqa: S603
        ["git", *args],  # noqa: S607
        input=input,
        capture_output=True,
        check=False,
        env=env,
    )
    if result.returncode != 0:
        raise SystemExit(
            f"git {' '.join(args)} failed: {result.stderr.decode(errors='replace').strip()}"
        )
    return result.stdout


def changed_entries(base, local):
    """Raw `git diff-tree` records: (new_mode, new_sha, status, old_mode, path)."""
    raw = git("diff-tree", "-r", "-z", "--no-renames", base, local)
    fields = raw.split(b"\0")
    entries = []
    i = 0
    while i + 1 < len(fields) and fields[i]:
        meta = fields[i].decode()
        path = fields[i + 1].decode()
        i += 2
        old_mode, new_mode, _old_sha, new_sha, status = meta.lstrip(":").split(" ")
        entries.append((new_mode, new_sha, status, old_mode, path))
    return entries


def tree_entries(api, entries):
    """Uploads changed blobs and returns the tree entries for `POST git/trees`."""
    tree = []
    for new_mode, new_sha, status, old_mode, path in entries:
        if status == "D":
            kind = "commit" if old_mode == GITLINK else "blob"
            tree.append({"path": path, "mode": old_mode, "type": kind, "sha": None})
            continue
        if new_mode == GITLINK:
            tree.append(
                {"path": path, "mode": new_mode, "type": "commit", "sha": new_sha}
            )
            continue
        content = git("cat-file", "blob", new_sha)
        blob = api.call(
            "POST",
            "/git/blobs",
            {"content": base64.b64encode(content).decode(), "encoding": "base64"},
        )
        if blob["sha"] != new_sha:
            raise SystemExit(
                f"blob for {path} came back as {blob['sha']}, expected {new_sha}"
            )
        tree.append({"path": path, "mode": new_mode, "type": "blob", "sha": new_sha})
    return tree


def push_env(git_url, token):
    """Token as an http.extraheader through GIT_CONFIG_* so it stays out of argv."""
    env = dict(os.environ)
    if not git_url.startswith(("https://", "http://")):
        return env
    basic = base64.b64encode(f"x-access-token:{token}".encode()).decode()
    key = f"http.{git_url}.extraheader"
    env.update(
        {
            "GIT_CONFIG_COUNT": "2",
            "GIT_CONFIG_KEY_0": key,
            "GIT_CONFIG_VALUE_0": "",
            "GIT_CONFIG_KEY_1": key,
            "GIT_CONFIG_VALUE_1": f"AUTHORIZATION: basic {basic}",
            "GIT_TERMINAL_PROMPT": "0",
        }
    )
    return env


def has_commit(api, sha):
    try:
        api.call("GET", f"/git/commits/{sha}")
        return True
    except ApiError as err:
        if err.status in (404, 422):
            return False
        raise


def set_branch(api, branch, sha):
    ref = urllib.parse.quote(f"heads/{branch}")
    try:
        api.call("PATCH", f"/git/refs/{ref}", {"sha": sha, "force": True})
    except ApiError as err:
        if err.status != 422:
            raise
        api.call("POST", "/git/refs", {"ref": f"refs/heads/{branch}", "sha": sha})


def delete_branch(api, branch):
    try:
        api.call("DELETE", f"/git/refs/{urllib.parse.quote(f'heads/{branch}')}")
    except ApiError as err:
        print(f"::warning::could not delete {branch}: {err}", file=sys.stderr)


def main():
    commit = json.loads(os.environ["CONTRIB_COMMIT"])
    repo = os.environ["CONTRIB_REPO"]
    token = os.environ["UPLINK_CONTRIB_TOKEN"]
    api_url = os.environ.get("GITHUB_API_URL") or "https://api.github.com"
    server_url = os.environ.get("GITHUB_SERVER_URL") or "https://github.com"
    git_url = (
        os.environ.get("CONTRIB_GIT_URL") or f"{server_url.rstrip('/')}/{repo}.git"
    )

    branch = commit["branch"]
    base = commit["baseSha"]
    local = commit["localSha"]
    expected_tree = commit["treeSha"]
    with open(commit["messageFile"], encoding="utf-8") as handle:
        message = handle.read()

    api = Api(api_url, repo, token)

    # The base is an upstream commit. A fork usually has it already; otherwise
    # push it to a scratch branch, never to the PR branch, which an open PR
    # would show as empty in between.
    scratch = None
    if not has_commit(api, base):
        scratch = f"uplink-base/{branch.removeprefix('uplink/')}"
        git(
            "push",
            "--force",
            "--quiet",
            git_url,
            f"{base}:refs/heads/{scratch}",
            env=push_env(git_url, token),
        )
    try:
        entries = changed_entries(base, local)
        if not entries:
            raise SystemExit(f"{local} has no changes on {base}")
        base_tree = git("rev-parse", f"{base}^{{tree}}").decode().strip()
        tree = api.call(
            "POST",
            "/git/trees",
            {"base_tree": base_tree, "tree": tree_entries(api, entries)},
        )
        if tree["sha"] != expected_tree:
            raise SystemExit(
                f"contrib tree {tree['sha']} does not match the local export tree {expected_tree}"
            )
        created = api.call(
            "POST",
            "/git/commits",
            {"message": message, "tree": tree["sha"], "parents": [base]},
        )
        set_branch(api, branch, created["sha"])
    finally:
        if scratch:
            delete_branch(api, scratch)

    verification = created.get("verification") or {}
    verified = bool(verification.get("verified"))
    reason = verification.get("reason", "")
    if not verified:
        note = (
            f"Contrib commit {created['sha']} is not Verified ({reason or 'unsigned'}). "
            "Use a GitHub App token for UPLINK_CONTRIB to get signed, verified commits."
        )
        print(f"::warning::{note}", file=sys.stderr)
        summary = os.environ.get("GITHUB_STEP_SUMMARY")
        if summary:
            with open(summary, "a", encoding="utf-8") as handle:
                handle.write(f"> [!WARNING]\n> {note}\n")
    print(json.dumps({"sha": created["sha"], "verified": verified, "reason": reason}))


if __name__ == "__main__":
    main()
