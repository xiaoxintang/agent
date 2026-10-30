#!/usr/bin/env python3
"""Verify pinned proto integrity and API coverage without compiling the project."""
import hashlib
import json
from pathlib import Path
import re


def main():
    root = Path(__file__).resolve().parent.parent
    upstream = json.loads((root / "proto/upstream.json").read_text())
    # 固定版本及逐文件摘要共同约束接口基准，防止上游文件被无意修改或遗漏。
    assert upstream["version"] == "v26.10.21"
    actual_paths = {
        str(path.relative_to(root / "proto"))
        for path in (root / "proto").rglob("*.proto")
    }
    assert actual_paths == set(upstream["sha256"]), "proto manifest differs from files"
    management = {
        "HandlerService", "StatsService", "RoutingService",
        "LoggerService", "ObservatoryService",
    }
    methods = set()
    for name, expected in upstream["sha256"].items():
        data = (root / "proto" / name).read_bytes()
        assert hashlib.sha256(data).hexdigest() == expected, f"modified proto: {name}"
        source = data.decode()
        # 所有导入必须在仓库内，构建描述集时不能隐式依赖外网文件。
        for dependency in re.findall(r'import\s+"([^"]+)"', source):
            assert dependency in actual_paths, f"missing dependency: {dependency}"
        for service in re.findall(r"\bservice\s+(\w+)\s*\{", source):
            if service in management:
                methods.update(
                    f"{service}/{method}"
                    for method in re.findall(r"\brpc\s+(\w+)\s*\(", source)
                )
    source = (root / "src/api.rs").read_text()
    # 从实际路由注册表提取映射，而不是再维护一份可能过期的接口清单。
    entries = re.findall(
        r'endpoint!\(\s*"(\w+)"\s*,\s*"([^"]+)"\s*,\s*"[^"]*"\s*,\s*\w+\s*,\s*(?:None|Some\("([^"]+)"\))',
        source,
    )
    assert len(entries) == source.count("endpoint!("), "unparsed endpoint declaration"
    entries.extend(re.findall(
        r'Endpoint\s*\{\s*verb:\s*"(\w+)"\s*,\s*path:\s*"([^"]+)"\s*,[^}]*?rpc:\s*(?:None|Some\("([^"]+)"\))',
        source,
    ))
    assert len({(verb, path) for verb, path, _ in entries}) == len(entries)
    covered = {method for _, _, method in entries if method}
    assert len(methods) == 26, f"unexpected upstream method count: {len(methods)}"
    assert covered == methods, f"missing={methods-covered}, extra={covered-methods}"
    print(f"OK: {len(actual_paths)} pinned proto files, {len(methods)} RPCs, {len(entries)} HTTP operations")


if __name__ == "__main__":
    main()
