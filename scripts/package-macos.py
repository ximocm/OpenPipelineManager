#!/usr/bin/env python3
"""Package the macOS arm64 build: opm.app inside a .dmg and a .tar.gz.

Non-system dylibs (Homebrew GTK 4) are collected by walking `otool -L` and
their install names are rewritten to @executable_path/@loader_path, so the
bundle does not depend on a Homebrew prefix at runtime. The app is ad-hoc
signed in CI; Developer ID signing and notarization are pending.
"""

import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import time
import tarfile
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parent.parent

SYSTEM_PREFIXES = ("/usr/lib/", "/System/Library/")
BREW_PREFIXES = ("/opt/homebrew", "/usr/local")
SMOKE_SECONDS = 10
FRAMEWORKS_SUBDIR = "Contents/Frameworks"
EXECUTABLE_PREFIX = "@executable_path/../Frameworks/"
LOADER_PREFIX = "@loader_path/"


def is_system_dependency(dep):
    """True si la dependencia es del sistema (no hay que empaquetarla)."""
    return dep.startswith(SYSTEM_PREFIXES)


def is_internal_dependency(dep):
    """True si ya se resuelve dentro del bundle (@executable_path/@loader_path)."""
    return dep.startswith(("@executable_path/", "@loader_path/"))


def rewrite_plan(deps, prefix):
    """Pares (old, new) para reescribir las dependencias no-sistema."""
    return [(dep, prefix + Path(dep).name)
            for dep in deps
            if not is_system_dependency(dep) and not is_internal_dependency(dep)]


def parse_otool_deps(text):
    """Rutas de dependencias de la salida de `otool -L` (un solo binario)."""
    deps = []
    for line in text.splitlines():
        line = line.strip()
        if not line or line.endswith(":"):
            continue
        deps.append(line.split()[0])
    return deps


def parse_otool_rpaths(text):
    """LC_RPATH de la salida de `otool -l` (un solo binario)."""
    rpaths = []
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("path ") and " (offset " in line:
            rpaths.append(line[len("path "):].rsplit(" (offset ", 1)[0])
    return rpaths


def info_plist(version):
    return {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleExecutable": "opm",
        "CFBundleIdentifier": "app.opm",
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": "opm",
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "NSHighResolutionCapable": True,
        "NSPrincipalClass": "NSApplication",
    }


def bundle_layout(app):
    """Rutas del `.app`: `Contents`, la carpeta de dylibs y el binario.

    `FRAMEWORKS_SUBDIR` es relativo a la raíz del bundle (no a `Contents`),
    que es donde resuelve `EXECUTABLE_PREFIX` visto desde `MacOS/`.
    """
    contents = app / "Contents"
    return contents, app / FRAMEWORKS_SUBDIR, contents / "MacOS" / "opm"


def stage_notices(stage, metadata):
    """Inventario de licencias de dependencias (mismo criterio que package-linux)."""
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
                      p.name.lower().startswith(("license", "licence", "copying",
                                                 "notice", "copyright"))]
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
    # Procedencia Apache-2.0 del código derivado: se copia de forma explícita
    # porque sus fuentes no son una dependencia de cargo.
    shutil.copy2(ROOT / "third-party" / "minmark-Apache-2.0.txt",
                 notices / "minmark-Apache-2.0.txt")


def install_text(version):
    return (
        f"opm {version} for macOS (Apple Silicon / arm64), built on the\n"
        "macos-15 GitHub runner with GTK 4 from Homebrew.\n"
        "\n"
        "Requirements: a Mac with an Apple Silicon chip. The GTK 4 libraries\n"
        "are bundled in opm.app/Contents/Frameworks; Homebrew is not\n"
        "needed to run the app.\n"
        "\n"
        "The app is ad-hoc signed in CI: Developer ID signing and notarization\n"
        "are pending, so Gatekeeper warns on first open. Open it with\n"
        "right click > Open, or clear the quarantine flag:\n"
        "  xattr -dr com.apple.quarantine opm.app\n"
    )


def run(command):
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(
            f"Command failed ({result.returncode}): {' '.join(command)}\n"
            f"{result.stdout}\n{result.stderr}")
    return result.stdout


def otool_deps(path):
    return parse_otool_deps(subprocess.check_output(
        ["otool", "-L", str(path)], text=True))


def image_rpaths(image):
    return parse_otool_rpaths(subprocess.check_output(
        ["otool", "-l", str(image)], text=True))


def resolve_dependency(dep, staged_names, extra_dirs=()):
    base = Path(dep).name
    if base in staged_names:
        return None  # ya está en Frameworks
    candidates = [Path(dep)]
    candidates += [Path(directory) / base for directory in extra_dirs]
    candidates += [Path(prefix) / "lib" / base for prefix in BREW_PREFIXES]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise SystemExit(f"Cannot locate dependency: {dep}")


def collect_and_rewrite(binary, frameworks):
    """Copia las dylibs no-sistema a Frameworks y reescribe las rutas."""
    staged_names = set()
    rewrites = {binary: []}
    queue = [binary]
    while queue:
        image = queue.pop()
        prefix = EXECUTABLE_PREFIX if image == binary else LOADER_PREFIX
        rpaths = image_rpaths(image)
        for dep in otool_deps(image):
            if is_system_dependency(dep) or is_internal_dependency(dep):
                continue
            base = Path(dep).name
            extra_dirs = ([Path(p) for p in rpaths if p.startswith("/")]
                          if dep.startswith("@rpath/") else [])
            source = resolve_dependency(dep, staged_names, extra_dirs)
            if source is not None:
                copied = frameworks / base
                shutil.copy2(source, copied)
                staged_names.add(base)
                queue.append(copied)
            rewrites.setdefault(image, []).append((dep, prefix + base))
    for image, pairs in rewrites.items():
        if pairs:
            args = ["install_name_tool"]
            for old, new in pairs:
                args += ["-change", old, new]
            args.append(str(image))
            run(args)
        if image != binary:
            run(["install_name_tool", "-id", LOADER_PREFIX + image.name, str(image)])
    # Ya no queda ninguna dependencia @rpath: elimina los LC_RPATH absolutos
    # (p. ej. de Homebrew) para que el bundle no dependa de su prefijo.
    for image in [binary] + sorted(frameworks.glob("*.dylib")):
        while True:
            foreign = [p for p in image_rpaths(image) if not p.startswith("@")]
            if not foreign:
                break
            run(["install_name_tool", "-delete_rpath", foreign[0], str(image)])
    return staged_names


def verify_bundle_paths(binary, frameworks):
    """Ninguna imagen puede referenciar rutas foráneas (p. ej. Homebrew)."""
    images = [binary] + sorted(frameworks.glob("*.dylib"))
    for image in images:
        for dep in otool_deps(image):
            if is_system_dependency(dep):
                continue
            if dep.startswith("@rpath/"):
                raise SystemExit(
                    f"{image.name}: unresolved @rpath dependency: {dep}")
            if is_internal_dependency(dep):
                continue
            raise SystemExit(
                f"{image.name}: dependency outside the bundle: {dep}")
        for rpath in image_rpaths(image):
            if not rpath.startswith("@"):
                raise SystemExit(
                    f"{image.name}: LC_RPATH outside the bundle: {rpath}")


def sign_ad_hoc(app, binary, frameworks):
    for dylib in sorted(frameworks.glob("*.dylib")):
        run(["codesign", "--force", "--sign", "-", str(dylib)])
    run(["codesign", "--force", "--sign", "-", str(binary)])
    run(["codesign", "--force", "--sign", "-", str(app)])


def smoke_launch(executable, cwd, seconds):
    """Arranque breve: el proceso debe seguir vivo (sin dylibs ausentes)."""
    with tempfile.TemporaryFile() as output:
        proc = subprocess.Popen([str(executable)], cwd=str(cwd),
                                stdout=output, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            code = proc.poll()
            if code is not None:
                output.seek(0)
                detail = output.read().decode("utf-8", "replace")[:4000]
                raise SystemExit(
                    f"Smoke test failed: {executable.name} exited with {code} "
                    f"within {seconds}s (dyld loads are checked here):\n{detail}")
            time.sleep(0.25)
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
    print(f"Smoke test: {executable.name} stayed alive for {seconds}s")


def build_dmg(stage, dist, name, version):
    with tempfile.TemporaryDirectory(prefix="opm-dmg-") as temp:
        root = Path(temp) / "dmg"
        root.mkdir()
        for item in stage.iterdir():
            target = root / item.name
            if item.is_dir():
                shutil.copytree(item, target)
            else:
                shutil.copy2(item, target)
        os.symlink("/Applications", root / "Applications")
        command = ["hdiutil", "create", "-volname", f"opm {version}",
                   "-srcfolder", str(root), "-ov", "-format", "UDZO",
                   str(dist / f"{name}.dmg")]
        # hdiutil puede fallar de forma transitoria con "Resource busy" en
        # runners ocupados: reintentos acotados antes de rendirse.
        for attempt in range(3):
            try:
                run(command)
                break
            except SystemExit:
                if attempt == 2:
                    raise
                time.sleep(2 + 3 * attempt)


def main():
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise SystemExit("This package supports macOS arm64 only")
    version = tomllib.loads((ROOT / "ui/Cargo.toml").read_text())["package"]["version"]
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    binary = target / "release/opm"
    if not binary.is_file():
        raise SystemExit(f"Build the release binary first: {binary}")
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--format-version", "1",
        "--manifest-path", str(ROOT / "ui/Cargo.toml"),
        "--filter-platform", "aarch64-apple-darwin",
    ], text=True))
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    name = f"opm-{version}-macos-aarch64"
    with tempfile.TemporaryDirectory(prefix="opm-package-") as temp:
        stage = Path(temp) / name
        app = stage / "opm.app"
        contents, frameworks, app_binary = bundle_layout(app)
        (contents / "MacOS").mkdir(parents=True)
        frameworks.mkdir()
        (contents / "Resources").mkdir()
        shutil.copy2(binary, app_binary)
        (contents / "Info.plist").write_bytes(
            plistlib.dumps(info_plist(version)))
        collect_and_rewrite(app_binary, frameworks)
        verify_bundle_paths(app_binary, frameworks)
        sign_ad_hoc(app, app_binary, frameworks)
        for filename in ["LICENSE", "NOTICE", "README.md", "BUILDING.md",
                         "CHANGELOG.md"]:
            shutil.copy2(ROOT / filename, stage / filename)
        (stage / "INSTALL.txt").write_text(install_text(version))
        stage_notices(stage, metadata)
        libraries = []
        for image in [app_binary] + sorted(frameworks.glob("*.dylib")):
            libraries.append(f"== {image.name} ==")
            libraries.append(subprocess.check_output(
                ["otool", "-L", str(image)], text=True).rstrip())
        (stage / "shared-libraries.txt").write_text("\n".join(libraries) + "\n")
        smoke_launch(app_binary, stage, SMOKE_SECONDS)

        archive = dist / f"{name}.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(stage, arcname=name)
        build_dmg(stage, dist, name, version)

    artifacts = [dist / f"{name}.tar.gz", dist / f"{name}.dmg"]
    lines = []
    for artifact in artifacts:
        digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
        lines.append(f"{digest}  {artifact.name}\n")
    (dist / "SHA256SUMS").write_text("".join(lines))
    print("Packaged " + ", ".join(artifact.name for artifact in artifacts))


if __name__ == "__main__":
    main()
