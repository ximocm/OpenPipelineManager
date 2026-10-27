#!/usr/bin/env python3
"""Package the Windows x86_64 build: portable ZIP and MSI installer.

GTK4 DLLs are discovered by walking the PE import tables of opm.exe, never by
hardcoded crate names.
"""

import hashlib
import html
import json
import os
from pathlib import Path
import platform
import re
import shutil
import struct
import subprocess
import tempfile
import time
import zipfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent

# Upgrade code generado desde cero para opm: los instaladores de futuras
# versiones actualizan este producto; reutilizar uno ajeno haría que las
# versiones convivan en su lugar.
MSI_UPGRADE_CODE = "{3D70EF32-9A02-4147-8B86-D9D374F23A82}"
# Contratos de API de Windows: los resuelve el propio sistema.
APISET_PREFIXES = ("api-ms-", "ext-ms-")
# Runtime de Visual C++ redistributable: se empaquetan aunque vivan en System32,
# porque una máquina limpia no los trae con el sistema operativo.
REDIST_RE = re.compile(r"^(msvcp|msvcr|vcruntime|concrt|vccorlib)\d", re.IGNORECASE)
GTK_BIN_FALLBACK = Path(r"C:\gtk-build\gtk\x64\release\bin")
SMOKE_SECONDS = 15


def system32_dir():
    return Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32"


def is_system_dll(name, system32):
    """True si el sistema lo aporta y no hay que copiarlo en el paquete."""
    if name.lower().startswith(APISET_PREFIXES):
        return True
    if REDIST_RE.match(name):
        return False
    return (system32 / name).is_file()


def parse_pe_imports(path):
    """DLLs referidas por las tablas de importación (y delay-import) de un PE."""
    data = Path(path).read_bytes()
    if data[:2] != b"MZ":
        raise ValueError(f"{path}: not a PE file")
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    if data[e_lfanew:e_lfanew + 4] != b"PE\0\0":
        raise ValueError(f"{path}: not a PE file")
    coff = e_lfanew + 4
    sections_count = struct.unpack_from("<H", data, coff + 2)[0]
    size_opt = struct.unpack_from("<H", data, coff + 16)[0]
    opt = coff + 20
    magic = struct.unpack_from("<H", data, opt)[0]
    if magic != 0x20B:
        raise ValueError(f"{path}: expected a 64-bit PE (magic {magic:#x})")
    data_dirs = opt + 112
    rva_count = struct.unpack_from("<I", data, opt + 108)[0]
    sections = []
    header = opt + size_opt
    for i in range(sections_count):
        off = header + i * 40
        virtual_size, virtual_addr, raw_size, raw_ptr = struct.unpack_from(
            "<IIII", data, off + 8)
        sections.append((virtual_addr, max(virtual_size, raw_size), raw_ptr))

    def rva_to_offset(rva):
        for virtual_addr, span, raw_ptr in sections:
            if virtual_addr <= rva < virtual_addr + span:
                return rva - virtual_addr + raw_ptr
        raise ValueError(f"{path}: RVA {rva:#x} outside of any section")

    def read_cstring(offset):
        end = data.index(b"\0", offset)
        return data[offset:end].decode("ascii", "replace")

    names = []
    if rva_count > 1:
        import_rva = struct.unpack_from("<I", data, data_dirs + 8)[0]
        if import_rva:
            offset = rva_to_offset(import_rva)
            while True:
                original_first, _, _, name_rva, first = struct.unpack_from(
                    "<IIIII", data, offset)
                if original_first == 0 and name_rva == 0 and first == 0:
                    break
                if name_rva:
                    names.append(read_cstring(rva_to_offset(name_rva)))
                offset += 20
    if rva_count > 13:
        delay_rva = struct.unpack_from("<I", data, data_dirs + 13 * 8)[0]
        if delay_rva:
            offset = rva_to_offset(delay_rva)
            while True:
                attributes, name_rva = struct.unpack_from("<II", data, offset)
                if attributes == 0 and name_rva == 0:
                    break
                if name_rva:
                    names.append(read_cstring(rva_to_offset(name_rva)))
                offset += 32
    return names


def resolve_dll(name, dirs):
    for directory in dirs:
        candidate = Path(directory) / name
        if candidate.is_file():
            return candidate
    return None


def path_dirs():
    return [Path(p.strip('"')) for p in os.environ.get("PATH", "").split(os.pathsep) if p]


def vc_redist_dirs():
    dirs = []
    redist = os.environ.get("VCToolsRedistDir")
    if redist:
        dirs.extend(sorted(Path(redist).glob("x64/Microsoft.VC*.CRT")))
    for base in filter(None, (os.environ.get("ProgramFiles"),
                              os.environ.get("ProgramFiles(x86)"))):
        root = Path(base) / "Microsoft Visual Studio" / "2022"
        dirs.extend(sorted(root.glob("*/VC/Redist/MSVC/*/x64/Microsoft.VC*.CRT")))
    return dirs


def collect_dlls(exe):
    """Cierre de DLLs no-sistema que opm.exe necesita para arrancar."""
    system32 = system32_dir()
    search = [exe.parent] + path_dirs() + [GTK_BIN_FALLBACK]
    staged = {}
    missing = []
    visited = {exe.name.lower()}
    queue = [exe]

    def treat(name):
        key = name.lower()
        if key in visited:
            return
        visited.add(key)
        if is_system_dll(name, system32):
            return
        source = resolve_dll(name, search)
        if source is None and REDIST_RE.match(name):
            source = resolve_dll(name, vc_redist_dirs() + [system32])
        if source is None:
            missing.append(name)
            return
        staged[name] = source
        queue.append(source)

    # DLLs colocadas junto al ejecutable por scripts de build también viajan
    # en el paquete.
    for extra in sorted(exe.parent.glob("*.dll")):
        treat(extra.name)
    while queue:
        for name in parse_pe_imports(queue.pop()):
            treat(name)
    if missing:
        raise SystemExit("Missing shared libraries:\n" + "\n".join(sorted(set(missing))))
    return staged


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
        f"opm {version} for Windows x86-64, built on Windows Server 2022\n"
        "with the MSVC toolchain and GTK 4 from gvsbuild.\n"
        "\n"
        "Requirements: Windows 10 or 11 on x86-64.\n"
        "\n"
        "Running pipelines requires a POSIX shell; Git for Windows provides\n"
        "bash. Without it the app refuses to run steps with a clear error.\n"
        "\n"
        f"Portable ZIP (opm-{version}-windows-x86_64.zip): unzip this archive\n"
        "anywhere and run opm.exe. The GTK 4 DLLs it needs are included in\n"
        "the same directory.\n"
        "\n"
        f"MSI (opm-{version}-windows-x86_64.msi): installs opm for all\n"
        "users under Program Files with a Start Menu shortcut.\n"
        "\n"
        "Privacy: opm adds no telemetry and no network access of its own\n"
        "(README.md).\n"
    )


def msi_wxs(version, stage):
    include = html.escape(str(stage) + "\\**", quote=True)
    return f"""<?xml version="1.0" encoding="utf-8"?>
<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
    <Package Name="opm" Manufacturer="Ximo" Version="{version}"
             UpgradeCode="{MSI_UPGRADE_CODE}" Scope="perMachine">
        <MajorUpgrade DowngradeErrorMessage="Ya hay una version mas reciente de opm instalada." />
        <MediaTemplate EmbedCab="yes" />
        <StandardDirectory Id="ProgramFiles64Folder">
            <Directory Id="INSTALLFOLDER" Name="opm" />
        </StandardDirectory>
        <StandardDirectory Id="ProgramMenuFolder" />
        <Feature Id="Main" Title="opm" Level="1">
            <Files Include="{include}" Directory="INSTALLFOLDER" />
            <Component Id="StartMenuShortcut" Directory="ProgramMenuFolder">
                <Shortcut Id="OpmShortcut" Name="opm"
                          Description="Gestor de pipelines opm"
                          Target="[INSTALLFOLDER]opm.exe"
                          WorkingDirectory="INSTALLFOLDER" />
                <RemoveFolder Id="RemoveStartMenuShortcut" On="uninstall" />
                <RegistryValue Root="HKLM" Key="Software\\opm"
                               Name="start-menu-shortcut" Type="integer"
                               Value="1" KeyPath="yes" />
            </Component>
        </Feature>
    </Package>
</Wix>
"""


def smoke_launch(exe, cwd, seconds):
    """Arranque breve: el proceso debe seguir vivo (sin DLLs ausentes)."""
    with tempfile.TemporaryFile() as output:
        proc = subprocess.Popen([str(exe)], cwd=str(cwd),
                                stdout=output, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            code = proc.poll()
            if code is not None:
                output.seek(0)
                detail = output.read().decode("utf-8", "replace")[:4000]
                if code & 0xFFFFFFFF == 0xC0000135:
                    raise SystemExit(
                        f"Smoke test failed: {exe.name} exited with 0xC0000135 "
                        f"(ERROR_MOD_NOT_FOUND, missing DLL):\n{detail}")
                raise SystemExit(
                    f"Smoke test failed: {exe.name} exited with {code} "
                    f"within {seconds}s:\n{detail}")
            time.sleep(0.25)
        subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                       capture_output=True)
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
    print(f"Smoke test: {exe.name} stayed alive for {seconds}s")


def find_wix():
    wix = shutil.which("wix") or shutil.which("wix.exe")
    if not wix:
        raise SystemExit(
            "WiX Toolset v6 not found. Install it with:\n"
            "  dotnet tool install --global wix --version 6.0.2")
    return wix


# El formato ZIP no admite fechas anteriores a 1980 y el stage puede traerlas:
# los LICENSE de crates.io llegan con mtimes de 1970/1973 (se copian con
# `shutil.copy2`, que conserva la fecha de origen).
ZIP_FECHA_MINIMA = (1980, 1, 1, 0, 0, 0)


def zip_member(path, arcname):
    """(ZipInfo, contenido) de un archivo del stage, con la fecha acotada."""
    st = path.stat()
    try:
        fecha = time.localtime(st.st_mtime)[:6]
    except (OSError, ValueError, OverflowError):
        fecha = ZIP_FECHA_MINIMA
    if fecha < ZIP_FECHA_MINIMA:
        print(f"Warning: {arcname} has timestamp {fecha}; using 1980-01-01")
        fecha = ZIP_FECHA_MINIMA
    info = zipfile.ZipInfo(arcname, fecha)
    info.compress_type = zipfile.ZIP_DEFLATED
    info.external_attr = (st.st_mode & 0xFFFF) << 16
    return info, path.read_bytes()


def wix_build(wix, args, cwd):
    result = subprocess.run([wix, "build", *args], cwd=str(cwd),
                            capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(
            f"wix build failed ({result.returncode}):\n"
            f"{result.stdout}\n{result.stderr}")


def main():
    if platform.system() != "Windows" or platform.machine() != "AMD64":
        raise SystemExit("This package supports Windows x86_64 only")
    version = tomllib.loads((ROOT / "ui/Cargo.toml").read_text())["package"]["version"]
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    binary = target / "release/opm.exe"
    if not binary.is_file():
        raise SystemExit(f"Build the release binary first: {binary}")
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--format-version", "1",
        "--manifest-path", str(ROOT / "ui/Cargo.toml"),
        "--filter-platform", "x86_64-pc-windows-msvc",
    ], text=True))
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    name = f"opm-{version}-windows-x86_64"
    with tempfile.TemporaryDirectory(prefix="opm-package-") as temp:
        stage = Path(temp) / name
        stage.mkdir()
        shutil.copy2(binary, stage / "opm.exe")
        dlls = collect_dlls(binary)
        for dll_name, source in sorted(dlls.items(), key=lambda item: item[0].lower()):
            shutil.copy2(source, stage / dll_name)
        for filename in ["LICENSE", "NOTICE", "README.md", "BUILDING.md",
                         "CHANGELOG.md"]:
            shutil.copy2(ROOT / filename, stage / filename)
        (stage / "INSTALL.txt").write_text(install_text(version))
        stage_notices(stage, metadata)
        (stage / "shared-libraries.txt").write_text(
            "".join(f"{dll_name}\t{source}\n"
                    for dll_name, source in
                    sorted(dlls.items(), key=lambda item: item[0].lower())))
        smoke_launch(stage / "opm.exe", stage, SMOKE_SECONDS)

        archive = dist / f"{name}.zip"
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zipped:
            for path in sorted(stage.rglob("*")):
                if path.is_file():
                    info, data = zip_member(
                        path, f"{name}/{path.relative_to(stage).as_posix()}")
                    zipped.writestr(info, data)

        wix = find_wix()
        wix_dir = Path(temp) / "wix"
        wix_dir.mkdir()
        product = wix_dir / "product.wxs"
        product.write_text(msi_wxs(version, stage), encoding="utf-8")
        msi = wix_dir / f"{name}.msi"
        wix_build(wix, ["-arch", "x64", "-o", str(msi), str(product)], wix_dir)
        shutil.copy2(msi, dist / msi.name)

    artifacts = [dist / f"{name}.zip", dist / f"{name}.msi"]
    lines = []
    for artifact in artifacts:
        digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
        lines.append(f"{digest}  {artifact.name}\n")
    (dist / "SHA256SUMS").write_text("".join(lines))
    print("Packaged " + ", ".join(artifact.name for artifact in artifacts))


if __name__ == "__main__":
    main()
