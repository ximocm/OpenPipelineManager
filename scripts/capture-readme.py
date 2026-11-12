#!/usr/bin/env python3
"""Capture real GTK windows using isolated demo projects; run from repo root."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--theme", choices=["light", "dark"], action="append")
    parser.add_argument("--marketplace", action="store_true", help="also capture the live public registry")
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="opm-readme-"))
    if not root.is_relative_to(Path(tempfile.gettempdir()).resolve()):
        parser.error("output must be under the system temporary directory")
    print(f"Screenshot output: {root}", flush=True)
    for theme in args.theme or ["light", "dark"]:
        output = root / theme
        env = os.environ.copy()
        for key in ["OPM_QA_LARGE_TEXT", "OPM_CAPTURE_MARKETPLACE"]:
            env.pop(key, None)
        env.update(GTK_THEME="Adwaita:dark" if theme == "dark" else "Adwaita", GSK_RENDERER="cairo",
                   OPM_QA_DIR=str(output), XDG_CONFIG_HOME=str(output / "config"),
                   OPM_PIPELINES_DIR=str(output / "pipelines"), OPM_TOOLS_DIR=str(output / "tools"))
        if args.marketplace and theme == "light":
            env["OPM_CAPTURE_MARKETPLACE"] = "1"
        subprocess.run(["cargo", "test", "--locked", "--manifest-path", "ui/Cargo.toml",
                        "readme_screenshots", "--", "--ignored", "--test-threads=1", "--nocapture"],
                       env=env, check=True)


if __name__ == "__main__":
    main()
