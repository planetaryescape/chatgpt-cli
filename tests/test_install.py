"""Exercise the POSIX installer without a network or a real chatgpt binary."""

import hashlib
import io
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest


INSTALLER = Path(__file__).resolve().parents[1] / "install.sh"
VERSION = "9.8.7"
BINARY = b"""#!/bin/sh
[ "$1" = --version ] || exit 64
echo ran >> "$EXECUTION_LOG"
echo 'chatgpt 9.8.7'
"""


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="chatgpt-installer-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.home = self.root / "home"
        self.home.mkdir()
        self.tmp = self.root / "tmp"
        self.tmp.mkdir()
        self.prefix = self.root / "install prefix;literal"
        self.installed = self.prefix / "bin/chatgpt"
        self.execution_log = self.root / "executions"
        self.download_log = self.root / "downloads"
        self.env = {
            "HOME": str(self.home),
            "PATH": str(self.tools),
            "TMPDIR": str(self.tmp),
            "EXECUTION_LOG": str(self.execution_log),
            "DOWNLOAD_LOG": str(self.download_log),
            "FIXTURE_RELEASE": str(self.root),
            "FAKE_OS": "Linux",
            "FAKE_ARCH": "x86_64",
        }
        # Only these tools can run. In particular, curl can never reach GitHub,
        # and missing-checker tests cannot find one elsewhere on the host.
        for tool in [
            "tar",
            "gzip",
            "mkdir",
            "mktemp",
            "rm",
            "cp",
            "basename",
            "install",
            "dirname",
            "readlink",
            "cat",
        ]:
            real = shutil.which(tool)
            self.assertIsNotNone(real, f"installer test needs {tool}")
            (self.tools / tool).symlink_to(real)
        self.script(
            "uname",
            """case "$1" in
  -s) echo "$FAKE_OS" ;;
  -m) echo "$FAKE_ARCH" ;;
  *) exit 64 ;;
esac
""",
        )
        self.checker("sha256sum")
        self.script(
            "curl",
            f"""#!{sys.executable}
import os
from pathlib import Path
import shutil
import sys

args = sys.argv[1:]
url = args[-1]
with open(os.environ["DOWNLOAD_LOG"], "a") as log:
    log.write(url + "\\n")
if url == "https://github.com/planetaryescape/chatgpt-cli/releases/latest":
    print("https://github.com/planetaryescape/chatgpt-cli/releases/tag/v9.8.7", end="")
else:
    name = url.rsplit("/", 1)[1]
    expected = os.environ["EXPECTED_ARCHIVE"]
    assert name in [expected, expected + ".sha256"], name
    assert url == "https://github.com/planetaryescape/chatgpt-cli/releases/download/v9.8.7/" + name, url
    shutil.copyfile(Path(os.environ["FIXTURE_RELEASE"]) / name, args[args.index("-o") + 1])
""",
        )

    def script(self, name, content):
        path = self.tools / name
        path.write_text(
            content if content.startswith("#!") else "#!/bin/sh\nset -eu\n" + content
        )
        path.chmod(0o755)

    def checker(self, name):
        real = shutil.which(name)
        self.assertIsNotNone(real, f"installer test needs real {name}")
        self.script(
            name, f'echo {name} >> "$HOME/checkers"\nexec {shlex.quote(real)} "$@"\n'
        )

    def archive(
        self, name="chatgpt-v9.8.7-linux-x86_64.tar.gz", binary=BINARY, mode=0o755
    ):
        archive = self.root / name
        with tarfile.open(archive, "w:gz", format=tarfile.USTAR_FORMAT) as tar:
            entry = tarfile.TarInfo(
                "./chatgpt" if binary is not None else "./README.md"
            )
            content = binary if binary is not None else b"no binary here\n"
            entry.size = len(content)
            entry.mode = mode
            tar.addfile(entry, io.BytesIO(content))
        self.checksum(archive)
        return archive

    def checksum(self, archive):
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        Path(str(archive) + ".sha256").write_text(f"{digest}  {archive.name}\n")

    def run_installer(self, *args):
        return subprocess.run(
            ["/bin/sh", str(INSTALLER), *map(str, args)],
            cwd=self.root,
            env=self.env,
            text=True,
            capture_output=True,
            timeout=15,
        )

    def assert_installed(self, result, prefix=None):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        installed = (prefix or self.prefix) / "bin/chatgpt"
        self.assertEqual(installed.read_bytes(), BINARY)
        self.assertEqual(installed.stat().st_mode & 0o777, 0o755)
        self.assertIn(f"installed chatgpt {VERSION} to {installed}", result.stdout)
        version = subprocess.run(
            [str(installed), "--version"],
            env=self.env,
            text=True,
            capture_output=True,
            timeout=5,
        )
        self.assertEqual(
            (version.returncode, version.stdout), (0, f"chatgpt {VERSION}\n")
        )
        self.assertEqual(
            list(self.tmp.iterdir()), [], "installer must clean its scratch directory"
        )

    def assert_unchanged(self, result):
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.installed.read_bytes(), b"existing installation\n")
        self.assertFalse(
            self.execution_log.exists(), "unverified binary must never run"
        )
        self.assertEqual(list(self.tmp.iterdir()), [])

    def existing_install(self):
        self.installed.parent.mkdir(parents=True)
        self.installed.write_bytes(b"existing installation\n")

    def test_download_selects_linux_and_both_existing_macos_archive_names(self):
        for os_name, arch, archive in [
            ("Linux", "x86_64", "chatgpt-v9.8.7-linux-x86_64.tar.gz"),
            ("Darwin", "arm64", "chatgpt-v9.8.7-macos-aarch64.tar.gz"),
            ("Darwin", "x86_64", "chatgpt-v9.8.7-macos-x86_64.tar.gz"),
        ]:
            with self.subTest(os=os_name, arch=arch):
                self.installed.unlink(missing_ok=True)
                self.archive(archive)
                self.env.update(
                    FAKE_OS=os_name, FAKE_ARCH=arch, EXPECTED_ARCHIVE=archive
                )
                self.download_log.unlink(missing_ok=True)
                result = self.run_installer(
                    "--version", VERSION, "--prefix", self.prefix
                )
                self.assert_installed(result)
                base = "https://github.com/planetaryescape/chatgpt-cli/releases/download/v9.8.7/"
                self.assertEqual(
                    self.download_log.read_text().splitlines(),
                    [base + archive, base + archive + ".sha256"],
                )

    def test_latest_release_and_default_prefix(self):
        self.archive()
        self.env["EXPECTED_ARCHIVE"] = "chatgpt-v9.8.7-linux-x86_64.tar.gz"
        self.assert_installed(self.run_installer(), self.home / ".local")
        self.assertEqual(len(self.download_log.read_text().splitlines()), 3)

    def test_local_archive_with_spaces_and_relative_option_like_paths(self):
        archive = self.archive("-local archive.tar.gz")
        self.assert_installed(
            self.run_installer("--archive", archive.name, "--prefix", "-local prefix"),
            self.root / "-local prefix",
        )
        self.assertFalse(self.download_log.exists())

    def test_environment_version_and_prefix(self):
        self.archive()
        self.env.update(
            CHATGPT_VERSION="v9.8.7",
            CHATGPT_INSTALL_PREFIX=str(self.prefix),
            EXPECTED_ARCHIVE="chatgpt-v9.8.7-linux-x86_64.tar.gz",
        )
        self.assert_installed(self.run_installer())

    def test_shasum_fallback_and_sha256sum_preference(self):
        self.checker("shasum")
        archive = self.archive()
        for expected in ["sha256sum", "shasum"]:
            with self.subTest(checker=expected):
                self.installed.unlink(missing_ok=True)
                (self.home / "checkers").unlink(missing_ok=True)
                self.assert_installed(
                    self.run_installer("--archive", archive, "--prefix", self.prefix)
                )
                self.assertEqual((self.home / "checkers").read_text(), expected + "\n")
                if expected == "sha256sum":
                    (self.tools / "sha256sum").unlink()

    def test_missing_checker_fails_without_installing_or_downloading(self):
        (self.tools / "sha256sum").unlink()
        self.existing_install()
        result = self.run_installer("--prefix", self.prefix)
        self.assertEqual(result.returncode, 69)
        self.assertIn("sha256sum or shasum is required", result.stderr)
        self.assert_unchanged(result)
        self.assertFalse(self.download_log.exists())

    def test_checksum_failure_never_clobbers_or_runs_existing_install(self):
        archive = self.archive()
        self.existing_install()
        (self.tools / "tar").unlink()
        self.script("tar", 'echo ran > "$HOME/tar-ran"\nexit 1\n')
        for checker in ["sha256sum", "shasum"]:
            with self.subTest(checker=checker):
                Path(str(archive) + ".sha256").write_text(
                    f"{'0' * 64}  {archive.name}\n"
                )
                result = self.run_installer(
                    "--archive", archive, "--prefix", self.prefix
                )
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assert_unchanged(result)
                self.assertFalse(
                    (self.home / "tar-ran").exists(), "checksum must precede extraction"
                )
                if checker == "sha256sum":
                    (self.tools / "sha256sum").unlink()
                    self.checker("shasum")

    def test_empty_checksum_and_checksum_for_wrong_archive_are_rejected(self):
        archive = self.archive()
        self.existing_install()
        for checksum in ["", "  \n", f"{'0' * 64}  wrong-archive.tar.gz\n"]:
            with self.subTest(checksum=checksum):
                self.installed.write_bytes(b"existing installation\n")
                Path(str(archive) + ".sha256").write_text(checksum)
                result = self.run_installer(
                    "--archive", archive, "--prefix", self.prefix
                )
                self.assertEqual(result.returncode, 65, result.stdout + result.stderr)
                self.assert_unchanged(result)

    def test_missing_archive_and_checksum_keep_exit_66(self):
        archive = self.root / "missing.tar.gz"
        self.existing_install()
        for exists in [False, True]:
            with self.subTest(archive_exists=exists):
                if exists:
                    archive.write_bytes(b"not an archive")
                result = self.run_installer(
                    "--archive", archive, "--prefix", self.prefix
                )
                self.assertEqual(result.returncode, 66)
                self.assertIn(
                    "no checksum at" if exists else "no archive at", result.stderr
                )
                self.assert_unchanged(result)

    def test_invalid_archive_or_missing_executable_never_clobbers_install(self):
        self.existing_install()
        for kind in ["invalid", "missing", "not executable"]:
            with self.subTest(kind=kind):
                archive = self.archive(
                    binary=None if kind == "missing" else BINARY,
                    mode=0o644 if kind == "not executable" else 0o755,
                )
                if kind == "invalid":
                    archive.write_bytes(b"not a gzip archive")
                    self.checksum(archive)
                result = self.run_installer(
                    "--archive", archive, "--prefix", self.prefix
                )
                self.assertIn(archive.name + ": OK", result.stdout)
                self.assert_unchanged(result)
                if kind != "invalid":
                    self.assertEqual(result.returncode, 65)
                    self.assertIn(
                        "the archive has no executable chatgpt", result.stderr
                    )

    def test_unsupported_linux_arch_and_os_fail_before_downloading(self):
        for os_name, arch in [("Linux", "aarch64"), ("FreeBSD", "x86_64")]:
            with self.subTest(os=os_name, arch=arch):
                self.env.update(FAKE_OS=os_name, FAKE_ARCH=arch)
                result = self.run_installer("--prefix", self.prefix)
                self.assertEqual(result.returncode, 69)
                self.assertIn(
                    f"no prebuilt chatgpt for {os_name} {arch}", result.stderr
                )
                self.assertIn("Linux x86_64 (glibc)", result.stderr)
                self.assertFalse(self.installed.exists())
                self.assertFalse(self.download_log.exists())

    def test_invalid_arguments_keep_exit_64(self):
        for args in [("--version",), ("--prefix",), ("--archive",), ("--unknown",)]:
            with self.subTest(args=args):
                result = self.run_installer(*args)
                self.assertEqual(result.returncode, 64)
                self.assertFalse(self.installed.exists())
                self.assertFalse(self.download_log.exists())


if __name__ == "__main__":
    unittest.main()
