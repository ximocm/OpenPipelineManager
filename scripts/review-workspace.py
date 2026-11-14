#!/usr/bin/env python3
"""Capture the actual GTK workspace in isolated, offline review sessions.

Run from the repository root with access to a desktop display. Output must
be under the system temporary directory; review PNGs before copying to docs.
"""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--mode", choices=["light", "dark", "contrast", "large", "live"], action="append")
    parser.add_argument("--surface", choices=["workspace", "navigation"], default="workspace")
    args = parser.parse_args()
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="opm-workspace-review-"))
    if not root.is_relative_to(Path(tempfile.gettempdir()).resolve()):
        parser.error("output must be inside the system temporary directory")
    print(f"Review output: {root}", flush=True)
    for mode in args.mode or ["light", "dark", "contrast", "large", "live"]:
        output = root / mode
        env = os.environ.copy()
        env.pop("OPM_QA_LARGE_TEXT", None)
        env.update(GTK_THEME={"dark": "Adwaita:dark", "contrast": "HighContrast"}.get(mode, "Adwaita"),
                   GSK_RENDERER="cairo", XDG_CONFIG_HOME=str(output / "config"),
                   OPM_PIPELINES_DIR=str(output / "pipelines"), OPM_TOOLS_DIR=str(output / "tools"),
                   OPM_QA_DIR=str(output))
        if mode == "large":
            env["OPM_QA_LARGE_TEXT"] = "1"
        if mode == "live":
            env.pop("GTK_THEME", None)
        subprocess.run(["cargo", "test", "--locked", "--manifest-path", "ui/Cargo.toml",
                        "explorer_navigation_review" if args.surface == "navigation" else "workspace_visual_review",
                        "--", "--ignored", "--test-threads=1", "--nocapture"],
                       env=env, check=True)


if __name__ == "__main__":
    main()
