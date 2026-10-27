#!/usr/bin/env python3
"""Package the Linux x86_64 binary, documentation and dependency license files."""

import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def main():
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise SystemExit("This package supports Linux x86_64 only")
    version = tomllib.loads((ROOT / "ui/Cargo.toml").read_text())["package"]["version"]
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    binary = target / "release/opm"
    if not binary.is_file():
        raise SystemExit(f"Build the release binary first: {binary}")
    libraries = subprocess.check_output(["ldd", str(binary)], text=True)
    if "not found" in libraries:
        raise SystemExit(f"Missing shared libraries:\n{libraries}")
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--format-version", "1",
        "--manifest-path", str(ROOT / "ui/Cargo.toml"),
        "--filter-platform", "x86_64-unknown-linux-gnu",
    ], text=True))
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    name = f"opm-{version}-linux-x86_64"
    with tempfile.TemporaryDirectory(prefix="opm-package-") as temp:
        stage = Path(temp) / name
        stage.mkdir()
        shutil.copy2(binary, stage / "opm")
        for filename in ["LICENSE", "NOTICE", "README.md", "BUILDING.md", "CHANGELOG.md"]:
            shutil.copy2(ROOT / filename, stage / filename)
        (stage / "INSTALL.txt").write_text(
            "opm for Linux x86_64, built on Ubuntu 24.04.\n"
            "Requires GTK >= 4.10 shared libraries.\n"
            "Ubuntu 24.04: sudo apt install libgtk-4-1\n"
            "Run ./opm from a graphical Wayland or X11 session.\n"
            "This archive is not a static or universally portable executable.\n"
            "No Windows or macOS application is included.\n"
        )
        notices = stage / "third-party"
        notices.mkdir()
        inventory = []
        for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
            if package["source"] is None:
                continue
            source = Path(package["manifest_path"]).parent
            dest = notices / f'{package["name"]}-{package["version"]}'
            dest.mkdir()
            candidates = [p for p in source.iterdir() if
                          p.name.lower().startswith(("license", "licence", "copying", "notice", "copyright"))]
            if package.get("license_file"):
                candidates.append(source / package["license_file"])
            for path in set(candidates):
                if path.is_file():
                    shutil.copy2(path, dest / path.name)
                elif path.is_dir():
                    shutil.copytree(path, dest / path.name, dirs_exist_ok=True)
            inventory.append({key: package.get(key) for key in
                              ("name", "version", "license", "repository")})
        (notices / "inventory.json").write_text(json.dumps(inventory, indent=2) + "\n")
        # Procedencia Apache-2.0 del código derivado: se copia de forma
        # explícita porque sus fuentes no son una dependencia de cargo.
        shutil.copy2(ROOT / "third-party" / "minmark-Apache-2.0.txt",
                     notices / "minmark-Apache-2.0.txt")
        (stage / "shared-libraries.txt").write_text(libraries)
        archive = dist / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(stage, arcname=name)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (dist / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n")
    print(f"Packaged {archive.name}")


if __name__ == "__main__":
    main()
