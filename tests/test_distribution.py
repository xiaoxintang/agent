"""Exercise packaging and bootstrap with fake release assets; never install on the host."""
import hashlib
import json
import os
import re
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
            "deploy/xray-agent.service", "deploy/xray.service", "deploy/xray-web.service", "deploy/options.sh", "proto/LICENSE",
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
import json, pathlib, subprocess, sys
if sys.argv[1:] == ['-v']:
    print('agent {binary_version}')
elif sys.argv[1:] == ['install-defaults']:
    print('agent_port=8080\\ngrpc_port=10085\\nweb_port=3000\\nweb_enabled=no')
elif sys.argv[1:2] == ['verify-web']:
    frontend=pathlib.Path(sys.argv[sys.argv.index('--frontend')+1])
    backend=sys.argv[sys.argv.index('--backend')+1]
    assert subprocess.check_output([backend,'--version'],text=True).strip()=='agent-web {binary_version}'
    assert json.loads((frontend/'version.json').read_text())['version']=='{binary_version}'
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

    def bootstrap(self, assets, *arguments, machine="x86_64", embedded=False):
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
import json, os, pathlib, sys
script=pathlib.Path(sys.argv[1])
binary=pathlib.Path(sys.argv[2])
assert script.name=='install.sh' and script.parent.name=='deploy'
assert binary.is_file() and not binary.is_symlink()
assert (script.parent/'xray-agent.service').read_text().startswith('# Managed by xray-agent')
pathlib.Path(os.environ['MOCK_MARKER']).write_text(json.dumps(sys.argv[3:]))
""")
        environment = dict(
            os.environ, PATH=str(mocks) + os.pathsep + os.environ["PATH"],
            MOCK_ASSETS=str(assets), MOCK_TAG=TAG, MOCK_MACHINE=machine,
            MOCK_MARKER=str(marker), MOCK_LOG=str(log),
        )
        command = ["/bin/sh", str(ROOT / "install.sh"), "--non-interactive"]
        if embedded:
            command = ["/bin/sh", "-c", (ROOT / "install.sh").read_text(), "agent-update"]
        result = subprocess.run(
            [*command, *(["--non-interactive"] if embedded else []), *arguments],
            env=environment, capture_output=True, text=True, timeout=15,
        )
        return result, marker, log

    def release(self, arch="amd64", web=False, frontend_version=VERSION):
        # 摘要从实际测试归档计算，后续篡改测试才能覆盖校验失败时的安装阻断。
        result, archive = self.package(arch)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
        (archive.parent / "SHA256SUMS").write_text(f"{checksum}  {archive.name}\n")
        if web:
            for kind in ['backend','frontend']:
                name=f'web-{kind}-{TAG}'+(f'-linux-{arch}' if kind=='backend' else '')
                directory=self.directory/name
                directory.mkdir()
                (directory/'version.json').write_text(json.dumps({'version':frontend_version if kind=='frontend' else VERSION}))
                if kind=='frontend':(directory/'index.html').write_text('<html>fixture</html>')
                else:executable(directory/'agent-web',f"print('agent-web {VERSION}')\n")
                asset=archive.parent/(name+'.tar.gz')
                with tarfile.open(asset,'w:gz') as tar:tar.add(directory,arcname=name)
                with (archive.parent/'SHA256SUMS').open('a') as sums:sums.write(f'{hashlib.sha256(asset.read_bytes()).hexdigest()}  {asset.name}\n')
        return archive

    def deploy(self, xray_failure=False, update=False, installed=False, extra=(), health_failure=False, installed_web=False, installer=None):
        mocks = self.directory / "mocks"
        mocks.mkdir()
        log = self.directory / "commands"
        root = self.directory / "root"
        for path in ["usr/local/bin", "etc/systemd/system", "run/systemd/system", "var/lib/xray-agent"]:
            (root/path).mkdir(parents=True, exist_ok=True)
        executable(mocks / "id", "print('0')\n")
        executable(mocks / "uname", "print('Linux')\n")
        executable(mocks / "systemctl", """
import os, sys
with open(os.environ['MOCK_LOG'], 'a') as log:
    log.write('systemctl ' + ' '.join(sys.argv[1:]) + '\\n')
if sys.argv[1] in ['is-active','is-enabled']:
    raise SystemExit(1)
""")
        binary = self.directory / "agent"
        executable(binary, f"""
import os, pathlib, sys
with open(os.environ['MOCK_LOG'], 'a') as log:
    log.write('agent ' + ' '.join(sys.argv[1:]) + '\\n')
if sys.argv[1:]==['-v']:
    print('agent {VERSION}')
elif sys.argv[1:]==['install-defaults']:
    print('agent_port=8080\\ngrpc_port=10085\\nweb_port=3000\\nweb_enabled=no')
elif sys.argv[1:2]==['install-backup']:
    import shutil
    shutil.copyfile(pathlib.Path(os.environ['MOCK_ROOT'])/'var/lib/xray-agent/state.db',sys.argv[sys.argv.index('--output')+1])
elif sys.argv[1:]==['install-xray'] and os.environ['MOCK_XRAY_FAILURE']=='1':
    raise SystemExit(1)
elif sys.argv[1:]==['install-health'] and os.environ['MOCK_HEALTH_FAILURE']=='1':
    raise SystemExit(1)
""")
        if installed_web:
            for name in ['getent','groupadd','useradd','chgrp','chown']:
                executable(mocks/name,"raise SystemExit(0)\n")
            executable(mocks/'install',"""
import subprocess,sys
args=sys.argv[1:];clean=[];index=0
while index<len(args):
    if args[index] in ['-o','-g']:index+=2
    else:clean.append(args[index]);index+=1
raise SystemExit(subprocess.call(['/usr/bin/install',*clean]))
""")
            executable(mocks/'mv',"""
import os,subprocess,sys
if sys.argv[1]=='-Tf':os.replace(sys.argv[2],sys.argv[3])
else:raise SystemExit(subprocess.call(['/bin/mv',*sys.argv[1:]]))
""")
            frontend=self.directory/'frontend'
            frontend.mkdir();(frontend/'index.html').write_text('new frontend');(frontend/'version.json').write_text(json.dumps({'version':VERSION}))
            backend=self.directory/'agent-web';executable(backend,f"print('agent-web {VERSION}')\n")
            extra=(*extra,'--web','yes','--web-backend',str(backend),'--web-frontend',str(frontend))
        script = re.sub(r"/(?:etc|usr/local|var|opt|run)/", lambda m: str(root)+m.group(), (installer or ROOT/"deploy/install.sh").read_text())
        installer = installer or self.project / "deploy/install.sh"
        installer.write_text(script)
        installed_binary = root / "usr/local/bin/agent"
        if installed:
            executable(installed_binary, "print('old agent')\n")
            (root/'etc/xray-agent').mkdir(parents=True)
            (root/'etc/xray-agent/preserved').write_text('old token')
            (root/'var/lib/xray-agent/state.db').write_bytes(b'old database')
        if installed_web:
            old=root/'opt/xray-web/versions/old'
            (old/'frontend').mkdir(parents=True)
            (old/'frontend/index.html').write_text('old frontend')
            (old/'agent-web').write_text('old backend')
            (root/'opt/xray-web/current').symlink_to(old)
            shutil.copyfile(ROOT/'deploy/xray-web.service',root/'etc/systemd/system/xray-web.service')
            for file in ['web.toml','web-credentials.json','web-local-agent.json']:(root/'etc/xray-agent'/file).write_text('old '+file)
            # The fake initializer represents generated files; all paths remain in this sandbox.
            with binary.open('a') as file:file.write("\nif sys.argv[1:2]==['init']:\n    root=pathlib.Path(os.environ['MOCK_ROOT'])/'etc/xray-agent'\n    root.mkdir(parents=True,exist_ok=True)\n    for name in ['web.toml','web-credentials.json','web-local-agent.json']:(root/name).write_text('new '+name)\n")
        environment = dict(os.environ, PATH=str(mocks)+os.pathsep+os.environ['PATH'], MOCK_ROOT=str(root),MOCK_LOG=str(log), MOCK_XRAY_FAILURE='1' if xray_failure else '0',MOCK_HEALTH_FAILURE='1' if health_failure else '0')
        result = subprocess.run(['/bin/sh',str(installer),str(binary),'--non-interactive',*(['--update'] if update else []),*extra],env=environment,capture_output=True,text=True,timeout=15)
        return result, log.read_text().splitlines() if log.exists() else []

    def test_deployment_installs_default_xray_before_starting_agent(self):
        result, commands = self.deploy()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        init=next(i for i,c in enumerate(commands) if c.startswith('agent init '))
        self.assertIn('--agent-port 8080 --grpc-port 10085 --web-port 3000 --web no',commands[init])
        self.assertLess(init,commands.index('agent install-xray'))
        self.assertLess(commands.index('agent install-xray'),commands.index('systemctl start xray-agent.service'))
        self.assertEqual(commands[-1],'agent install-health')

    def test_xray_install_failure_still_starts_management_and_reports_failure(self):
        result, commands = self.deploy(xray_failure=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("systemctl start xray-agent.service",commands)
        self.assertEqual(commands[-1],"agent install-health")
        self.assertIn("Default Xray installation failed", result.stderr)

    def test_agent_update_restarts_agent_without_installing_xray(self):
        result, commands = self.deploy(update=True, xray_failure=True, installed=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("agent install-xray", commands)
        self.assertIn("systemctl start xray-agent.service",commands)
        self.assertEqual(commands[-1],"agent install-health")

    def test_update_requires_an_existing_agent(self):
        result, commands = self.deploy(update=True, installed=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(commands, [])
        self.assertIn("Agent is not installed", result.stderr)
        self.assertFalse((self.directory / "root/usr/local/bin/agent").exists())

    def test_packaged_installer_supports_legacy_updater_extraction(self):
        result, archive = self.package()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        legacy = self.directory / "legacy"
        legacy.mkdir()
        stem = f"agent-{TAG}-linux-amd64"
        subprocess.run([
            "tar", "-xzf", str(archive), "-C", str(legacy), "--strip-components=1",
            f"{stem}/agent", f"{stem}/deploy/install.sh", f"{stem}/deploy/xray-agent.service",
        ], check=True)
        self.assertFalse((legacy / "deploy/options.sh").exists())
        result, commands = self.deploy(
            update=True, installed=True, installer=legacy / "deploy/install.sh",
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn("agent install-xray", commands)
        self.assertEqual(commands[-1], "agent install-health")
        root = self.directory / "root"
        self.assertEqual((root / "etc/xray-agent/preserved").read_text(), "old token")
        self.assertEqual((root / "var/lib/xray-agent/state.db").read_bytes(), b"old database")

    def test_embedded_updater_downloads_latest_and_forwards_update_mode(self):
        archive = self.release()
        result, marker, log = self.bootstrap(archive.parent, "--update", embedded=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("--update", json.loads(marker.read_text()))
        self.assertTrue(any(url.endswith('/releases/latest') for url in log.read_text().splitlines()))

    def test_embedded_updater_uses_the_requested_release(self):
        archive = self.release()
        result, marker, log = self.bootstrap(archive.parent, "--update", "--version", TAG, embedded=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("--update", json.loads(marker.read_text()))
        self.assertTrue(all(f'/releases/download/{TAG}/' in url for url in log.read_text().splitlines()))

    def test_embedded_update_checksum_failure_does_not_deploy(self):
        archive = self.release()
        (archive.parent / "SHA256SUMS").write_text(f"{'0' * 64}  {archive.name}\n")
        result, marker, _ = self.bootstrap(archive.parent, "--update", embedded=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(marker.exists())

    def test_archive_contains_executable_docs_and_units(self):
        result, archive = self.package()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        stem = f"agent-{TAG}-linux-amd64"
        with tarfile.open(archive) as package:
            self.assertEqual(package.getmember(f"{stem}/agent").mode & 0o777, 0o755)
            for name in ["deploy/install.sh", "deploy/options.sh", "deploy/xray-agent.service", "deploy/xray-web.service", "deploy/xray.service", "licenses/Xray-core.LICENSE", "README.md"]:
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

    def test_disabled_web_downloads_no_optional_artifacts(self):
        archive=self.release()
        result,marker,log=self.bootstrap(archive.parent)
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        self.assertFalse(any('/web-' in url for url in log.read_text().splitlines()))

    def test_enabled_web_downloads_exact_matching_pair(self):
        archive=self.release(web=True)
        result,marker,log=self.bootstrap(archive.parent,'--web','yes','--agent-port','9000','--grpc-port','12000','--web-port','4000')
        self.assertEqual(result.returncode,0,result.stdout+result.stderr)
        urls=log.read_text()
        self.assertIn(f'web-backend-{TAG}-linux-amd64.tar.gz',urls)
        self.assertIn(f'web-frontend-{TAG}.tar.gz',urls)
        arguments=json.loads(marker.read_text())
        self.assertEqual(arguments[arguments.index('--web')+1],'yes')
        self.assertEqual(arguments[arguments.index('--web-port')+1],'4000')

    def test_missing_web_artifact_never_falls_back_or_deploys(self):
        archive=self.release()
        result,marker,_=self.bootstrap(archive.parent,'--web','yes')
        self.assertNotEqual(result.returncode,0)
        self.assertFalse(marker.exists())

    def test_mismatched_frontend_is_rejected_before_deployment(self):
        archive=self.release(web=True,frontend_version='0.0.0')
        result,marker,_=self.bootstrap(archive.parent,'--web','yes')
        self.assertNotEqual(result.returncode,0)
        self.assertFalse(marker.exists())

    def test_bad_web_checksum_is_rejected_before_deployment(self):
        archive=self.release(web=True)
        with (archive.parent/f'web-frontend-{TAG}.tar.gz').open('ab') as file:file.write(b'tampered')
        result,marker,_=self.bootstrap(archive.parent,'--web','yes')
        self.assertNotEqual(result.returncode,0)
        self.assertFalse(marker.exists())

    def test_health_failure_restores_previous_binary_and_state(self):
        result,commands=self.deploy(update=True,installed=True,health_failure=True)
        self.assertNotEqual(result.returncode,0)
        root=self.directory/'root'
        output=subprocess.check_output([str(root/'usr/local/bin/agent')],text=True)
        self.assertEqual(output.strip(),'old agent')
        self.assertEqual((root/'etc/xray-agent/preserved').read_text(),'old token')
        self.assertEqual((root/'var/lib/xray-agent/state.db').read_bytes(),b'old database')
        self.assertFalse((root/'var/lib/xray-agent/install-transaction').exists())

    def test_failed_web_update_restores_both_web_components_and_credentials(self):
        result,commands=self.deploy(update=True,installed=True,health_failure=True,installed_web=True)
        self.assertIn('systemctl start xray-web.service',commands)
        self.assertIn('agent install-health',commands)
        self.assertNotEqual(result.returncode,0,result.stdout+result.stderr)
        root=self.directory/'root'
        self.assertIn('Restoring the previous installation',result.stderr)
        self.assertEqual((root/'opt/xray-web/current/frontend/index.html').read_text(),'old frontend')
        self.assertEqual((root/'opt/xray-web/current/agent-web').read_text(),'old backend')
        self.assertEqual((root/'etc/xray-agent/web-credentials.json').read_text(),'old web-credentials.json')

    def test_help_needs_no_root_or_network(self):
        result = subprocess.run(["/bin/sh", str(ROOT / "install.sh"), "--help"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn("--version", result.stdout)


if __name__ == "__main__":
    unittest.main()
