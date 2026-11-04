#!/usr/bin/env python3
"""Package actual Linux binaries; never builds or downloads dependencies."""
import argparse
import hashlib
import json
import pathlib
import shutil

parser = argparse.ArgumentParser()
parser.add_argument("--binaries", required=True, type=pathlib.Path)
parser.add_argument("--output", required=True, type=pathlib.Path)
parser.add_argument("--arch", required=True, choices=["x86_64", "aarch64"])
parser.add_argument("--version", required=True)
args = parser.parse_args()
if not args.version or not all(ch.isdigit() or ch in ".-" for ch in args.version):
    parser.error("version must contain only digits, dots and hyphens")
args.output.mkdir(mode=0o700)
files = {}
for name in ["panel-server", "panel-agent", "panel-installer"]:
    source = args.binaries / name
    data = source.read_bytes()
    machine = 62 if args.arch == "x86_64" else 183
    if len(data) < 20 or data[:6] != b"\x7fELF\x02\x01" or int.from_bytes(data[18:20], "little") != machine:
        raise SystemExit(f"{name}: expected matching Linux ELF64 executable")
    shutil.copyfile(source, args.output / name)
    (args.output / name).chmod(0o755)
    files[name] = hashlib.sha256(data).hexdigest()
metadata = {"format": 1, "version": args.version, "arch": args.arch, "files": files}
content = (json.dumps(metadata, indent=2, sort_keys=True) + "\n").encode()
(args.output / "bundle.json").write_bytes(content)
print(json.dumps({"bundle": str(args.output), "bundle_sha256": hashlib.sha256(content).hexdigest()}))
