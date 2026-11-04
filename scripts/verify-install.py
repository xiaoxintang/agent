#!/usr/bin/env python3
"""Real root/systemd acceptance on a fresh, explicitly dedicated Linux test host.

Run with sudo and --bundle DIR --sha256 TRUSTED_HASH. It refuses any existing
Panel installation, creates only owned Panel accounts/services/data, and purges
that installation in finally. No packages or unrelated services are modified.
"""
import argparse
import hashlib
import http.cookiejar
import json
import os
import pathlib
import pwd
import secrets
import socket
import sqlite3
import ssl
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid

parser = argparse.ArgumentParser()
parser.add_argument("--bundle", type=pathlib.Path, required=True)
parser.add_argument("--sha256", required=True)
parser.add_argument("--failure-bundle", type=pathlib.Path)
parser.add_argument("--failure-sha256")
parser.add_argument("--wizard", action="store_true", help="Drive the actual TTY wizard for the initial install")
parser.add_argument("--browser-state", type=pathlib.Path)
parser.add_argument("--browser-done", type=pathlib.Path)
parser.add_argument("--backup-restore", action="store_true")
args = parser.parse_args()
if os.geteuid() != 0:
    parser.error("requires root on a dedicated fresh test host")
for path in ["/var/lib/panel-installer", "/var/lib/panel", "/etc/panel", "/opt/panel/xray", "/usr/local/bin/panel-installer", "/usr/local/bin/panel-server", "/usr/local/bin/panel-agent", "/usr/local/bin/xray", "/usr/bin/xray", "/etc/xray", "/etc/systemd/system/xray.service"]:
    if pathlib.Path(path).exists():
        parser.error(f"existing path refused before any mutation: {path}")
for name in ["panel-server", "panel-agent"]:
    try:
        pwd.getpwnam(name)
    except KeyError:
        pass
    else:
        parser.error(f"existing account refused: {name}")
version = json.loads((args.bundle / "bundle.json").read_text())["version"]
parents_before = {path: pathlib.Path(path).exists() for path in ["/etc/panel", "/var/lib/panel", "/opt/panel"]}
installer = args.bundle / "panel-installer"
fixture = pathlib.Path(tempfile.mkdtemp(prefix="panel-install-acceptance-", dir="/var/lib"))
fixture.chmod(0o700)
label = "phase7-" + str(uuid.uuid4())
password = secrets.token_urlsafe(24)


def run(command, expected=0, stdin_data=None):
    result = subprocess.run([str(arg) for arg in command], capture_output=True, text=True, timeout=360, input=stdin_data)
    if result.returncode != expected:
        raise AssertionError(f"acceptance subprocess failed: {command[0]}: {result.returncode}: {result.stdout[:2000]} {result.stderr[:2000]}")
    return result.stdout + result.stderr


def cli(*arguments, expected=0):
    return run([installer, *arguments], expected)


def write_config(path, settings, token=None):
    path.write_text(json.dumps({"settings": settings, "admin_password": password, "enrollment_token": token}))
    path.chmod(0o600)


def wait(predicate, description, seconds=45):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(0.25)
    raise AssertionError("acceptance timeout: " + description)


def free_port():
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        return reservation.getsockname()[1]


def wizard_install(settings, cert, key, authority):
    import fcntl
    import pty
    import re
    import select
    import signal
    import struct
    import termios

    steps = [
        ("Component", "\r"),
        ("Setup mode", "\x1b[B\r"),
        ("Use local-only development HTTP?", "n"),
        ("Server bind IP:port", settings["server_bind"] + "\r"),
        ("Exact browser origin", settings["server_origin"] + "\r"),
        ("Automatically obtain and renew", "n"),
        ("TLS certificate PEM path", str(cert) + "\r"),
        ("TLS private key PEM path", str(key) + "\r"),
        ("Server data path under /var/lib/panel", "\r"),
        ("Administrator username", "\r"),
        ("First administrator password", password + "\r"),
        ("Confirm password", password + "\r"),
        ("Node display name", label + "\r"),
        ("Additional trusted CA PEM path", str(authority) + "\r"),
        ("Install verified Xray", "n"),
        ("Enable services at boot?", "n"),
        ("Apply this plan?", "y"),
    ]
    pid, master = pty.fork()
    if pid == 0:
        attributes = termios.tcgetattr(0)
        attributes[3] &= ~termios.ECHO
        termios.tcsetattr(0, termios.TCSANOW, attributes)
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        os.environ["TERM"] = "xterm-256color"
        os.execv(str(installer), [str(installer), "install", "--component", "all", "--bundle", str(args.bundle), "--bundle-sha256", args.sha256])
    index = 0
    output = ""
    deadline = time.monotonic() + 150
    result = None
    try:
        while time.monotonic() < deadline:
            if select.select([master], [], [], 0.1)[0]:
                try:
                    chunk = os.read(master, 8192)
                except OSError:
                    chunk = b""
                if chunk:
                    if b"\x1b[6n" in chunk:
                        os.write(master, b"\x1b[1;1R")
                    output += chunk.decode(errors="replace")
                    assert len(output) < 256 * 1024, "TTY output exceeded bound"
                    plain = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", output)
                    if index < len(steps) and steps[index][0] in plain:
                        time.sleep(0.05)
                        os.write(master, steps[index][1].encode())
                        index += 1
                        output = ""
            child, status = os.waitpid(pid, os.WNOHANG)
            if child:
                result = os.waitstatus_to_exitcode(status)
                while select.select([master], [], [], 0)[0]:
                    try:
                        trailing = os.read(master, 8192)
                    except OSError:
                        break
                    if not trailing:
                        break
                    output += trailing.decode(errors="replace")
                break
        if result is None:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        diagnostic = output.replace(password, "[test password redacted]")[-2000:]
        assert result == 0 and index == len(steps), f"TTY wizard failed at step {index}, exit {result}: {diagnostic}"
        print("acceptance: actual TTY wizard installed the selected plan", flush=True)
    finally:
        os.close(master)


manifest = pathlib.Path("/var/lib/panel-installer/manifest.json")
try:
    key = fixture / "key.pem"
    cert = fixture / "cert.pem"
    authority = fixture / "authority.pem"
    authority_key = fixture / "authority-key.pem"
    csr = fixture / "server.csr"
    extensions = fixture / "server.ext"
    extensions.write_text("subjectAltName=IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n")
    run(["/usr/bin/openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", authority_key, "-out", authority, "-days", "1", "-subj", "/CN=Panel isolated test authority", "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign"])
    run(["/usr/bin/openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", key, "-out", csr, "-subj", "/CN=localhost"])
    run(["/usr/bin/openssl", "x509", "-req", "-in", csr, "-CA", authority, "-CAkey", authority_key, "-CAcreateserial", "-out", cert, "-days", "1", "-extfile", extensions])
    authority_key.chmod(0o600)
    authority.chmod(0o600)
    key.chmod(0o600)
    cert.chmod(0o600)
    port = free_port()
    origin = f"https://127.0.0.1:{port}"
    settings = {"server_bind": f"127.0.0.1:{port}", "server_origin": origin, "local_http": False, "tls_cert": str(cert), "tls_key": str(key), "agent_server": origin.replace("https:", "wss:") + "/agent/v1/connect", "agent_name": label, "agent_ca": str(authority), "autostart": False}
    config = fixture / "install.json"
    write_config(config, settings)
    if args.wizard:
        wizard_install(settings, cert, key, authority)
    else:
        cli("install", "--component", "all", "--non-interactive", "--config", config, "--bundle", args.bundle, "--bundle-sha256", args.sha256)
    print("acceptance: real HTTPS Server, unprivileged Agent and root socket helper installed", flush=True)
    owned = json.loads(manifest.read_text())
    assert owned["settings"]["agent_name"] == label
    assert json.loads(cli("status"))["integrity_ok"]
    assert "already installed" in cli("install", "--component", "all", "--non-interactive", "--config", config, "--bundle", args.bundle, "--bundle-sha256", args.sha256, expected=1)
    context = ssl.create_default_context(cafile=str(authority))
    cookies = http.cookiejar.CookieJar()
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context), urllib.request.HTTPCookieProcessor(cookies))
    csrf = None

    def api(path, method="GET", body=None):
        headers = {"Origin": origin}
        if csrf:
            headers["X-CSRF-Token"] = csrf
        data = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            data = json.dumps(body).encode()
        with opener.open(urllib.request.Request(origin + "/api/v1" + path, data=data, method=method, headers=headers), timeout=10) as response:
            payload = response.read(65537)
            assert len(payload) <= 65536
            return json.loads(payload) if payload else None

    session = api("/auth/login", "POST", {"username": "admin", "password": password})
    csrf = session["csrf_token"]
    nodes = api("/nodes")
    assert len(nodes) == 1
    node = nodes[0]["id"]
    wait(lambda: api(f"/nodes/{node}")["status"] == "online", "Agent online")
    wait(lambda: not api(f"/nodes/{node}/metrics")["stale"], "real Linux metrics")
    for unit, username in [("panel-agent.service", "panel-agent"), ("panel-server.service", "panel-server")]:
        pid = run(["/usr/bin/systemctl", "show", unit, "--property=MainPID", "--value"]).strip()
        uid = pwd.getpwnam(username).pw_uid
        assert uid != 0
        for thread in pathlib.Path(f"/proc/{pid}/task").iterdir():
            values = dict(line.split(":", 1) for line in (thread / "status").read_text().splitlines() if ":" in line)
            assert all(int(value) == uid for value in values["Uid"].split())
            assert values["NoNewPrivs"].strip() == "1"
            assert int(values["CapEff"], 16) == 0
            assert set(int(value) for value in values["Groups"].split()) <= {pwd.getpwnam(username).pw_gid}
    endpoint = pathlib.Path("/run/panel/xray-helper.sock")
    assert endpoint.stat().st_uid == pwd.getpwnam("panel-agent").pw_uid
    assert endpoint.stat().st_mode & 0o777 == 0o600
    # Root can pass filesystem permissions but must fail the helper's independent peer-UID check.
    with socket.socket(socket.AF_UNIX) as unauthorized:
        unauthorized.settimeout(5)
        unauthorized.connect(str(endpoint))
        try:
            unauthorized.sendall(b'{"operation":{"lifecycle":"status"}}\n')
            unauthorized.shutdown(socket.SHUT_WR)
            assert unauthorized.recv(4096) == b""
        except ConnectionResetError:
            pass

    def task_result(task_id):
        task = api("/tasks/" + task_id)
        if task["status"] in ["failed", "expired", "canceled"]:
            raise AssertionError("real task failed: " + json.dumps(task))
        return task if task["status"] == "succeeded" else None

    install = api(f"/nodes/{node}/xray/install", "POST", {"version": "26.10.31", "idempotency_key": str(uuid.uuid4())})
    wait(lambda: task_result(install["id"]), "actual verified Xray install through privileged bridge", 280)
    inbound = api(f"/nodes/{node}/inbounds", "POST", {"name": "acceptance", "listen_address": "127.0.0.1", "listen_port": free_port(), "target": "www.example.com:443", "server_names": ["www.example.com"], "short_ids": ["abcd"], "enabled": True})
    created = api("/clients", "POST", {"display_name": "acceptance client", "enabled": True})
    api(f"/inbounds/{inbound['id']}/clients/{created['client']['id']}", "POST")
    revision = api(f"/nodes/{node}/config/apply", "POST", {"idempotency_key": str(uuid.uuid4())})
    wait(lambda: task_result(revision["task_id"]), "real DynamicUser Xray configuration apply", 60)
    wait(lambda: api(f"/nodes/{node}/traffic")["available"], "systemd InvocationID plus tonic/prost statistics", 45)
    print("acceptance: peer denial, zero-capability Agent threads, real Xray apply and StatsService passed", flush=True)
    before = sqlite3.connect("/var/lib/panel/agent/agent.sqlite3").execute("SELECT node_id FROM identity").fetchone()[0]
    if args.failure_bundle:
        assert args.failure_sha256, "failure bundle requires a trusted digest"
        failure_version = json.loads((args.failure_bundle / "bundle.json").read_text())["version"]
        assert failure_version != version, "failure bundle must use a different version to avoid the installed bundle cache"
        failure_started = time.time()
        failure_output = cli("upgrade", "--component", "all", "--bundle", args.failure_bundle, "--bundle-sha256", args.failure_sha256, "--version", failure_version, expected=1)
        failure_log = run(["/usr/bin/journalctl", "--no-pager", "--since", f"@{failure_started}", "-u", "panel-server.service"])
        assert "Intentional acceptance startup failure" in failure_log, f"failure injection must reach actual service startup; upgrade output: {failure_output}; server journal: {failure_log}"
        restored = json.loads(cli("status"))
        assert restored["integrity_ok"] and all(value == version for value in restored["versions"].values())
        assert sqlite3.connect("/var/lib/panel/agent/agent.sqlite3").execute("SELECT node_id FROM identity").fetchone()[0] == before
        assert len(api(f"/nodes/{node}/inbounds")) == 1
        assert len(api("/clients")) == 1
        print("acceptance: failed upgrade restored real service binaries, SQLite identity and business data", flush=True)
    cli("upgrade", "--component", "all", "--bundle", args.bundle, "--bundle-sha256", args.sha256, "--version", version)
    assert json.loads(cli("status"))["integrity_ok"]
    assert sqlite3.connect("/var/lib/panel/agent/agent.sqlite3").execute("SELECT node_id FROM identity").fetchone()[0] == before
    wait(lambda: api(f"/nodes/{node}")["status"] == "online", "Agent reconnect after real upgrade")
    unit = pathlib.Path("/etc/systemd/system/panel-agent.service")
    unit.write_text(unit.read_text() + "\n# acceptance damage\n")
    assert not json.loads(cli("status"))["integrity_ok"]
    cli("repair", "--component", "agent")
    assert json.loads(cli("status"))["integrity_ok"]
    assert run(["/usr/bin/systemctl", "is-active", "panel-server.service"]).strip() == "active"
    cli("uninstall", "--component", "agent")
    assert pathlib.Path("/var/lib/panel/agent/agent.sqlite3").exists()
    assert run(["/usr/bin/systemctl", "is-active", "xray.service"]).strip() == "active"
    assert sqlite3.connect("/var/lib/panel/server/panel.sqlite3").execute("SELECT revoked_at FROM node_credentials WHERE node_id=?", (node,)).fetchone()[0] is not None
    token = api("/nodes/enrollments", "POST", {"node_id": node})["token"]
    settings = json.loads(manifest.read_text())["settings"]
    write_config(config, settings, token)
    cli("install", "--component", "agent", "--non-interactive", "--config", config, "--bundle", args.bundle, "--bundle-sha256", args.sha256)
    wait(lambda: api(f"/nodes/{node}")["status"] == "online", "retained identity re-enrolled after normal uninstall")
    assert "exact confirmation" in cli("uninstall", "--component", "all", "--purge", "--confirm", "wrong", expected=1)
    if args.backup_restore:
        for unit in ["panel-agent.service", "panel-server.service"]:
            run(["/usr/bin/systemctl", "stop", unit])

        def copy_database(source, destination):
            reader = sqlite3.connect(source)
            writer = sqlite3.connect(destination)
            try:
                reader.backup(writer)
            finally:
                writer.close()
                reader.close()

        server_database = pathlib.Path("/var/lib/panel/server/panel.sqlite3")
        agent_database = pathlib.Path("/var/lib/panel/agent/agent.sqlite3")
        for name, database in [("server", server_database), ("agent", agent_database)]:
            copy_database(database, fixture / (name + "-backup.sqlite3"))
            (fixture / (name + "-backup.sqlite3")).chmod(0o600)
        database = sqlite3.connect(server_database)
        database.execute("UPDATE clients SET display_name='restore-failure-sentinel'")
        database.commit()
        database.close()
        copy_database(fixture / "server-backup.sqlite3", server_database)
        copy_database(fixture / "agent-backup.sqlite3", agent_database)
        database = sqlite3.connect(server_database)
        database.executescript("BEGIN IMMEDIATE; UPDATE sessions SET revoked_at=unixepoch(); UPDATE enrollments SET consumed_at=COALESCE(consumed_at,unixepoch()); UPDATE node_credentials SET revoked_at=unixepoch(); UPDATE nodes SET status='offline',connection_id=NULL WHERE revoked_at IS NULL; COMMIT;")
        database.close()
        for database, username in [(server_database, "panel-server"), (agent_database, "panel-agent")]:
            account = pwd.getpwnam(username)
            for suffix in ["", "-wal", "-shm"]:
                path = pathlib.Path(str(database) + suffix)
                if path.exists():
                    assert path.is_file() and not path.is_symlink()
                    os.chown(path, account.pw_uid, account.pw_gid)
                    path.chmod(0o600)
        run(["/usr/bin/systemctl", "start", "panel-server.service"])
        def restored_ready():
            try:
                return api("/health")["status"] == "ok"
            except urllib.error.URLError:
                return False
        wait(restored_ready, "restored HTTPS ready")
        csrf = api("/auth/login", "POST", {"username": "admin", "password": password})["csrf_token"]
        assert api("/clients")[0]["display_name"] == "acceptance client"
        token = api("/nodes/enrollments", "POST", {"node_id": node})["token"]
        run(["/usr/sbin/runuser", "-u", "panel-agent", "--", "/usr/local/bin/panel-agent", "--data-dir", "/var/lib/panel/agent", "--ca-cert", "/etc/panel/agent-ca.pem", "enroll", "--server", settings["agent_server"], "--token-stdin"], stdin_data=token)
        run(["/usr/bin/systemctl", "start", "panel-agent.service"])
        wait(lambda: api(f"/nodes/{node}")["status"] == "online", "restore re-enrollment")
        print("acceptance: cold SQLite backup/restore preserved business data and required fresh sessions/node credentials", flush=True)
    if args.browser_state:
        assert args.browser_done, "browser pause requires completion marker"
        if args.browser_done.exists():
            args.browser_done.unlink()
        args.browser_state.write_text(json.dumps({"origin": origin, "username": "admin", "password": password, "node_id": node, "agent_server": settings["agent_server"], "certificate_pem": authority.read_text(), "browser_port": free_port()}))
        args.browser_state.chmod(0o600)
        print("acceptance: awaiting external real-browser E2E (private state file ready)", flush=True)
        wait(lambda: args.browser_done.exists(), "browser completion", 600)
    print(json.dumps({"real_install_upgrade_repair_uninstall_reinstall": True, "ordinary_uninstall_preserves_identity_and_running_xray": True, "peer_uid_boundary": True, "agent_zero_caps_nnp": True, "https_validated": True, "stats_with_systemd_invocation": True}), flush=True)
finally:
    if manifest.exists():
        owned = json.loads(manifest.read_text())
        if owned["settings"]["agent_name"] != label:
            raise AssertionError("cleanup refused: installation does not belong to this acceptance")
        cli("uninstall", "--component", "all", "--purge", "--remove-xray", "--confirm", "PURGE PANEL DATA")
        assert not manifest.exists()
        assert not pathlib.Path("/var/lib/panel/server").exists()
        assert not pathlib.Path("/var/lib/panel/agent").exists()
        assert not pathlib.Path("/opt/panel/xray").exists()
        print("acceptance: typed purge removed only this test installation", flush=True)
    for parent, existed in parents_before.items():
        path = pathlib.Path(parent)
        if not existed and path.exists() and not any(path.iterdir()):
            path.rmdir()
    for path in fixture.iterdir():
        if not path.is_file():
            raise AssertionError("unexpected acceptance fixture entry")
        path.unlink()
    fixture.rmdir()
