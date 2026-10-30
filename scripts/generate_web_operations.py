#!/usr/bin/env python3
"""Keep every registered Agent operation discoverable in the Web UI (no build)."""
import json
import re
from pathlib import Path
ROOT=Path(__file__).resolve().parent.parent
LABELS={
'UpdateAgent':'更新 Agent','Install':'安装 Xray','Versions':'本地版本','DeleteVersion':'删除版本','Status':'进程与同步状态','Capabilities':'接口能力','Start':'启动 Xray','Stop':'停止 Xray','Restart':'重启并恢复配置','GetConfig':'查看完整配置','PutConfig':'替换完整配置','AddInbound':'创建入站','RemoveInbound':'删除入站','PatchInbound':'修改入站用户','AddOutbound':'创建出站','RemoveOutbound':'删除出站','PatchOutbound':'修改出站','AddUser':'添加原生用户','RemoveUser':'删除原生用户','AddRules':'追加或替换路由规则','RemoveRule':'删除路由规则','Override':'覆盖负载均衡目标','Events':'订阅路由事件',
'HandlerService/ListInbounds':'查询入站','HandlerService/GetInboundUsers':'查询原生用户','HandlerService/GetInboundUsersCount':'查询用户数量','HandlerService/ListOutbounds':'查询出站','StatsService/GetStats':'读取计数器','StatsService/QueryStats':'查询计数器','StatsService/GetStatsOnline':'查询在线数','StatsService/GetStatsOnlineIpList':'查询在线 IP','StatsService/GetAllOnlineUsers':'查询在线用户','StatsService/GetUsersStats':'查询用户流量','StatsService/GetSysStats':'系统运行统计','RoutingService/ListRule':'查询路由规则','RoutingService/TestRoute':'测试路由','RoutingService/GetBalancerInfo':'查询负载均衡','LoggerService/RestartLogger':'重开日志','ObservatoryService/GetOutboundStatus':'出站探测'}
def generate():
    source=(ROOT/'src/api.rs').read_text()
    entries=[]
    pattern=r'endpoint!\(\s*"(\w+)"\s*,\s*"([^"]+)"\s*,\s*"([^"]*)"\s*,\s*(\w+)\s*,\s*(None|Some\("[^"]+"\))\s*,.*?,\s*(true|false)\s*\)'
    for method,path,summary,kind,rpc,mutating in re.findall(pattern,source,re.S):
        root=path.split('/')[1]
        group={'install':'版本与进程','versions':'版本与进程','status':'版本与进程','capabilities':'版本与进程','start':'版本与进程','stop':'版本与进程','restart':'版本与进程','config':'完整配置','inbounds':'入站与用户','outbounds':'出站管理','stats':'统计与在线','routing':'路由与负载均衡','logger':'日志与探测','observatory':'日志与探测'}[root]
        label=LABELS.get(kind if kind!='Rpc' else rpc[6:-2],summary)
        entries.append(dict(method=method,path='/api/v1/xray'+path,label=label,group=group,mutating=mutating=='true'))
    entries.insert(0,dict(method='post',path='/api/v1/agent/update',label='更新 Agent',group='版本与进程',mutating=True))
    assert len(entries)==source.count('endpoint!(')+1
    return entries
if __name__=='__main__':(ROOT/'web/src/operations.json').write_text(json.dumps(generate(),ensure_ascii=False,indent=2)+'\n')
