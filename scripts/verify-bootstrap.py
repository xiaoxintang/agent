#!/usr/bin/env python3
"""Test distribution TLS and the actual installer in a private mount namespace.

The GitHub download endpoint is explicitly a local test stand-in serving real
built artifacts. This does not establish that a public GitHub release exists.
Run only on a disposable dedicated Linux VM with --owned-vm.
"""
import argparse
import http.server
import json
import os
import pathlib
import secrets
import socket
import ssl
import subprocess
import sys
import tarfile
import tempfile
import threading

parser = argparse.ArgumentParser()
parser.add_argument("--bundle", required=True, type=pathlib.Path)
parser.add_argument("--bootstrap", required=True, type=pathlib.Path)
parser.add_argument("--owned-vm", action="store_true", required=True)
parser.add_argument("--namespace", action="store_true", help=argparse.SUPPRESS)
args = parser.parse_args()
if os.geteuid() != 0:
    parser.error("requires root on an owned disposable VM")
if not args.namespace:
    os.execv("/usr/bin/unshare", ["unshare", "--mount", "--propagation", "private", "--", sys.executable, str(pathlib.Path(__file__).resolve()), *sys.argv[1:], "--namespace"])
for name in ["/var/lib/panel-installer", "/var/lib/panel", "/etc/panel", "/usr/local/bin/panel-server"]:
    assert not pathlib.Path(name).exists(), "pre-existing installation refused"
metadata = json.loads((args.bundle / "bundle.json").read_text())
version = "v" + metadata["version"]
fixture = pathlib.Path(tempfile.mkdtemp(prefix="panel-bootstrap-acceptance-", dir="/var/lib"))
fixture.chmod(0o700)
server = None
mounted = False


def command(argv, expected=0, env=None):
    result = subprocess.run([str(arg) for arg in argv], env=env, capture_output=True, text=True, timeout=180)
    assert result.returncode == expected, f"bootstrap acceptance command failed: {result.stdout[-1500:]} {result.stderr[-1500:]}"
    return result.stdout


try:
    archive = fixture / "bundle.tar.gz"
    with tarfile.open(archive, "w:gz", format=tarfile.USTAR_FORMAT) as output:
        for name in ["bundle.json", "panel-server", "panel-agent", "panel-installer"]:
            output.add(args.bundle / name, arcname=name, recursive=False)
    content = archive.read_bytes()
    cert = fixture / "distribution-cert.pem"
    key = fixture / "distribution-key.pem"
    command(["/usr/bin/openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", key, "-out", cert, "-days", "1", "-subj", "/CN=github.com", "-addext", "subjectAltName=DNS:github.com,IP:127.0.0.1", "-addext", "basicConstraints=critical,CA:FALSE", "-addext", "extendedKeyUsage=serverAuth"])
    key.chmod(0o600)
    cert.chmod(0o600)
    hosts = fixture / "hosts"
    hosts.write_text(pathlib.Path("/etc/hosts").read_text() + "\n127.0.0.1 github.com\n")
    command(["/usr/bin/mount", "--bind", hosts, "/etc/hosts"])
    mounted = True
    route = f"/xiaoxintang/agent/releases/download/{version}/panel-linux-{metadata['arch']}-{version}.tar.gz"

    class Distribution(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            if self.path != route:
                self.send_error(404)
                return
            self.send_response(200)
            self.send_header("Content-Type", "application/gzip")
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            self.wfile.write(content)

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 443), Distribution)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    config = fixture / "install.json"
    config.write_text(json.dumps({"settings": {"server_bind": f"127.0.0.1:{port}", "server_origin": f"https://127.0.0.1:{port}", "local_http": False, "tls_cert": str(cert), "tls_key": str(key), "autostart": False}, "admin_password": secrets.token_urlsafe(24)}))
    config.chmod(0o600)
    environment = os.environ.copy()
    environment["CURL_CA_BUNDLE"] = str(cert)
    environment["NO_PROXY"] = "github.com,127.0.0.1"
    base = ["/bin/sh", args.bootstrap, "--version", version, "--component", "server", "--non-interactive", "--config", config]
    command(base, env=environment)
    health_context = ssl.create_default_context(cafile=str(cert))
    import urllib.request
    with urllib.request.urlopen(f"https://127.0.0.1:{port}/api/v1/health", context=health_context, timeout=5) as response:
        assert json.load(response)["status"] == "ok"
    print(json.dumps({"distribution_endpoint": "explicit local TLS stand-in", "hostname_and_ca_verified": True, "real_bootstrap_installer_and_https_server": True}), flush=True)
finally:
    if pathlib.Path("/var/lib/panel-installer/manifest.json").exists():
        command([args.bundle / "panel-installer", "uninstall", "--component", "server", "--purge", "--confirm", "PURGE PANEL DATA"])
    if server:
        server.shutdown()
        server.server_close()
    if mounted:
        command(["/usr/bin/umount", "/etc/hosts"])
    for path in fixture.iterdir():
        assert path.is_file() and not path.is_symlink()
        path.unlink()
    fixture.rmdir()
    for name in ["/var/lib/panel", "/etc/panel"]:
        path = pathlib.Path(name)
        if path.exists() and not any(path.iterdir()):
            path.rmdir()
