"""Regression tests for release validation and package contents (no Rust build)."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from unittest.mock import patch
import zipfile
import plistlib
from xml.dom import minidom

SCRIPTS = Path(__file__).resolve().parent


def manifest(root, version="0.2.0"):
    (root / "core").mkdir(exist_ok=True)
    (root / "ui").mkdir(exist_ok=True)
    (root / "core/Cargo.toml").write_text(f'[workspace.package]\nversion = "{version}"\n')
    (root / "ui/Cargo.toml").write_text(f'[package]\nversion = "{version}"\n')


class ReleaseValidation(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        manifest(self.root)
        self.git("init", "-b", "main")
        self.git("config", "user.name", "Release Test")
        self.git("config", "user.email", "test@example.invalid")
        self.git("add", ".")
        self.git("commit", "-m", "fixture")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")
        self.git("tag", "v0.2.0")

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, stderr=subprocess.DEVNULL)

    def validate(self, tag):
        return subprocess.run(
            [sys.executable, str(SCRIPTS / "validate-release.py"), tag],
            cwd=self.root, env={**os.environ, "GITHUB_OUTPUT": str(self.root / "output")},
            text=True, capture_output=True,
        )

    def test_matching_tag(self):
        result = self.validate("v0.2.0")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("tag=v0.2.0\nsha=", (self.root / "output").read_text())

    def test_rejects_invalid_tag(self):
        self.assertNotEqual(self.validate("v0.2.0; echo injected").returncode, 0)

    def test_rejects_version_mismatch(self):
        self.git("tag", "v2.0.0")
        self.assertNotEqual(self.validate("v2.0.0").returncode, 0)

    def test_rejects_wrong_checkout(self):
        self.git("commit", "--allow-empty", "-m", "new head")
        self.assertNotEqual(self.validate("v0.2.0").returncode, 0)

    def test_rejects_commit_outside_main(self):
        self.git("commit", "--allow-empty", "-m", "unmerged")
        self.git("tag", "v0.2.1")
        manifest(self.root, "0.2.1")
        self.assertNotEqual(self.validate("v0.2.1").returncode, 0)


class LinuxPackage(unittest.TestCase):
    def test_archive_contains_binary_notices_and_matching_checksum(self):
        spec = importlib.util.spec_from_file_location("package_linux", SCRIPTS / "package-linux.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            manifest(root)
            target = root / "target/release"
            target.mkdir(parents=True)
            (target / "opm").write_bytes(b"test executable")
            (target / "opm").chmod(0o755)
            for filename in ["LICENSE", "NOTICE", "README.md", "BUILDING.md", "CHANGELOG.md"]:
                (root / filename).write_text(filename)
            dep = root / "dependency"
            dep.mkdir()
            (dep / "NOTICE").write_text("dependency license and copyright")
            metadata = {"packages": [{"name": "dependency", "version": "1.0.0",
                        "source": "registry", "manifest_path": str(dep / "Cargo.toml"),
                        "license": "MIT"}]}
            provenance_fixture(root)
            with patch.object(module, "ROOT", root), \
                 patch.dict(os.environ, clear=True), \
                 patch.object(module.platform, "system", return_value="Linux"), \
                 patch.object(module.platform, "machine", return_value="x86_64"), \
                 patch.object(module.subprocess, "check_output", side_effect=["libc found\n", json.dumps(metadata)]):
                module.main()
            archive = root / "dist/opm-0.2.0-linux-x86_64.tar.gz"
            self.assertEqual((root / "dist/SHA256SUMS").read_text(),
                             f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n")
            with tarfile.open(archive) as tar:
                prefix = "opm-0.2.0-linux-x86_64/"
                self.assertEqual(tar.getmember(prefix + "opm").mode & 0o111, 0o111)
                self.assertEqual(tar.extractfile(prefix + "third-party/dependency-1.0.0/NOTICE").read(),
                                 b"dependency license and copyright")
                self.assertIn(prefix + "INSTALL.txt", tar.getnames())
                self.assertIn(prefix + "third-party/inventory.json", tar.getnames())
                self.assertEqual(
                    tar.extractfile(prefix + "third-party/minmark-Apache-2.0.txt")
                        .read().decode(),
                    PROVENANCE_TEXT)
                install = tar.extractfile(prefix + "INSTALL.txt").read().decode()
                self.assertIn("GTK >= 4.10", install)
                self.assertNotIn("WebKitGTK", install)
                self.assertIn(prefix + "shared-libraries.txt", tar.getnames())


def load_script(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


PROVENANCE_TEXT = "Apache License\nVersion 2.0, January 2004\n"


def provenance_fixture(root):
    """Fichero de procedencia Apache-2.0 que los empaquetadores copian."""
    (root / "third-party").mkdir(exist_ok=True)
    (root / "third-party/minmark-Apache-2.0.txt").write_text(PROVENANCE_TEXT)


def notices_fixture(root):
    """Dependencias falsas (con licencias) y su metadata de cargo."""
    dep = root / "dependency"
    dep.mkdir()
    (dep / "LICENSE").write_text("dependency license")
    (dep / "NOTICE").write_text("dependency notices")
    metadata = {"packages": [
        {"name": "dependency", "version": "1.0.0",
         "source": "registry+https://example.invalid",
         "manifest_path": str(dep / "Cargo.toml"), "license": "MIT",
         "repository": "https://example.invalid/dependency"},
        {"name": "opm-ui", "version": "0.2.0", "source": None,
         "manifest_path": str(root / "ui" / "Cargo.toml"),
         "license": "LicenseRef-Attribution-NonCommercial"},
    ]}
    return metadata


def minimal_pe():
    """PE x64 mínimo con una tabla de importaciones y otra de delay-import."""
    e_lfanew = 0x80
    coff = e_lfanew + 4
    opt = coff + 20
    size_opt = 240
    header = opt + size_opt
    raw_ptr, raw_size, va = 0x200, 0x400, 0x1000
    data = bytearray(0x600)
    data[0:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, e_lfanew)
    data[e_lfanew:e_lfanew + 4] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", data, coff, 0x8664, 1, 0, 0, 0, size_opt, 0x22)
    struct.pack_into("<H", data, opt, 0x20B)
    struct.pack_into("<I", data, opt + 108, 16)
    struct.pack_into("<II", data, opt + 120, va, 0x60)
    struct.pack_into("<II", data, opt + 112 + 13 * 8, va + 0x40, 0x40)
    data[header:header + 8] = b".rdata\0\0"
    struct.pack_into("<IIII", data, header + 8, raw_size, va, raw_size, raw_ptr)
    struct.pack_into("<IIIII", data, 0x200, 0, 0, 0, 0x1100, 0)
    struct.pack_into("<IIIII", data, 0x214, 0, 0, 0, 0x1110, 0)
    struct.pack_into("<IIIIIIII", data, 0x240, 1, 0x1120, 0, 0, 0, 0, 0, 0)
    data[0x300:0x30D] = b"TESTDLL1.dll\0"
    data[0x310:0x31D] = b"TESTDLL2.dll\0"
    data[0x320:0x32B] = b"DELAY1.dll\0"
    return bytes(data)


class PeImportParsing(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.module = load_script("package_windows", "package-windows.py")

    def test_reads_import_and_delay_import_tables(self):
        with tempfile.TemporaryDirectory() as temp:
            pe = Path(temp) / "fake.exe"
            pe.write_bytes(minimal_pe())
            self.assertEqual(set(self.module.parse_pe_imports(pe)),
                             {"TESTDLL1.dll", "TESTDLL2.dll", "DELAY1.dll"})

    def test_rejects_non_pe_file(self):
        with tempfile.TemporaryDirectory() as temp:
            plain = Path(temp) / "plain.bin"
            plain.write_bytes(b"not an executable")
            with self.assertRaises(ValueError):
                self.module.parse_pe_imports(plain)


class WindowsPackage(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.module = load_script("package_windows", "package-windows.py")

    def test_msi_wxs_is_valid_xml_with_expected_authoring(self):
        source = self.module.msi_wxs(
            "0.2.0", Path(r"C:\stage\opm-0.2.0-windows-x86_64"))
        minidom.parseString(source)
        self.assertIn(f'UpgradeCode="{self.module.MSI_UPGRADE_CODE}"', source)
        # El UpgradeCode es fresco y propio: jamás se reutiliza el de la
        # plantilla original.
        self.assertNotEqual(self.module.MSI_UPGRADE_CODE,
                            "{39C00825-AAA6-5B48-9A1E-76E2B66F043C}")
        self.assertRegex(self.module.MSI_UPGRADE_CODE,
                         r"^\{[0-9A-F]{8}(?:-[0-9A-F]{4}){3}-[0-9A-F]{12}\}$")
        self.assertIn('<Package Name="opm"', source)
        self.assertIn('Scope="perMachine"', source)
        self.assertIn('Include="C:\\stage\\opm-0.2.0-windows-x86_64\\**"', source)
        self.assertIn('Target="[INSTALLFOLDER]opm.exe"', source)
        self.assertIn('<MediaTemplate EmbedCab="yes" />', source)
        self.assertIn('Key="Software\\opm"', source)
        self.assertIn('<Shortcut Id="OpmShortcut" Name="opm"', source)

    def test_system_dll_classification(self):
        with tempfile.TemporaryDirectory() as temp:
            system32 = Path(temp)
            (system32 / "kernel32.dll").write_bytes(b"")
            (system32 / "vcruntime140.dll").write_bytes(b"")
            self.assertTrue(self.module.is_system_dll("kernel32.dll", system32))
            self.assertTrue(
                self.module.is_system_dll("api-ms-win-core-l1-1-0.dll", system32))
            # El runtime de VC++ se empaqueta aunque exista en System32.
            self.assertFalse(self.module.is_system_dll("vcruntime140.dll", system32))
            self.assertFalse(self.module.is_system_dll("libgtk-4-1.dll", system32))

    def test_install_text_documents_requirements(self):
        text = self.module.install_text("0.2.0")
        self.assertIn("Windows 10 or 11", text)
        self.assertIn("no telemetry", text)
        self.assertIn("Start Menu shortcut", text)
        self.assertIn("opm-0.2.0-windows-x86_64.zip", text)
        self.assertIn("opm-0.2.0-windows-x86_64.msi", text)
        # El quoting de un solo argv es POSIX: sin shell el motor se niega.
        self.assertIn("Running pipelines requires a POSIX shell", text)
        self.assertIn("Git for Windows provides\nbash", text)
        self.assertIn("refuses to run steps with a clear error", text)

    def test_stage_notices_inventory(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            metadata = notices_fixture(root)
            provenance_fixture(root)
            stage = root / "stage"
            stage.mkdir()
            with patch.object(self.module, "ROOT", root):
                self.module.stage_notices(stage, metadata)
            self.assertEqual(
                (stage / "third-party/dependency-1.0.0/LICENSE").read_text(),
                "dependency license")
            self.assertEqual(
                (stage / "third-party/dependency-1.0.0/NOTICE").read_text(),
                "dependency notices")
            inventory = json.loads(
                (stage / "third-party/inventory.json").read_text())
            self.assertEqual([entry["name"] for entry in inventory],
                             ["dependency"])
            self.assertEqual(
                (stage / "third-party/minmark-Apache-2.0.txt").read_text(),
                PROVENANCE_TEXT)

    def test_stage_notices_succeeds_without_notice_files(self):
        # El inventario es obligatorio; los NOTICE se copian si existen, pero
        # su ausencia no bloquea el empaquetado. La procedencia Apache-2.0
        # se copia siempre.
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            dep = root / "dependency"
            dep.mkdir()
            (dep / "LICENSE").write_text("dependency license")
            provenance_fixture(root)
            metadata = {"packages": [
                {"name": "dependency", "version": "1.0.0",
                 "source": "registry+https://example.invalid",
                 "manifest_path": str(dep / "Cargo.toml"), "license": "MIT"},
            ]}
            stage = root / "stage"
            stage.mkdir()
            with patch.object(self.module, "ROOT", root):
                self.module.stage_notices(stage, metadata)
            inventory = json.loads(
                (stage / "third-party/inventory.json").read_text())
            self.assertEqual([entry["name"] for entry in inventory],
                             ["dependency"])
            self.assertEqual(
                (stage / "third-party/minmark-Apache-2.0.txt").read_text(),
                PROVENANCE_TEXT)

    def test_zip_member_clamps_pre_1980_timestamps(self):
        # El formato ZIP no admite fechas anteriores a 1980 y los LICENSE del
        # registro de crates las traen: se acotan sin romper el empaquetado.
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "raro.dll"
            path.write_bytes(b"contenido")
            os.utime(path, (0, 0))
            info, data = self.module.zip_member(path, "pkg/raro.dll")
        self.assertEqual(info.date_time, (1980, 1, 1, 0, 0, 0))
        self.assertEqual(info.compress_type, zipfile.ZIP_DEFLATED)
        self.assertEqual(data, b"contenido")

    def test_zip_member_keeps_current_timestamps(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "normal.dll"
            path.write_bytes(b"ok")
            info, _ = self.module.zip_member(path, "pkg/normal.dll")
            self.assertEqual(info.date_time,
                             time.localtime(path.stat().st_mtime)[:6])


class MacosPackage(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.module = load_script("package_macos", "package-macos.py")

    def test_parse_otool_deps(self):
        text = ("opm.o:\n"
                "\t/opt/homebrew/opt/gtk4/lib/libgtk-4.1.dylib "
                "(compat version 804.0.0, current version 804.0.0)\n"
                "\t/usr/lib/libSystem.B.dylib "
                "(compat version 1.0.0, current version 1400.0.0)\n"
                "\t@executable_path/../Frameworks/libglib-2.0-0.dylib "
                "(compat version 0.0.0, current version 0.0.0)\n")
        self.assertEqual(self.module.parse_otool_deps(text), [
            "/opt/homebrew/opt/gtk4/lib/libgtk-4.1.dylib",
            "/usr/lib/libSystem.B.dylib",
            "@executable_path/../Frameworks/libglib-2.0-0.dylib",
        ])

    def test_rewrite_plan_keeps_system_paths(self):
        deps = ["/opt/homebrew/opt/gtk4/lib/libgtk-4.1.dylib",
                "/usr/lib/libSystem.B.dylib",
                "/System/Library/Frameworks/Foundation.framework/Foundation",
                "@executable_path/../Frameworks/libyaud.dylib",
                "@rpath/libfoo.dylib"]
        self.assertEqual(
            self.module.rewrite_plan(deps, "@executable_path/../Frameworks/"),
            [("/opt/homebrew/opt/gtk4/lib/libgtk-4.1.dylib",
              "@executable_path/../Frameworks/libgtk-4.1.dylib"),
             ("@rpath/libfoo.dylib",
              "@executable_path/../Frameworks/libfoo.dylib")])
        self.assertEqual(
            self.module.rewrite_plan(deps, "@loader_path/"),
            [("/opt/homebrew/opt/gtk4/lib/libgtk-4.1.dylib",
              "@loader_path/libgtk-4.1.dylib"),
             ("@rpath/libfoo.dylib", "@loader_path/libfoo.dylib")])

    def test_parse_otool_rpaths(self):
        text = """\
opm:
Load command 8
      cmd LC_RPATH
  cmdsize 48
         path /opt/homebrew/lib (offset 12)
Load command 9
      cmd LC_LOAD_DYLIB
  cmdsize 56
         name /usr/lib/libSystem.B.dylib (offset 24)
"""
        self.assertEqual(self.module.parse_otool_rpaths(text),
                         ["/opt/homebrew/lib"])

    def test_info_plist_round_trips(self):
        plist = self.module.info_plist("0.2.0")
        self.assertEqual(plist["CFBundleExecutable"], "opm")
        self.assertEqual(plist["CFBundleIdentifier"], "app.opm")
        self.assertEqual(plist["CFBundleShortVersionString"], "0.2.0")
        restored = plistlib.loads(plistlib.dumps(plist))
        self.assertEqual(restored, plist)

    def test_install_text_notes_ad_hoc_signing(self):
        text = self.module.install_text("0.2.0")
        self.assertIn("Apple Silicon", text)
        self.assertIn("ad-hoc signed in CI", text)
        self.assertIn("Developer ID signing and notarization", text)
        self.assertIn("Gatekeeper", text)
        self.assertIn("opm.app/Contents/Frameworks", text)

    def test_stage_notices_inventory(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            metadata = notices_fixture(root)
            provenance_fixture(root)
            stage = root / "stage"
            stage.mkdir()
            with patch.object(self.module, "ROOT", root):
                self.module.stage_notices(stage, metadata)
            self.assertEqual(
                (stage / "third-party/dependency-1.0.0/NOTICE").read_text(),
                "dependency notices")
            inventory = json.loads(
                (stage / "third-party/inventory.json").read_text())
            self.assertEqual([entry["name"] for entry in inventory],
                             ["dependency"])
            self.assertEqual(
                (stage / "third-party/minmark-Apache-2.0.txt").read_text(),
                PROVENANCE_TEXT)

    def test_bundle_layout_matches_executable_prefix(self):
        # Las dylibs deben quedar donde resuelve el install name reescrito
        # (@executable_path/../Frameworks desde Contents/MacOS) y donde dice
        # INSTALL.txt: opm.app/Contents/Frameworks.
        app = Path("/tmp/package/opm.app")
        contents, frameworks, binary = self.module.bundle_layout(app)
        self.assertEqual(frameworks, app / "Contents" / "Frameworks")
        self.assertEqual(contents, app / "Contents")
        resolved = Path(os.path.normpath(
            binary.parent /
            self.module.EXECUTABLE_PREFIX[len("@executable_path/"):]))
        self.assertEqual(resolved, frameworks)


if __name__ == "__main__":
    unittest.main()
