"""Run the embedded uninstall script against a temporary filesystem and fake systemd."""
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent


class UninstallTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="agent-uninstall-test-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.root = self.directory / "root"
        self.log = self.directory / "commands"
        self.units = self.root / "etc/systemd/system"
        self.data_paths = [
            "etc/xray-agent", "etc/xray", "opt/xray", "var/lib/xray-agent",
            "var/log/xray", "run/xray",
            "etc/systemd/system/xray-agent.service.d", "etc/systemd/system/xray.service.d",
            "run/systemd/system/xray-agent.service.d", "run/systemd/system/xray.service.d",
        ]
        for path in self.data_paths:
            directory = self.root / path
            directory.mkdir(parents=True)
            (directory / "data").write_text("fixture")
        for unit in ["xray-agent.service", "xray.service"]:
            shutil.copyfile(ROOT / "deploy" / unit, self.units / unit)
            wants = self.units / "multi-user.target.wants"
            wants.mkdir(exist_ok=True)
            (wants / unit).symlink_to(self.units / unit)
        for name in ["agent", "agent.new"]:
            binary = self.root / "usr/local/bin" / name
            binary.parent.mkdir(parents=True, exist_ok=True)
            binary.write_text("binary fixture")
        # This sentinel and a later symlink target must survive recursive cleanup.
        self.unrelated = self.root / "etc/unrelated.conf"
        self.unrelated.write_text("keep")
        self.mocks = self.directory / "mocks"
        self.mocks.mkdir()
        self.executable("id", "print('0')\n")
        self.executable("uname", "print('Linux')\n")
        self.executable("systemctl", """
import os, pathlib, sys
args = sys.argv[1:]
with open(os.environ['MOCK_LOG'], 'a') as log:
    log.write(' '.join(args) + '\\n')
if args[0] == 'show':
    root = pathlib.Path(os.environ['MOCK_ROOT'])
    fragment = root / 'etc/systemd/system' / args[1]
    if not fragment.exists():
        fragment = root / 'usr/lib/systemd/system' / args[1]
    if args[1] in os.environ.get('MOCK_CACHED_UNITS', '').split():
        fragment = root / 'etc/systemd/system' / args[1]
        print('LoadState=loaded\\nFragmentPath=' + str(fragment))
    elif fragment.exists():
        print('LoadState=loaded\\nFragmentPath=' + str(fragment))
    else:
        print('LoadState=not-found\\nFragmentPath=')
        raise SystemExit(1)
elif args == ['stop', os.environ.get('MOCK_STOP_FAILURE')]:
    raise SystemExit(1)
""")
        # Rewrite all fixed system path prefixes in one pass, including error messages.
        self.script = re.sub(
            r"/(?:etc|usr/local|var|opt|run)/",
            lambda match: str(self.root) + match.group(),
            (ROOT / "deploy/uninstall.sh").read_text(),
        )

    def executable(self, name, source):
        path = self.mocks / name
        path.write_text(f"#!{sys.executable}\n" + source)
        path.chmod(0o755)

    def uninstall(self, stop_failure="", cached_units=""):
        environment = dict(
            os.environ, PATH=str(self.mocks) + os.pathsep + os.environ["PATH"],
            MOCK_LOG=str(self.log), MOCK_ROOT=str(self.root), MOCK_STOP_FAILURE=stop_failure,
            MOCK_CACHED_UNITS=cached_units,
        )
        return subprocess.run(
            ["/bin/sh", "-c", self.script, "agent-uninstall"], env=environment,
            capture_output=True, text=True, timeout=15,
        )

    def test_uninstall_removes_binaries_units_configuration_and_state(self):
        result = self.uninstall()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for path in self.data_paths + ["usr/local/bin/agent", "usr/local/bin/agent.new"]:
            self.assertFalse((self.root / path).exists(), path)
        for unit in ["xray-agent.service", "xray.service"]:
            self.assertFalse((self.units / unit).exists())
            self.assertFalse((self.units / "multi-user.target.wants" / unit).is_symlink())
        actions = self.log.read_text().splitlines()
        self.assertLess(actions.index("stop xray-agent.service"), actions.index("stop xray.service"))
        self.assertLess(actions.index("stop xray.service"), actions.index("disable xray-agent.service"))
        self.assertIn("disable xray.service", actions)
        self.assertIn("daemon-reload", actions)
        self.assertEqual(self.unrelated.read_text(), "keep")
        repeated = self.uninstall()
        self.assertEqual(repeated.returncode, 0, repeated.stdout + repeated.stderr)

    def test_stop_failure_preserves_all_files(self):
        for unit in ["xray-agent.service", "xray.service"]:
            with self.subTest(unit=unit):
                result = self.uninstall(stop_failure=unit)
                self.assertNotEqual(result.returncode, 0)
                self.assertTrue((self.root / "usr/local/bin/agent").is_file())
                self.assertTrue((self.units / "xray-agent.service").is_file())
                self.assertTrue((self.units / "xray.service").is_file())
                for path in self.data_paths:
                    self.assertEqual((self.root / path / "data").read_text(), "fixture")

    def test_unmanaged_unit_is_rejected_before_stopping_services(self):
        (self.units / "xray.service").write_text("# managed by someone else\n")
        result = self.uninstall()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Unmanaged unit", result.stderr)
        self.assertNotIn("stop ", self.log.read_text())
        self.assertTrue((self.root / "usr/local/bin/agent").is_file())
        self.assertTrue((self.root / "etc/xray/data").is_file())

    def test_unit_outside_managed_path_is_preserved(self):
        (self.units / "xray.service").unlink()
        external = self.root / "usr/lib/systemd/system/xray.service"
        external.parent.mkdir(parents=True)
        external.write_text("# external Xray\n")
        result = self.uninstall()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Unmanaged unit", result.stderr)
        self.assertNotIn("stop ", self.log.read_text())
        self.assertEqual(external.read_text(), "# external Xray\n")

    def test_external_xray_binary_is_preserved(self):
        external = self.root / "usr/local/bin/xray"
        external.write_text("external Xray")
        result = self.uninstall()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(external.read_text(), "external Xray")
        self.assertTrue((self.root / "etc/xray/data").is_file())

    def test_missing_services_do_not_prevent_cleanup(self):
        for unit in ["xray-agent.service", "xray.service"]:
            (self.units / unit).unlink()
        result = self.uninstall()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((self.root / "etc/xray-agent").exists())
        self.assertNotIn("stop ", self.log.read_text())

    def test_cleanup_does_not_follow_symlinks_outside_installation(self):
        outside = self.directory / "keep"
        outside.mkdir()
        (outside / "data").write_text("keep")
        (self.root / "opt/xray/versions").symlink_to(outside, target_is_directory=True)
        result = self.uninstall()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((outside / "data").read_text(), "keep")

    def test_cached_services_are_stopped_after_unit_files_were_deleted(self):
        for unit in ["xray-agent.service", "xray.service"]:
            (self.units / unit).unlink()
        result = self.uninstall(cached_units="xray-agent.service xray.service")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        commands = self.log.read_text().splitlines()
        self.assertIn("stop xray-agent.service", commands)
        self.assertIn("stop xray.service", commands)
        self.assertFalse((self.root / "var/lib/xray-agent").exists())


if __name__ == "__main__":
    unittest.main()
