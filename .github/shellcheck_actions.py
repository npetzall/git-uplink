"""Run shellcheck on the `run:` steps of composite actions.

actionlint checks the scripts in workflows but not in composite actions
(`action.yml`), so this pre-commit hook does it. Each `run:` step with a
bash or sh shell goes to shellcheck in the Docker image given with --image.
`${{ ... }}` expressions are replaced by underscores of the same length, as
actionlint does, so lines and columns stay where they are. Findings are
printed as `path:line:col: message` against the action file.

Usage: shellcheck_actions.py --image <image> <action.yml>...
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys

import yaml

EXPRESSION = re.compile(r"\$\{\{.*?\}\}", re.DOTALL)
FINDING = re.compile(r"^-:(\d+):(\d+): (.*)$")
SHELLS = ("bash", "sh")


def mapping_get(node: yaml.Node, key: str) -> yaml.Node | None:
    """The value node for `key` in a mapping node, or None."""
    if not isinstance(node, yaml.MappingNode):
        return None
    for key_node, value_node in node.value:
        if isinstance(key_node, yaml.ScalarNode) and key_node.value == key:
            return value_node
    return None


def run_steps(root: yaml.Node) -> list[tuple[str, yaml.ScalarNode]]:
    """(shell, run node) for each bash or sh `run:` step of a composite action."""
    runs = mapping_get(root, "runs")
    using = mapping_get(runs, "using") if runs else None
    steps = mapping_get(runs, "steps") if runs else None
    if not isinstance(using, yaml.ScalarNode) or using.value != "composite":
        return []
    if not isinstance(steps, yaml.SequenceNode):
        return []
    found = []
    for step in steps.value:
        run = mapping_get(step, "run")
        shell = mapping_get(step, "shell")
        if not isinstance(run, yaml.ScalarNode) or not isinstance(
            shell, yaml.ScalarNode
        ):
            continue
        name = shell.value.split()[0] if shell.value.split() else ""
        if name in SHELLS:
            found.append((name, run))
    return found


def position(
    run: yaml.ScalarNode, lines: list[str], line: int, col: int
) -> tuple[int, int]:
    """Where line and column `line`:`col` of the script are in the action file."""
    mark = run.start_mark
    if run.style in ("|", ">"):
        # The script starts on the line after the block indicator.
        first = mark.line + 1
        source = lines[first] if first < len(lines) else ""
        indent = len(source) - len(source.lstrip(" "))
        return first + line, indent + col
    return mark.line + line, mark.column + col


def check(path: str, docker: str, image: str) -> list[str]:
    with open(path, encoding="utf-8") as f:
        text = f.read()
    root = yaml.compose(text, Loader=yaml.SafeLoader)
    if root is None:
        return []
    lines = text.splitlines()
    findings = []
    for shell, run in run_steps(root):
        script = EXPRESSION.sub(lambda m: "_" * len(m.group(0)), run.value)
        # The arguments are fixed but for the image from .pre-commit-config.yaml;
        # the script only goes to stdin.
        result = subprocess.run(  # noqa: S603
            [docker, "run", "--rm", "-i", image, "-f", "gcc", "-s", shell, "-"],
            input=script,
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode not in (0, 1):
            findings.append(f"{path}: shellcheck failed: {result.stderr.strip()}")
            continue
        for out in result.stdout.splitlines():
            match = FINDING.match(out)
            if not match:
                findings.append(f"{path}: {out}")
                continue
            line, col = position(run, lines, int(match[1]), int(match[2]))
            findings.append(f"{path}:{line}:{col}: {match[3]}")
    return findings


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--image", required=True, help="shellcheck Docker image")
    parser.add_argument("paths", nargs="*", help="action.yml files")
    args = parser.parse_args(argv)
    docker = shutil.which("docker")
    if docker is None:
        print("shellcheck_actions.py needs docker on PATH", file=sys.stderr)
        return 2
    findings = [
        finding for path in args.paths for finding in check(path, docker, args.image)
    ]
    for finding in findings:
        print(finding)
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
