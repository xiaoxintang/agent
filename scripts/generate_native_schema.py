#!/usr/bin/env python3
"""Extract JSON-tagged Go configuration fields from the pinned Xray source tree.
No network access or Go compiler is needed. Supply an unpacked upstream checkout.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parent.parent
UPSTREAM = json.loads((ROOT / 'proto/upstream.json').read_text())


def ref(name):
    return {'$ref': '#/components/schemas/native.' + name}


def extract(source):
    # Keep strings/tags intact, but remove comments before balancing struct braces.
    source = re.sub(r'/\*.*?\*/', '', source, flags=re.S)
    source = re.sub(r'(?m)^\s*//.*$', '', source)
    result = {}
    for match in re.finditer(r'\btype\s+(\w+)\s+struct\s*\{', source):
        depth, index, quoted = 1, match.end(), None
        begin = index
        while depth and index < len(source):
            ch = source[index]
            if quoted:
                if ch == quoted and (quoted == '`' or source[index - 1] != '\\'):
                    quoted = None
            elif ch in ('`', '"', "'"):
                quoted = ch
            elif ch == '{':
                depth += 1
            elif ch == '}':
                depth -= 1
            index += 1
        body = source[begin:index - 1]
        fields = {}
        embedded = []
        for line in body.splitlines():
            field = re.match(r'\s*(\w+)\s+(.+?)\s+`[^`]*json:"([^",]+)[^`]*`', line)
            if field and field[3] != '-':
                fields[field[3]] = field[2].strip()
            elif re.fullmatch(r'\s*\*?[A-Z]\w*\s*', line):
                embedded.append(line.strip().lstrip('*'))
        result[match[1]] = {'fields': fields, 'embedded': embedded}
    return result


def generate(source_root):
    types, sources, aliases = {}, {}, {}
    for path in sorted((source_root / 'infra/conf').glob('*.go')):
        if path.name.endswith('_test.go'):
            continue
        text = path.read_text()
        name = str(path.relative_to(source_root))
        sources[name] = hashlib.sha256(path.read_bytes()).hexdigest()
        for key, value in extract(text).items():
            if key in types and types[key] != value:
                # Function-local decoding structs can share names. Include all fields.
                types[key]['fields'].update(value['fields'])
            else:
                types[key] = value
        for key, value in re.findall(r'(?m)^type\s+(\w+)\s+([^\n{]+)$', text):
            aliases[key] = value.strip()
    for filename, original, target in [
        ('common/protocol/headers.pb.go','SecurityConfig','protocol.SecurityConfig'),
        ('transport/internet/finalmask/realm/config.pb.go','PortMapping','realm.PortMapping'),
        ('app/router/config.pb.go','StrategyWeight','router.StrategyWeight'),
        ('proxy/vless/account.pb.go','Reverse','account.vless.Reverse'),
        ('app/proxyman/config.pb.go','SniffingConfig','proxyman.SniffingConfig'),
        ('common/geodata/geodat.pb.go','DomainRule','geodata.DomainRule'),
        ('common/geodata/geodat.pb.go','IPRule','geodata.IPRule'),
    ]:
        path=source_root/filename
        sources[filename]=hashlib.sha256(path.read_bytes()).hexdigest()
        types[target]=extract(path.read_text())[original]
    schemas = {}
    custom = {
        'Address': {'type': 'string'},
        'duration.Duration': {'oneOf':[{'type':'string'},{'type':'number'}]},
        'SecurityType': {'type':'integer'},
        'Reverse': ref('account.vless.Reverse'),
        'StringList': {'oneOf': [{'type': 'string'}, {'type': 'array', 'items': {'type': 'string'}}]},
        'NetworkList': {'oneOf': [{'type': 'string'}, {'type': 'array', 'items': {'type': 'string'}}]},
        'PortList': {'oneOf': [{'type': 'integer', 'minimum': 1, 'maximum': 65535}, {'type': 'string'}]},
        'PortRange': {'oneOf': [{'type': 'integer'}, {'type': 'string'}]},
        'Int32Range': {'oneOf': [{'type': 'integer'}, {'type': 'string'}]},
        'HostsWrapper': {'type': 'object', 'additionalProperties': {'oneOf': [{'type': 'string'}, {'type': 'array', 'items': {'type': 'string'}}]}},
        'FakeDNSConfig': {'oneOf':[ref('FakeDNSPoolElementConfig'),{'type': 'array', 'items': ref('FakeDNSPoolElementConfig')}]},
    }
    unknown = set()
    def shape(go_type, stack=()):
        go_type = go_type.lstrip('*')
        if go_type.startswith('[]'):
            return {'type': 'array', 'items': shape(go_type[2:], stack)}
        if go_type.startswith('map['):
            return {'type': 'object', 'additionalProperties': shape(go_type.split(']', 1)[1], stack)}
        if go_type in custom:
            return custom[go_type]
        if go_type in ('string', 'TransportProtocol'):
            return {'type': 'string'}
        if go_type == 'bool':
            return {'type': 'boolean'}
        if re.fullmatch(r'(u?int(8|16|32|64)?|byte)', go_type):
            # Native JSON uses JSON numbers; protobuf's int64 string rule is separate.
            return {'type': 'integer', **({'minimum': 0} if go_type.startswith('u') or go_type == 'byte' else {})}
        if go_type in ('float32', 'float64'):
            return {'type': 'number'}
        if go_type in types:
            return ref(go_type)
        if go_type in aliases and go_type not in stack:
            return shape(aliases[go_type], stack + (go_type,))
        if go_type in ('json.RawMessage', 'interface{}', 'any'):
            return {'description': 'Polymorphic native JSON; use the visual field tree.'}
        unknown.add(go_type)
        return {'description': 'Native Go value: ' + go_type}
    for name, definition in types.items():
        fields = dict(definition['fields'])
        for base in definition['embedded']:
            fields = {**types.get(base, {}).get('fields', {}), **fields}
        schemas['native.' + name] = {'title': name, 'type': 'object', 'properties': {key: shape(value) for key, value in fields.items()}, 'additionalProperties': True}
    for name, value in custom.items():
        schemas['native.' + name] = value
    inbound = {'tunnel':'DokodemoConfig','dokodemo-door':'DokodemoConfig','http':'HTTPServerConfig','shadowsocks':'ShadowsocksServerConfig','mixed':'SocksServerConfig','socks':'SocksServerConfig','vless':'VLessInboundConfig','vmess':'VMessInboundConfig','trojan':'TrojanServerConfig','wireguard':'WireGuardConfig','hysteria':'HysteriaServerConfig','masque':'MasqueServerConfig','tun':'TunConfig'}
    outbound = {'block':'BlackholeConfig','blackhole':'BlackholeConfig','loopback':'LoopbackConfig','direct':'FreedomConfig','freedom':'FreedomConfig','http':'HTTPClientConfig','shadowsocks':'ShadowsocksClientConfig','socks':'SocksClientConfig','vless':'VLessOutboundConfig','vmess':'VMessOutboundConfig','trojan':'TrojanClientConfig','hysteria':'HysteriaClientConfig','masque':'MasqueClientConfig','dns':'DNSOutboundConfig','wireguard':'WireGuardConfig'}
    for title, base, protocols in [('InboundConfig','InboundDetourConfig',inbound),('OutboundConfig','OutboundDetourConfig',outbound)]:
        alternatives = []
        for protocol, settings in protocols.items():
            assert settings in types, settings
            props = dict(schemas['native.'+base]['properties'])
            props['protocol'] = {'type':'string','enum':[protocol]}
            props['tag'] = {'type':'string','minLength':1}
            props['settings'] = ref(settings)
            alternatives.append({'type':'object','properties':props,'required':['tag','protocol'],'additionalProperties':True})
        schemas[title] = {'oneOf':alternatives,'discriminator':{'propertyName':'protocol'}}
    schemas['native.Config']['properties']['inbounds'] = {'type':'array','items':{'$ref':'#/components/schemas/InboundConfig'}}
    schemas['native.Config']['properties']['outbounds'] = {'type':'array','items':{'$ref':'#/components/schemas/OutboundConfig'}}
    schemas['XrayConfig'] = schemas['native.Config'] | {'required':['api','stats','inbounds','outbounds']}
    schemas['RoutingRule'] = schemas['native.RawFieldRule']
    schemas['RoutingConfig'] = schemas['native.RouterConfig']
    schemas['BalancerConfig'] = schemas['native.BalancingRule']
    schemas['StreamSettings'] = schemas['native.StreamConfig']
    schemas['native.RouterConfig']['properties']['rules'] = {'type':'array','items':{'$ref':'#/components/schemas/RoutingRule'}}
    # RawMessage user decoding merges protocol.User and account fields.
    user = dict(schemas['native.User']['properties'])
    for path in sorted((source_root/'proxy').rglob('account.pb.go')):
        sources[str(path.relative_to(source_root))] = hashlib.sha256(path.read_bytes()).hexdigest()
        for name, definition in extract(path.read_text()).items():
            if name == 'Account':
                user.update({key:shape(value) for key,value in definition['fields'].items()})
    for name in ('TrojanUserConfig','ShadowsocksUserConfig','HysteriaUserConfig','MasqueUserConfig','WireGuardPeerConfig'):
        user.update(schemas['native.'+name]['properties'])
    user.update({'id':{'type':'string'},'email':{'type':'string','minLength':1},'level':{'type':'integer','minimum':0},'password':{'type':'string','format':'password'},'auth':{'type':'string','format':'password'},'pass':{'type':'string','format':'password'},'reverse':ref('account.vless.Reverse')})
    # Decoders selected by native discriminators; raw payloads retain a field tree.
    loader_source=(source_root/'infra/conf/transport_finalmask.go').read_text()
    masks=dict(re.findall(r'"([^"\n]+)":\s*func\(\) interface\{\} \{ return new\((\w+)\)',loader_source))
    def variants(base, discriminator, choices):
        alternatives=[]
        for name, target in choices.items():
            if target not in types: continue
            props={key:shape(value) for key,value in types[base]['fields'].items()}
            props[discriminator]={'type':'string','enum':[name]}
            props['settings']=ref(target)
            alternatives.append({'type':'object','properties':props,'required':[discriminator],'additionalProperties':True})
        return {'oneOf':alternatives,'discriminator':{'propertyName':discriminator}}
    if masks: schemas['native.Mask']=variants('Mask','type',masks)
    for kind in ('tcp','udp'):
        block=re.search(kind+r'maskLoader = NewJSONConfigLoader\(ConfigCreatorCache\{(.*?)\}, "type"',loader_source,re.S)[1]
        choices=dict(re.findall(r'"([^"\n]+)":\s*func\(\) interface\{\} \{ return new\((\w+)\)',block))
        schemas['native.Mask.'+kind]=variants('Mask','type',choices)
        schemas['native.FinalMask']['properties'][kind]={'type':'array','items':ref('Mask.'+kind)}
    schemas['native.StrategyConfig']=variants('StrategyConfig','type',{'random':'strategyEmptyConfig','roundRobin':'strategyEmptyConfig','leastPing':'strategyEmptyConfig','leastLoad':'strategyLeastLoadConfig'})
    http_header=schemas['native.Authenticator'] | {'properties':{**schemas['native.Authenticator']['properties'],'type':{'type':'string','enum':['http']}},'required':['type']}
    schemas['native.TCPConfig']['properties']['header']={'oneOf':[http_header,{'type':'object','properties':{'type':{'type':'string','enum':['none']}},'required':['type']}],'discriminator':{'propertyName':'type'}}
    schemas['native.SplitHTTPConfig']['properties']['extra']=ref('SplitHTTPConfig')
    schemas['native.NameServerConfig']={'oneOf':[{'type':'string'},schemas['native.NameServerConfig']]}
    schemas['NativeUser'] = {'type':'object','properties':user,'required':['email'],'additionalProperties':True}
    schemas['native.ProtocolUser']={k:v for k,v in schemas['NativeUser'].items() if k!='required'}
    for name in ('VLessInboundConfig','VMessInboundConfig','VLessOutboundVnext','VMessOutboundTarget'):
        for field in ('users','clients'):
            if field in schemas['native.'+name]['properties']:
                schemas['native.'+name]['properties'][field] = {'type':'array','items':ref('ProtocolUser')}
    for protocol in ('HTTP','Socks'):
        schemas['native.'+protocol+'User']={'type':'object','properties':{**schemas['native.User']['properties'],**schemas['native.'+protocol+'Account']['properties']},'additionalProperties':True}
        schemas['native.'+protocol+'RemoteConfig']['properties']['users']={'type':'array','items':ref(protocol+'User')}
    for name, schema in schemas.items():
        for key, field in schema.get('properties',{}).items():
            if any(word in key.lower() for word in ('password','privatekey','presharedkey')) and field.get('type')=='string':
                field['format']='password'
    inventory = {'repository':UPSTREAM['repository'],'commit':UPSTREAM['commit'],'version':UPSTREAM['version'],'sources':sources,'types':types,'custom_scalars':sorted(custom),'untyped_external_values':sorted(unknown)}
    return schemas, inventory


def main():
    parser=argparse.ArgumentParser();parser.add_argument('source',type=Path);args=parser.parse_args()
    schemas,inventory=generate(args.source)
    manifest=ROOT/'schemas/xray-native-source.json'
    existing=json.loads(manifest.read_text()) if manifest.exists() else {}
    if existing.get('commit')==UPSTREAM['commit']:
        assert inventory['sources']==existing['sources'], 'source hashes differ from the pinned manifest'
    else:
        head=subprocess.check_output(['git','-C',str(args.source),'rev-parse','HEAD'],text=True).strip()
        assert head==UPSTREAM['commit'], 'new baseline requires a checkout at the pinned commit'
    (ROOT/'schemas/xray-native.json').write_text(json.dumps(schemas,ensure_ascii=False,indent=2)+'\n')
    (ROOT/'schemas/xray-native-source.json').write_text(json.dumps(inventory,ensure_ascii=False,indent=2)+'\n')
    print(f"Generated {len(schemas)} schemas from {len(inventory['sources'])} pinned sources")
    print('Untyped external values:', inventory['untyped_external_values'])
if __name__=='__main__':main()
