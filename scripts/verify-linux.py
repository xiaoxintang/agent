#!/usr/bin/env python3
"""Real Linux/systemd acceptance. Uses private temporary files and unprivileged processes only."""
import argparse
import json
import math
import os
from pathlib import Path
import re
import secrets
import selectors
import shutil
import sqlite3
import ssl
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", default="target/release")
    parser.add_argument("--unit-file", default="deploy/systemd/panel-agent.service")
    args = parser.parse_args()
    if os.uname().sysname != "Linux" or os.geteuid() == 0:
        raise RuntimeError("Requires real Linux and a non-root user")
    if Path("/proc/1/comm").read_text().strip() != "systemd":
        raise RuntimeError("Requires systemd as PID 1; no simulated service states")
    binaries = Path(args.bin_dir).resolve()
    template = Path(args.unit_file).resolve().read_text()
    for setting in ["User=panel-agent", "NoNewPrivileges=true", "CapabilityBoundingSet=", "ProtectSystem=strict", "ProtectHome=true"]:
        assert setting in template
    os.umask(0o077)
    processes = []
    logs = []
    with tempfile.TemporaryDirectory(prefix="panel-phase3-verify-") as directory:
        root = Path(directory)
        cert, key = root / "cert.pem", root / "key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(key), "-out", str(cert), "-subj", "/CN=localhost", "-addext", "subjectAltName=IP:127.0.0.1,DNS:localhost", "-addext", "basicConstraints=critical,CA:FALSE", "-addext", "keyUsage=critical,digitalSignature,keyEncipherment", "-addext", "extendedKeyUsage=serverAuth", "-days", "1"], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        env = dict(os.environ)
        for name in ["PANEL_DEV_HTTP", "PANEL_ORIGIN", "PANEL_TLS_CERT", "PANEL_TLS_KEY"]:
            env.pop(name, None)
        env.update(PANEL_BIND="127.0.0.1:0", PANEL_DATA_DIR=str(root / "server"), PANEL_TLS_CERT=str(cert), PANEL_TLS_KEY=str(key))
        password = secrets.token_hex(24)
        subprocess.run([str(binaries / "panel-server"), "init-admin", "--username", "phase3-check", "--password-stdin"], input=password + "\n", text=True, env=env, check=True, stdout=subprocess.DEVNULL, timeout=15)
        try:
            server_log = (root / "server.stderr").open("w")
            logs.append(server_log)
            server = subprocess.Popen([str(binaries / "panel-server")], env=env, stdout=subprocess.PIPE, stderr=server_log, text=True)
            processes.append(server)
            selector = selectors.DefaultSelector()
            selector.register(server.stdout, selectors.EVENT_READ)
            assert selector.select(15), "HTTPS server startup timed out"
            line = server.stdout.readline()
            selector.close()
            match = re.search(r"listening on (https://127\.0\.0\.1:\d+)", line)
            assert match, "HTTPS server failed to start"
            base = match.group(1)
            context = ssl.create_default_context(cafile=str(cert))
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))
            cookie = csrf = None

            def request(method, path, payload=None):
                headers = {"Origin": base}
                if cookie:
                    headers.update(Cookie=cookie, **{"X-CSRF-Token": csrf})
                data = None
                if payload is not None:
                    headers["Content-Type"] = "application/json"
                    data = json.dumps(payload).encode()
                req = urllib.request.Request(base + "/api/v1" + path, data=data, headers=headers, method=method)
                try:
                    response = opener.open(req, timeout=8)
                except urllib.error.HTTPError as error:
                    response = error
                with response:
                    body = response.read(256 * 1024)
                    return response.status, response.headers, json.loads(body) if body else None

            status, headers, session = request("POST", "/auth/login", {"username": "phase3-check", "password": password})
            assert status == 200 and "Secure" in headers["Set-Cookie"]
            cookie, csrf = headers["Set-Cookie"].split(";", 1)[0], session["csrf_token"]
            status, _, node = request("POST", "/nodes", {"name": "Linux acceptance"})
            assert status == 201
            node_id = node["id"]
            status, _, enrollment = request("POST", "/nodes/enrollments", {"node_id": node_id})
            assert status == 200
            agent_args = [str(binaries / "panel-agent"), "--data-dir", str(root / "agent"), "--ca-cert", str(cert)]
            subprocess.run(agent_args + ["enroll", "--server", base.replace("https:", "wss:") + "/agent/v1/connect", "--token-stdin"], input=enrollment["token"] + "\n", text=True, check=True, stdout=subprocess.DEVNULL, timeout=15)
            agent_log = (root / "agent.log").open("w")
            logs.append(agent_log)
            agent = subprocess.Popen(agent_args + ["run"], stdout=agent_log, stderr=agent_log)
            processes.append(agent)

            def wait_for(function, timeout=20):
                deadline = time.monotonic() + timeout
                while True:
                    assert server.poll() is None and agent.poll() is None, "Acceptance process exited"
                    value = function()
                    if value:
                        return value
                    assert time.monotonic() < deadline, "Linux acceptance timed out"
                    time.sleep(0.2)

            def sampled():
                status, _, result = request("GET", f"/nodes/{node_id}/metrics")
                assert status == 200
                snapshot = result["snapshot"]
                return result if snapshot and snapshot["cpu_percent"] is not None and snapshot["network"] and snapshot["network"]["received_bytes_per_second"] is not None and not result["stale"] else None

            metrics = wait_for(sampled)
            snapshot = metrics["snapshot"]
            assert not snapshot["issues"]
            assert math.isfinite(snapshot["cpu_percent"]) and 0 <= snapshot["cpu_percent"] <= 100
            for field in ["memory", "disk"]:
                total, used, available = (int(snapshot[field][key]) for key in ["total_bytes", "used_bytes", "available_bytes"])
                assert total > 0 and 0 <= used <= total and 0 <= available <= total and used + available <= total
            for field in ["received_bytes_per_second", "transmitted_bytes_per_second"]:
                assert math.isfinite(snapshot["network"][field]) and snapshot["network"][field] >= 0
            actual_memory = int(re.search(r"^MemTotal:\s+(\d+) kB$", Path("/proc/meminfo").read_text(), re.MULTILINE).group(1)) * 1024
            actual_disk = os.statvfs("/")
            assert int(snapshot["memory"]["total_bytes"]) == actual_memory
            assert int(snapshot["disk"]["total_bytes"]) == actual_disk.f_blocks * actual_disk.f_frsize
            _, _, observed = request("GET", f"/nodes/{node_id}/status")
            assert observed["os"] == "linux" and observed["status"] == "online"
            thread_files = list(Path(f"/proc/{agent.pid}/task").glob("*/status"))
            assert thread_files
            for path in thread_files:
                fields = dict(line.split(":", 1) for line in path.read_text().splitlines() if ":" in line)
                assert int(fields["Uid"].split()[1]) == os.geteuid()
                assert fields["NoNewPrivs"].strip() == "1"
                assert int(fields["CapEff"].strip(), 16) == 0

            services = {}
            for service in ["panel_agent", "xray"]:
                status, _, task = request("POST", f"/nodes/{node_id}/tasks", {"operation": {"service_status": {"service": service}}, "idempotency_key": str(uuid.uuid4())})
                assert status == 201
                def completed():
                    _, _, value = request("GET", f"/tasks/{task['id']}")
                    return value if value["status"] in ["succeeded", "failed", "expired"] else None
                result = wait_for(completed)
                assert result["status"] == "succeeded", result["result"]
                assert result["result"]["status"] == "service_status"
                report = result["result"]["data"]
                assert report["service"] == service and report["load_state"]
                services[service] = {key: report[key] for key in ["load_state", "active_state", "sub_state"]}

            forbidden = [{"service_status": {"service": "ssh.service"}}, {"service_status": {"service": "../../private"}}, {"service_status": {"service": "panel_agent", "method": "RestartUnit"}}, {"service_restart": {"service": "panel_agent"}}, {"shell": "id"}]
            for operation in forbidden:
                status, _, response = request("POST", f"/nodes/{node_id}/tasks", {"operation": operation, "idempotency_key": str(uuid.uuid4())})
                assert status == 400 and response["error"]["code"] == "invalid_json"
            with sqlite3.connect(root / "server" / "panel.sqlite3") as database:
                assert database.execute("SELECT COUNT(*) FROM node_metrics").fetchone()[0] == 1
                assert database.execute("SELECT COUNT(*) FROM audit_logs WHERE action='task.reject' AND result='denied'").fetchone()[0] == len(forbidden)
                assert database.execute("SELECT COUNT(*) FROM audit_logs WHERE action='service.status'").fetchone()[0] == 2

            if shutil.which("systemd-analyze"):
                # Validate a temporary copy against available test binary/user; never install/start a unit.
                unit = template.replace("/usr/local/bin/panel-agent", str(binaries / "panel-agent")).replace("User=panel-agent", f"User={os.geteuid()}").replace("Group=panel-agent", f"Group={os.getegid()}")
                unit_file = root / "panel-agent.service"
                unit_file.write_text(unit)
                subprocess.run(["systemd-analyze", "verify", str(unit_file)], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=15)
            print(json.dumps({"linux_acceptance": "passed", "transport": "verified_wss", "unprivileged": True, "all_threads_no_new_privileges": True, "cpu_percent": snapshot["cpu_percent"], "memory_total_bytes": snapshot["memory"]["total_bytes"], "disk_total_bytes": snapshot["disk"]["total_bytes"], "network_rates_nonnegative": True, "services": services, "unauthorized_operations_denied": len(forbidden), "latest_snapshot_rows": 1}, ensure_ascii=False))
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)
                if process.stdout:
                    process.stdout.close()
            for log in logs:
                log.close()


if __name__ == "__main__":
    main()
