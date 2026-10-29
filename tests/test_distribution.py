"""Exercise packaging and bootstrap with fake release assets; never install on the host."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parent.parent
VERSION = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
TAG = f"v{VERSION}"


def executable(path, source):
    # 用当前解释器制作模拟命令，测试不需要编译 Rust 或依赖真实系统服务。
    path.write_text(f"#!{sys.executable}\n" + source)
    path.chmod(0o755)


class DistributionTests(unittest.TestCase):
    def setUp(self):
        # 把打包输入和产物全部隔离到临时项目，测试不污染仓库的 target/dist。
        self.temporary = tempfile.TemporaryDirectory(prefix="agent-distribution-test-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.project = self.directory / "project"
        for name in [
            "Cargo.toml", "README.md", "scripts/package.sh", "deploy/install.sh",
            "deploy/xray-agent.service", "deploy/xray.service", "proto/LICENSE",
            "proto/upstream.json",
        ]:
            destination = self.project / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, destination)

    def package(self, arch="amd64", binary_version=VERSION):
        # 伪二进制仅实现打包冒烟检查需要的命令，真正验证的是包名、文件和版本约束。
        target = {
            "amd64": "x86_64-unknown-linux-musl",
            "arm64": "aarch64-unknown-linux-musl",
        }[arch]
        binary = self.project / f"target/{target}/release/agent"
        binary.parent.mkdir(parents=True, exist_ok=True)
        executable(binary, f"""
import json, pathlib, sys
if sys.argv[1:] == ['-v']:
    print('agent {binary_version}')
elif sys.argv[1:] == ['--help']:
    print('Usage: agent <COMMAND>')
elif sys.argv[1:3] == ['openapi', '--output']:
    pathlib.Path(sys.argv[3]).write_text(json.dumps({{'openapi':'3.1.0','info':{{'title':'Fixture','version':'{VERSION}'}},'paths':{{}}}}))
else:
    raise SystemExit(1)
""")
        result = subprocess.run(
            ["/bin/sh", str(self.project / "scripts/package.sh"), target, arch],
            capture_output=True, text=True, timeout=15,
        )
        return result, self.project / f"dist/agent-{TAG}-linux-{arch}.tar.gz"

    def bootstrap(self, assets, *arguments, machine="x86_64"):
        # 下载、架构识别和部署入口都由 PATH 中的替身接管，不访问 GitHub 或写系统目录。
        mocks = self.directory / "mocks"
        mocks.mkdir(exist_ok=True)
        marker = self.directory / "deployed"
        log = self.directory / "downloaded"
        executable(mocks / "id", "print('0')\n")
        executable(mocks / "uname", "import os,sys\nprint('Linux' if sys.argv[1]=='-s' else os.environ['MOCK_MACHINE'])\n")
        executable(mocks / "systemctl", "import sys\nraise SystemExit(0 if sys.argv[1:] == ['show-environment'] else 1)\n")
        executable(mocks / "curl", """
import os, pathlib, shutil, sys
args=sys.argv[1:]
url=args[-1]
with open(os.environ['MOCK_LOG'],'a') as log:
    log.write(url+'\\n')
if '--head' in args:
    print(url.removesuffix('/releases/latest')+'/releases/tag/'+os.environ['MOCK_TAG'],end='')
else:
    destination=pathlib.Path(args[args.index('--output')+1])
    source=pathlib.Path(os.environ['MOCK_ASSETS'])/url.rsplit('/',1)[-1]
    if not source.is_file():
        raise SystemExit(22)
    shutil.copyfile(source,destination)
""")
        executable(mocks / "sha256sum", """
import hashlib, pathlib, sys
assert sys.argv[1]=='--check'
for line in pathlib.Path(sys.argv[2]).read_text().splitlines():
    expected,name=line.split()
    if hashlib.sha256(pathlib.Path(name).read_bytes()).hexdigest()!=expected:
        print('checksum mismatch',file=sys.stderr)
        raise SystemExit(1)
""")
        # Only intercept the final deployment invocation. The bootstrap itself runs via /bin/sh.
        executable(mocks / "sh", """
import os, pathlib, sys
script=pathlib.Path(sys.argv[1])
binary=pathlib.Path(sys.argv[2])
assert script.name=='install.sh' and script.parent.name=='deploy'
assert binary.is_file() and not binary.is_symlink()
assert (script.parent/'xray-agent.service').read_text().startswith('# Managed by xray-agent')
pathlib.Path(os.environ['MOCK_MARKER']).write_text('deployed')
""")
        environment = dict(
            os.environ, PATH=str(mocks) + os.pathsep + os.environ["PATH"],
            MOCK_ASSETS=str(assets), MOCK_TAG=TAG, MOCK_MACHINE=machine,
            MOCK_MARKER=str(marker), MOCK_LOG=str(log),
        )
        result = subprocess.run(
            ["/bin/sh", str(ROOT / "install.sh"), *arguments],
            env=environment, capture_output=True, text=True, timeout=15,
        )
        return result, marker, log

    def release(self, arch="amd64"):
        # 摘要从实际测试归档计算，后续篡改测试才能覆盖校验失败时的安装阻断。
        result, archive = self.package(arch)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        (archive.parent / "SHA256SUMS").write_text(f"{checksum}  {archive.name}\n")
        return archive

    def test_archive_contains_executable_docs_and_units(self):
        result, archive = self.package()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        stem = f"agent-{TAG}-linux-amd64"
        with tarfile.open(archive) as package:
            self.assertEqual(package.getmember(f"{stem}/agent").mode & 0o777, 0o755)
            for name in ["deploy/install.sh", "deploy/xray-agent.service", "deploy/xray.service", "licenses/Xray-core.LICENSE", "README.md"]:
                self.assertTrue(package.getmember(f"{stem}/{name}").isfile())
            self.assertEqual(json.load(package.extractfile(f"{stem}/openapi.json"))["openapi"], "3.1.0")

    def test_package_rejects_wrong_binary_version(self):
        result, archive = self.package(binary_version="0.0.0-wrong")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(archive.exists())

    def test_latest_release_is_resolved_once_and_installed(self):
        archive = self.release()
        result, marker, log = self.bootstrap(archive.parent)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(marker.exists())
        urls = log.read_text().splitlines()
        self.assertEqual(sum(url.endswith('/releases/latest') for url in urls), 1)
        self.assertTrue(any(f'/releases/download/{TAG}/{archive.name}' in url for url in urls))

    def test_explicit_arm64_release_and_fork(self):
        archive = self.release("arm64")
        result, marker, log = self.bootstrap(archive.parent, "--version", TAG, "--repo", "example/fork", machine="aarch64")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(marker.exists())
        self.assertTrue(all(url.startswith('https://github.com/example/fork/releases/download/') for url in log.read_text().splitlines()))

    def test_checksum_failure_prevents_deployment(self):
        archive = self.release()
        (archive.parent / "SHA256SUMS").write_text(f"{'0' * 64}  {archive.name}\n")
        result, marker, _ = self.bootstrap(archive.parent, "--version", TAG)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(marker.exists())

    def test_duplicate_checksum_prevents_deployment(self):
        archive = self.release()
        checksums = archive.parent / "SHA256SUMS"
        checksums.write_text(checksums.read_text() * 2)
        result, marker, _ = self.bootstrap(archive.parent, "--version", TAG)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(marker.exists())

    def test_unsupported_machine_does_not_download(self):
        result, marker, log = self.bootstrap(self.directory, machine="mips")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(marker.exists())
        self.assertFalse(log.exists())

    def test_invalid_inputs_are_rejected_before_network(self):
        for arguments in [
            ["--version", "../bad"], ["--version", "--help"],
            ["--repo", "owner/repo/extra"], ["--repo", "owner;false/repo"],
            ["--unknown"], ["--version"],
        ]:
            with self.subTest(arguments=arguments):
                result = subprocess.run(["/bin/sh", str(ROOT / "install.sh"), *arguments], capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)

    def test_help_needs_no_root_or_network(self):
        result = subprocess.run(["/bin/sh", str(ROOT / "install.sh"), "--help"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn("--version", result.stdout)


if __name__ == "__main__":
    unittest.main()
