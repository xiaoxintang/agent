#!/usr/bin/env python3
"""Verify Web interface coverage and pinned native schemas without a build."""
import json
import re
from pathlib import Path
from generate_web_operations import generate
ROOT=Path(__file__).resolve().parent.parent

def walk(value):
    if isinstance(value,dict):
        yield value
        for child in value.values():yield from walk(child)
    elif isinstance(value,list):
        for child in value:yield from walk(child)

def fields(schema):
    result=set(schema.get('properties',{}))
    for item in schema.get('oneOf',[]):result.update(fields(item))
    return result

def main():
    operations=json.loads((ROOT/'web/src/operations.json').read_text())
    assert operations==generate(),'Agent operation pages are stale; run generate_web_operations.py'
    assert len({(o['method'],o['path']) for o in operations})==len(operations)
    assert all(o['label'] and o['group'] for o in operations)
    schemas=json.loads((ROOT/'schemas/xray-native.json').read_text())
    source=json.loads((ROOT/'schemas/xray-native-source.json').read_text())
    pinned=json.loads((ROOT/'proto/upstream.json').read_text())
    assert source['commit']==pinned['commit'] and source['version']==pinned['version']
    assert not source['untyped_external_values'],source['untyped_external_values']
    for name,definition in source['types'].items():
        if name in source['custom_scalars']:continue
        missing=set(definition['fields'])-fields(schemas['native.'+name])
        assert not missing,f'{name}: missing fields {missing}'
    for item in walk(schemas):
        if '$ref'in item:
            assert item['$ref'].startswith('#/components/schemas/')
            assert item['$ref'].split('/')[-1] in schemas,item['$ref']
        if 'oneOf'in item:assert item['oneOf'],'empty schema alternatives'
    web=json.loads((ROOT/'schemas/web-api.json').read_text())
    for item in walk(web):
        if '$ref'in item:assert item['$ref'].split('/')[-1] in web['components']['schemas']
    assert '/api/assignments' in web['paths']
    assert not any('password' in path or 'credentials' in path for path in web['paths']),'password recovery must remain CLI-only'
    routes=set(re.findall(r'\.route\("([^"]+)"',re.sub(r'\s+','','\n'.join(path.read_text() for path in (ROOT/'src/web').rglob('*.rs')))))
    assert routes=={path.removeprefix('/api') for path in web['paths']},(routes,web['paths'].keys())
    print(f'OK: {len(operations)} operation pages, {len(schemas)} native schemas, {len(source["sources"])} pinned sources, {len(web["paths"])} Web API paths')
if __name__=='__main__':main()
