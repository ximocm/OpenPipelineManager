#!/usr/bin/env python3
"""Reject tags that do not identify the checked-out version on main."""

import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib


def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()


def main():
    tag = sys.argv[1]
    if not re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", tag):
        raise SystemExit("Expected a version tag such as v0.1.0 or v0.1.0-rc.1")
    sha = git("rev-parse", f"refs/tags/{tag}^{{commit}}")
    if sha != git("rev-parse", "HEAD"):
        raise SystemExit("Select the release tag as the workflow ref, not main")
    subprocess.run(["git", "merge-base", "--is-ancestor", sha, "origin/main"], check=True)
    core = tomllib.loads(Path("core/Cargo.toml").read_text())["workspace"]["package"]["version"]
    ui = tomllib.loads(Path("ui/Cargo.toml").read_text())["package"]["version"]
    if core != ui or tag != f"v{ui}":
        raise SystemExit(f"Tag {tag} does not match core={core}, ui={ui}")
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        output.write(f"tag={tag}\nsha={sha}\n")


if __name__ == "__main__":
    main()
